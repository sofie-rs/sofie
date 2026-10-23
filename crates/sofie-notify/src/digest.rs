use serde::{Deserialize, Serialize};
use serde_json::json;
use sofie_core::{Clock, Instant, Result, Uuid};
use sofie_db::worker::JobContext;
use sofie_db::{Jobs, Outcome, Plan, Queued, Read, Resolve, Select, Stamp};
use sofie_mail::{Address, Mail, Template};
use sofie_values::Email;

use crate::deliver::Mails;
use crate::store::{
    DIGEST_CREATED_AT, DIGEST_PRINCIPAL, DIGEST_SENT_AT, DigestItem, SETTINGS_PRINCIPAL, Settings,
};

pub(crate) const JOB: &str = "sofie_notify.digest";
/// Keeps a digest a readable mail and one bounded query; the rest waits for the next digest.
const MAXIMUM_DIGESTED: u64 = 50;
const MAIL: Template = Template::new(
    "sofie_notify/digest",
    sofie_db::embed!(
        "src/mails/digest.html",
        compiled = "mails/sofie_notify/digest.sft"
    ),
);

/// The local schedule's key, so changing the digest moves the principal's one schedule.
pub(crate) fn key(principal: Uuid) -> String {
    format!("{JOB}:{principal}")
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Due {
    pub(crate) principal: Uuid,
}

impl Resolve<JobContext> for Due {
    fn resolve(context: &mut JobContext) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(crate::job_input(context, JOB))
    }
}

/// Nothing held, nothing mailed. Each mail goes to one address the items were held for,
/// the oldest's first; items held for another address follow in a run of their own.
pub(crate) async fn send_digest(due: Due, read: Read, clock: Clock, mails: Mails) -> Result<Sent> {
    let Some(mailer) = mails.0 else {
        tracing::warn!("a digest was due, but mail is not set up");
        return Ok(Sent::default());
    };
    let held = DigestItem::query()
        .where_(DIGEST_PRINCIPAL.eq(due.principal))
        .where_(DIGEST_SENT_AT.is_null())
        .order_by(DIGEST_CREATED_AT.asc())
        .limit(MAXIMUM_DIGESTED)
        .all(&read)
        .await?;
    let full = u64::try_from(held.len()).is_ok_and(|taken| taken == MAXIMUM_DIGESTED);
    let Some(oldest) = held.first() else {
        return Ok(Sent::default());
    };
    let email = oldest.email.clone();
    let (mut items, elsewhere): (Vec<_>, Vec<_>) =
        held.into_iter().partition(|item| item.email == email);
    let address = Address::new(&Email::parse(&email)?)?;
    let sent_at = clock.now().timestamp();
    let mut notices = Vec::with_capacity(items.len());
    for item in &mut items {
        let notice = &item.notice;
        // As an immediate mail skips itself, a digest leaves out only the link.
        let link = notice
            .link
            .as_deref()
            .and_then(|path| match mailer.link(path) {
                Ok(link) => Some(link),
                Err(error) => {
                    tracing::warn!(error = %error, "a digested notice cannot link back to the app; it is listed without its link");
                    None
                }
            });
        notices.push(json!({ "title": notice.title, "body": notice.body, "link": link }));
        item.sent_at = Some(sent_at);
    }
    let subject = match notices.len() {
        1 => items[0].notice.title.clone(),
        count => format!("{count} notifications"),
    };
    let body = mailer.body_from(&MAIL, &json!({ "notices": notices }))?;
    let mail = mailer.send(Mail::new(address, &subject, body)?)?;
    let follows = !elsewhere.is_empty() || (full && !digest_on(&read, &due).await?);
    let more = follows
        .then(|| Jobs::queue_named_at(JOB, &due, Instant::from(sent_at)))
        .transpose()?;
    Ok(Sent {
        items,
        mail: Some(mail),
        more,
    })
}

/// A stopped digest has no next run to take what one mail could not hold.
async fn digest_on(read: &Read, due: &Due) -> Result<bool> {
    let settings = Settings::query()
        .where_(SETTINGS_PRINCIPAL.eq(due.principal))
        .first(read)
        .await?;
    Ok(settings.is_some_and(|settings| settings.digest.is_on()))
}

#[derive(Default)]
pub(crate) struct Sent {
    items: Vec<DigestItem>,
    mail: Option<Queued>,
    more: Option<Queued>,
}

impl Outcome for Sent {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        self.items.plan(plan)?;
        self.mail.plan(plan)?;
        self.more.plan(plan)
    }

    fn committed(&mut self, stamp: Stamp) {
        self.items.committed(stamp);
    }
}
