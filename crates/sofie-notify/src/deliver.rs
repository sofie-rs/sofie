use serde::{Deserialize, Serialize};
use sofie_core::{Clock, Error, Instant, Result, Timestamp, Uuid};
use sofie_db::worker::JobContext;
use sofie_db::{Jobs, Outcome, Plan, Queued, Read, Resolve, Select, Stamp};
use sofie_mail::{Address, Mail, Mailer, Template};
use sofie_values::LocalDateTime;

use crate::preferences::jiff_zone;
use crate::store::{
    CHOICE_PRINCIPAL, CHOICE_TOPIC, Choice, DigestItem, InboxEntry, SETTINGS_PRINCIPAL, Settings,
};
use crate::{Channel, Notice, Recipient, push};

pub(crate) const JOB: &str = "sofie_notify.deliver";
const MAIL: Template = Template::new(
    "sofie_notify/notification",
    sofie_db::embed!(
        "src/mails/notification.html",
        compiled = "mails/sofie_notify/notification.sft"
    ),
);

/// The topic's declaration travels with the notification, so the worker needs no registry.
#[derive(Serialize, Deserialize)]
pub(crate) struct Sending {
    pub(crate) recipient: Recipient,
    pub(crate) topic: String,
    pub(crate) defaults: Vec<Channel>,
    pub(crate) urgent: bool,
    pub(crate) digested: bool,
    pub(crate) notice: Notice,
    /// Set when quiet hours held mail and push back: only these are left to send.
    pub(crate) held: Option<Vec<Channel>>,
}

impl Resolve<JobContext> for Sending {
    fn resolve(context: &mut JobContext) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(crate::job_input(context, JOB))
    }
}

/// Mail is optional: without it the mail channel is skipped, not every channel failed.
pub(crate) struct Mails(pub(crate) Option<Mailer>);

impl Resolve<JobContext> for Mails {
    async fn resolve(context: &mut JobContext) -> Result<Self> {
        Ok(Self(Mailer::resolve(context).await.ok()))
    }
}

pub(crate) async fn deliver(
    sending: Sending,
    read: Read,
    clock: Clock,
    jobs: Jobs,
    mails: Mails,
) -> Result<Delivered> {
    let principal = sending.recipient.principal;
    if sofie_db::forgotten(&read, principal).await? {
        return Ok(Delivered::default());
    }
    let now = clock.now().timestamp();
    let settings = Settings::query()
        .where_(SETTINGS_PRINCIPAL.eq(principal))
        .first(&read)
        .await?;
    let channels = chosen(&read, &sending).await?;
    let mut delivered = Delivered {
        recipient: Some(principal),
        ..Delivered::default()
    };
    if sending.held.is_none() && channels.contains(&Channel::Inbox) {
        delivered.inbox = Some(InboxEntry {
            meta: sofie_db::Meta::new()?,
            principal,
            topic: sending.topic.clone(),
            notice: sending.notice.clone(),
            created_at: now,
            read_at: None,
        });
    }
    let loud: Vec<Channel> = channels
        .into_iter()
        .filter(|channel| *channel != Channel::Inbox)
        .collect();
    if loud.is_empty() {
        return Ok(delivered);
    }
    let quiet_end = settings
        .as_ref()
        .filter(|_| !sending.urgent)
        .map(|settings| quiet_until(settings, now))
        .transpose()?
        .flatten();
    if let Some(end) = quiet_end {
        let later = Sending {
            held: Some(loud),
            ..sending
        };
        delivered.later = Some(Jobs::queue_named_at(JOB, &later, Instant::from(end))?);
        return Ok(delivered);
    }
    if loud.contains(&Channel::Mail) {
        let digest = settings
            .as_ref()
            .is_some_and(|settings| settings.digest.is_on());
        mail(
            &mut delivered,
            &sending,
            mails.0.as_ref(),
            digest && sending.digested,
            now,
        )?;
    }
    if loud.contains(&Channel::Push) {
        for subscription in push::subscriptions(&read, principal).await? {
            let pushing = push::Pushing {
                subscription,
                notice: sending.notice.clone(),
            };
            delivered.pushes.push(jobs.queue(push::push, &pushing)?);
        }
    }
    Ok(delivered)
}

async fn chosen(read: &Read, sending: &Sending) -> Result<Vec<Channel>> {
    let choices = Choice::query()
        .where_(CHOICE_PRINCIPAL.eq(sending.recipient.principal))
        .where_(CHOICE_TOPIC.eq(sending.topic.clone()))
        .all(read)
        .await?;
    let on = |channel: Channel| {
        choices
            .iter()
            .find(|choice| choice.channel == channel)
            .map_or_else(
                || sending.defaults.contains(&channel),
                |choice| choice.enabled,
            )
    };
    Ok(Channel::ALL
        .into_iter()
        .filter(|channel| on(*channel))
        .filter(|channel| {
            sending
                .held
                .as_ref()
                .is_none_or(|held| held.contains(channel))
        })
        .collect())
}

fn mail(
    delivered: &mut Delivered,
    sending: &Sending,
    mailer: Option<&Mailer>,
    digest: bool,
    now: Timestamp,
) -> Result<()> {
    let Some(email) = &sending.recipient.email else {
        tracing::debug!(
            topic = sending.topic,
            "no address to mail a notification to"
        );
        return Ok(());
    };
    let Ok(address) = Address::new(email) else {
        tracing::warn!(
            topic = sending.topic,
            "the recipient's address cannot receive mail; the mail is skipped"
        );
        return Ok(());
    };
    let Some(mailer) = mailer else {
        tracing::warn!(
            topic = sending.topic,
            "a notification was due by mail, but mail is not set up"
        );
        return Ok(());
    };
    let notice = &sending.notice;
    if digest {
        delivered.digest = Some(DigestItem {
            meta: sofie_db::Meta::new()?,
            principal: sending.recipient.principal,
            email: email.as_str().to_owned(),
            notice: notice.clone(),
            created_at: now,
            sent_at: None,
        });
        return Ok(());
    }
    let link = match notice
        .link
        .as_deref()
        .map(|path| mailer.link(path))
        .transpose()
    {
        Ok(link) => link,
        Err(error) => {
            tracing::warn!(topic = sending.topic, error = %error, "a notification's mail cannot link back to the app; the mail is skipped");
            return Ok(());
        }
    };
    let body = mailer.body_from(
        &MAIL,
        &serde_json::json!({ "body": notice.body, "link": link }),
    )?;
    let mail = Mail::new(address, &notice.title, body)?;
    delivered.mail = Some(mailer.send(mail)?);
    Ok(())
}

fn quiet_until(settings: &Settings, now: Timestamp) -> Result<Option<Timestamp>> {
    let (Some(from), Some(until), Some(zone)) =
        (settings.quiet_from, settings.quiet_until, &settings.zone)
    else {
        return Ok(None);
    };
    let local = now.to_zoned(jiff_zone(zone)?);
    let time = crate::TimeOfDay::of(local.time());
    let quiet = if from <= until {
        from <= time && time < until
    } else {
        time >= from || time < until
    };
    if !quiet {
        return Ok(None);
    }
    let date = if time < until {
        local.date()
    } else {
        local
            .date()
            .tomorrow()
            .map_err(|error| Error::internal("finding the end of quiet hours", error))?
    };
    let end = LocalDateTime::new(date.to_datetime(until.civil()), zone.clone())
        .instant()?
        .timestamp();
    if end > now {
        return Ok(Some(end));
    }
    // Inside the second pass of a repeated hour the first one is already over.
    let later = jiff_zone(zone)?
        .to_ambiguous_zoned(date.to_datetime(until.civil()))
        .later()
        .map_err(|error| Error::internal("finding the end of quiet hours", error))?;
    Ok(Some(later.timestamp()))
}

#[derive(Default)]
pub(crate) struct Delivered {
    recipient: Option<Uuid>,
    inbox: Option<InboxEntry>,
    digest: Option<DigestItem>,
    mail: Option<Queued>,
    later: Option<Queued>,
    pushes: Vec<Queued>,
}

impl Outcome for Delivered {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        if let Some(principal) = self.recipient {
            sofie_db::unless_forgotten(plan, principal);
        }
        self.inbox.plan(plan)?;
        self.digest.plan(plan)?;
        self.mail.plan(plan)?;
        self.later.plan(plan)?;
        self.pushes.plan(plan)
    }

    fn committed(&mut self, stamp: Stamp) {
        self.inbox.committed(stamp);
        self.digest.committed(stamp);
    }
}

#[cfg(test)]
mod tests {
    use sofie_core::Uuid;
    use sofie_values::TimeZone;

    use super::*;
    use crate::TimeOfDay;

    fn quiet(from: &str, until: &str) -> Settings {
        let mut settings = Settings::new(Uuid::now_v7().unwrap()).unwrap();
        settings.zone = Some(TimeZone::parse("Europe/Istanbul").unwrap());
        settings.quiet_from = Some(TimeOfDay::parse(from).unwrap());
        settings.quiet_until = Some(TimeOfDay::parse(until).unwrap());
        settings
    }

    fn at(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    #[test]
    fn without_mail_set_up_nothing_is_held_for_a_digest() {
        let email = sofie_values::Email::parse("ada@example.test").unwrap();
        let sending = Sending {
            recipient: crate::Recipient::new(Uuid::now_v7().unwrap()).email(&email),
            topic: "shop.order_shipped".to_owned(),
            defaults: vec![Channel::Mail],
            urgent: false,
            digested: true,
            notice: Notice::new("Shipped").unwrap(),
            held: None,
        };
        let mut delivered = Delivered::default();

        mail(
            &mut delivered,
            &sending,
            None,
            true,
            at("2026-10-14T09:00:00Z"),
        )
        .unwrap();

        assert!(delivered.digest.is_none());
    }

    #[test]
    fn quiet_hours_over_midnight_end_the_next_morning_in_the_zone() {
        let overnight = quiet("22:00", "07:00");

        let late = quiet_until(&overnight, at("2026-10-14T20:30:00Z")).unwrap();
        let early = quiet_until(&overnight, at("2026-10-14T23:00:00Z")).unwrap();
        let day = quiet_until(&overnight, at("2026-10-14T09:00:00Z")).unwrap();

        assert_eq!(late, Some(at("2026-10-15T04:00:00Z")));
        assert_eq!(early, Some(at("2026-10-15T04:00:00Z")));
        assert_eq!(day, None);
    }

    #[test]
    fn quiet_hours_ending_in_a_repeated_hour_end_after_now() {
        let mut night = quiet("00:00", "01:30");
        night.zone = Some(TimeZone::parse("America/New_York").unwrap());
        let second_one_ten = at("2026-11-01T06:10:00Z");

        let end = quiet_until(&night, second_one_ten).unwrap();

        assert_eq!(end, Some(at("2026-11-01T06:30:00Z")));
    }

    #[test]
    fn quiet_hours_within_a_day_end_that_day() {
        let lunch = quiet("12:00", "13:00");

        assert_eq!(
            quiet_until(&lunch, at("2026-10-14T09:30:00Z")).unwrap(),
            Some(at("2026-10-14T10:00:00Z"))
        );
        assert_eq!(
            quiet_until(&lunch, at("2026-10-14T10:00:00Z")).unwrap(),
            None
        );
    }
}
