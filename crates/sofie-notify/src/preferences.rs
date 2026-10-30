use jiff::civil::Weekday;
use jiff::tz;
use serde::Serialize;
use sofie_core::{Error, Instant, Result, SqlValue, Timestamp, Uuid};
use sofie_db::{
    Aggregate, Db, Jobs, LocalJobs, LocalScheduled, Outcome, Plan, Queued, Reach, Read, Repeat,
    Select, Stamp, Step,
};
use sofie_http::{Area, Principal, RequestContext};
use sofie_values::{LocalDateTime, TimeZone};

use crate::digest::{self, Due};
use crate::store::{CHOICE_PRINCIPAL, Choice, SETTINGS_PRINCIPAL, Settings};
use crate::{Channel, TimeOfDay, Topic};
use sofie_db::{Context, Resolve};

/// How a principal reads mail: each at once, or held for one mail a day or a week.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Digest {
    Off,
    Daily(TimeOfDay),
    Weekly(Weekday, TimeOfDay),
}

impl Digest {
    pub(crate) fn to_stored(self) -> String {
        match self {
            Self::Off => "off".to_owned(),
            Self::Daily(at) => format!("daily {at}"),
            Self::Weekly(day, at) => format!("weekly {} {at}", weekday_name(day)),
        }
    }

    pub(crate) fn from_stored(text: &str) -> Result<Self> {
        let malformed = || Error::internal_message(format!("a stored digest reads `{text}`"));
        let parts: Vec<&str> = text.split(' ').collect();
        Ok(match parts.as_slice() {
            ["off"] => Self::Off,
            ["daily", at] => Self::Daily(TimeOfDay::parse(at).map_err(|_| malformed())?),
            ["weekly", day, at] => Self::Weekly(
                weekday(day).ok_or_else(malformed)?,
                TimeOfDay::parse(at).map_err(|_| malformed())?,
            ),
            _ => return Err(malformed()),
        })
    }

    pub(crate) fn is_on(self) -> bool {
        self != Self::Off
    }
}

const WEEKDAYS: [(Weekday, &str); 7] = [
    (Weekday::Monday, "monday"),
    (Weekday::Tuesday, "tuesday"),
    (Weekday::Wednesday, "wednesday"),
    (Weekday::Thursday, "thursday"),
    (Weekday::Friday, "friday"),
    (Weekday::Saturday, "saturday"),
    (Weekday::Sunday, "sunday"),
];

fn weekday_name(day: Weekday) -> &'static str {
    WEEKDAYS[usize::from(day.to_monday_zero_offset().unsigned_abs())].1
}

fn weekday(name: &str) -> Option<Weekday> {
    WEEKDAYS
        .iter()
        .find(|(_, known)| *known == name)
        .map(|(day, _)| *day)
}

pub struct Preferences {
    principal: Uuid,
    db: Db,
    reach: Reach,
}

impl<A: Area, M: Send> Resolve<RequestContext<A, M>> for Preferences {
    async fn resolve(context: &mut RequestContext<A, M>) -> Result<Self> {
        let principal = Principal::<A>::resolve(context).await?;
        Ok(Self {
            principal: principal.id().uuid(),
            db: context.db().clone(),
            reach: context.reach(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Chosen {
    pub zone: Option<String>,
    pub quiet_from: Option<TimeOfDay>,
    pub quiet_until: Option<TimeOfDay>,
    pub digest: String,
    pub channels: Vec<ChannelChoice>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ChannelChoice {
    pub topic: String,
    pub channel: Channel,
    pub enabled: bool,
}

impl Preferences {
    pub async fn chosen(&self) -> Result<Chosen> {
        let settings = self.settings().await?;
        let choices = Choice::query()
            .where_(CHOICE_PRINCIPAL.eq(self.principal))
            .all(&self.read())
            .await?;
        Ok(Chosen {
            zone: settings.zone.map(|zone| zone.name().to_owned()),
            quiet_from: settings.quiet_from,
            quiet_until: settings.quiet_until,
            digest: settings.digest.to_stored(),
            channels: choices
                .into_iter()
                .map(|choice| ChannelChoice {
                    topic: choice.topic,
                    channel: choice.channel,
                    enabled: choice.enabled,
                })
                .collect(),
        })
    }

    /// An upsert, so two first choices at once both hold, the later one winning.
    pub fn choose(&self, topic: Topic, channel: Channel, enabled: bool) -> Result<Changed> {
        Ok(Changed {
            choice: Some(Chose {
                id: Uuid::now_v7()?,
                principal: self.principal,
                topic: topic.name(),
                channel,
                enabled,
            }),
            ..Changed::default()
        })
    }

    /// Mail and push wait for `until`; the inbox fills meanwhile. `None` ends quiet hours.
    pub async fn quiet_hours(
        &self,
        hours: Option<(TimeOfDay, TimeOfDay)>,
        zone: TimeZone,
    ) -> Result<Changed> {
        let mut settings = self.settings().await?;
        settings.quiet_from = hours.map(|(from, _)| from);
        settings.quiet_until = hours.map(|(_, until)| until);
        settings.zone = Some(zone);
        Ok(Changed {
            settings: Some(settings),
            ..Changed::default()
        })
    }

    pub async fn digest(&self, digest: Digest, zone: TimeZone) -> Result<Changed> {
        let mut settings = self.settings().await?;
        let held = settings.digest.is_on() && !digest.is_on();
        settings.digest = digest;
        settings.zone = Some(zone.clone());
        let jobs = LocalJobs::within(&self.db, self.reach);
        let key = digest::key(self.principal);
        let now = self.db.clock().now().timestamp();
        let due = Due {
            principal: self.principal,
        };
        let schedule = match digest {
            Digest::Off => None,
            Digest::Daily(at) => Some((None, at, Repeat::Daily)),
            Digest::Weekly(day, at) => Some((Some(day), at, Repeat::Weekly)),
        };
        let scheduled = match schedule {
            None => jobs.cancel(&key).await?,
            Some((day, at, repeat)) => {
                let first = first_on(now, &zone, day, at)?;
                jobs.set(&key, digest::send_digest, &due, first, repeat)
                    .await?
            }
        };
        // What the stopped digest held is mailed now rather than never.
        let last = held
            .then(|| Jobs::queue_named_at(digest::JOB, &due, Instant::from(now)))
            .transpose()?;
        Ok(Changed {
            settings: Some(settings),
            scheduled: Some(scheduled),
            last,
            ..Changed::default()
        })
    }

    fn read(&self) -> Read {
        Read::within(&self.db, self.reach)
    }

    async fn settings(&self) -> Result<Settings> {
        let stored = Settings::query()
            .where_(SETTINGS_PRINCIPAL.eq(self.principal))
            .first(&self.read())
            .await?;
        stored.map_or_else(|| Settings::new(self.principal), Ok)
    }
}

/// Today in `zone` at `at`, or the next `day`; one already past moves on by itself.
fn first_on(
    now: Timestamp,
    zone: &TimeZone,
    day: Option<Weekday>,
    at: TimeOfDay,
) -> Result<LocalDateTime> {
    let today = now.to_zoned(jiff_zone(zone)?).date();
    let ahead = day.map_or(0, |day| {
        (day.to_monday_zero_offset() - today.weekday().to_monday_zero_offset()).rem_euclid(7)
    });
    let date = today
        .checked_add(jiff::Span::new().days(i64::from(ahead)))
        .map_err(|error| Error::internal("finding the digest's first day", error))?;
    Ok(LocalDateTime::new(
        date.to_datetime(at.civil()),
        zone.clone(),
    ))
}

pub(crate) fn jiff_zone(zone: &TimeZone) -> Result<tz::TimeZone> {
    tz::TimeZone::get(zone.name())
        .map_err(|error| Error::internal(format!("reading time zone {}", zone.name()), error))
}

#[derive(Default)]
#[must_use = "the preferences change only when the handler returns it"]
pub struct Changed {
    settings: Option<Settings>,
    choice: Option<Chose>,
    scheduled: Option<LocalScheduled>,
    last: Option<Queued>,
}

impl Outcome for Changed {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        self.settings.plan(plan)?;
        self.choice.plan(plan)?;
        self.scheduled.plan(plan)?;
        self.last.plan(plan)
    }

    fn committed(&mut self, stamp: Stamp) {
        self.settings.committed(stamp);
        self.scheduled.committed(stamp);
    }
}

struct Chose {
    id: Uuid,
    principal: Uuid,
    topic: &'static str,
    channel: Channel,
    enabled: bool,
}

impl Outcome for Chose {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        let dialect = plan.dialect();
        let at = |index| dialect.placeholder(index);
        let sql = format!(
            "INSERT INTO {} AS held (id, version, principal, topic, channel, enabled) \
             VALUES ({}, 1, {}, {}, {}, {}) \
             ON CONFLICT (principal, topic, channel) DO UPDATE \
             SET enabled = EXCLUDED.enabled, version = held.version + 1",
            dialect.qualified(Choice::MODULE, Choice::NAME),
            at(1),
            at(2),
            at(3),
            at(4),
            at(5),
        );
        let params = vec![
            SqlValue::Uuid(self.id),
            SqlValue::Uuid(self.principal),
            SqlValue::Text(self.topic.to_owned()),
            SqlValue::Text(self.channel.as_str().to_owned()),
            SqlValue::Bool(self.enabled),
        ];
        plan.push_as::<Choice>(Step::statement(sql, params, Some(1)));
        plan.touched::<Choice>(Some(self.principal));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_digest_is_stored_as_words() {
        let at = TimeOfDay::parse("08:30").unwrap();
        for digest in [
            Digest::Off,
            Digest::Daily(at),
            Digest::Weekly(Weekday::Friday, at),
        ] {
            assert_eq!(Digest::from_stored(&digest.to_stored()).unwrap(), digest);
        }
        assert_eq!(
            Digest::Weekly(Weekday::Friday, at).to_stored(),
            "weekly friday 08:30"
        );
    }

    #[test]
    fn a_weekly_digest_starts_on_the_next_such_day_in_the_zone() {
        let zone = TimeZone::parse("Asia/Tokyo").unwrap();
        // Wednesday 23:30 in UTC is already Thursday in Tokyo.
        let now: Timestamp = "2026-10-14T23:30:00Z".parse().unwrap();
        let at = TimeOfDay::parse("09:00").unwrap();

        let friday = first_on(now, &zone, Some(Weekday::Friday), at).unwrap();
        let thursday = first_on(now, &zone, Some(Weekday::Thursday), at).unwrap();

        assert_eq!(friday.local().to_string(), "2026-10-16T09:00:00");
        assert_eq!(thursday.local().to_string(), "2026-10-15T09:00:00");
    }
}
