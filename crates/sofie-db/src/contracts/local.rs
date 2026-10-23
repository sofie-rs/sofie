//! Jobs at a wall-clock time in a zone, such as 09:00 in Europe/Istanbul every day.
//! Kept as that local time, so each run is placed by the zone's rules as they are then.

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use sofie_core::variants::{self, Variants};
use sofie_core::{
    Cast, ColumnReader, ColumnSpec, Error, Result, SqlType, SqlValue, Timestamp, Uuid,
};
use sofie_values::LocalDateTime;

use super::jobs::{JobNames, Jobs, Queued};
use crate::Resolve;
use crate::worker::JobContext;
use crate::{
    Aggregate, Call, Column, Db, Field, Id, Load, MessageKind, Meta, Outcome, Plan, Reach, Select,
    Stamp, Writes,
};

const TEXT: &[ColumnSpec] = &[ColumnSpec::new("", SqlType::Text)];
const DAY: Duration = Duration::from_hours(24);
/// Fired per pass, so a backlog never holds the worker's claim for long.
const BATCH: u64 = 100;
/// A schedule that cannot fire waits this long, so it neither floods the log nor fills each batch.
const FAILED_RETRY_AFTER: Duration = Duration::from_mins(15);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Repeat {
    Once,
    Daily,
    Weekly,
}

impl Variants for Repeat {
    const TYPE: &'static str = "local schedule repeat";
    const VARIANTS: &'static [(&'static str, Self)] = &[
        ("once", Self::Once),
        ("daily", Self::Daily),
        ("weekly", Self::Weekly),
    ];
}

impl Repeat {
    fn step(self) -> Option<Duration> {
        match self {
            Self::Once => None,
            Self::Daily => Some(DAY),
            Self::Weekly => Some(DAY * 7),
        }
    }
}

pub(crate) struct LocalSchedule {
    meta: Meta,
    key: String,
    job: String,
    payload: String,
    at: LocalDateTime,
    repeat: Repeat,
    /// `None` once a single run ran, or when canceled.
    next_at: Option<Timestamp>,
    owner: Option<Uuid>,
}

const KEY: Column<LocalSchedule, String> = Column::new("key");
const NEXT_AT: Column<LocalSchedule, Timestamp> = Column::new("next_at");

/// Sets and cancels jobs at a local time, each under a key the app chooses, such as
/// `digest:<account id>`, so setting it again moves it.
pub struct LocalJobs {
    jobs: Jobs,
    db: Db,
    owner: Option<Uuid>,
}

impl<C: Writes> Resolve<C> for LocalJobs {
    fn resolve(context: &mut C) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(Ok(Self::within(context.db(), context.reach())))
    }
}

impl LocalJobs {
    /// For a package resolving its own arguments: what `reach` owns is what it writes.
    #[must_use]
    pub fn within(db: &Db, reach: Reach) -> Self {
        let names = db.provided.get::<JobNames>().unwrap_or_default();
        Self {
            jobs: Jobs(names),
            db: db.clone(),
            owner: reach.owner(),
        }
    }

    /// The first run is the next time `at` falls at or after now; a single one already past runs at once.
    pub async fn set<Args, J: Call<JobContext, Args>>(
        &self,
        key: &str,
        job: J,
        payload: &impl Serialize,
        at: LocalDateTime,
        repeat: Repeat,
    ) -> Result<LocalScheduled> {
        let Queued {
            name: job, payload, ..
        } = self.jobs.queue(job, payload)?;
        let now = self.db.clock().now().timestamp();
        let (at, next_at) = first_run(at, repeat, now)?;
        let schedule = match self.keyed(key).await? {
            Some(mut schedule) => {
                job.clone_into(&mut schedule.job);
                schedule.payload = payload.to_string();
                schedule.at = at;
                schedule.repeat = repeat;
                schedule.next_at = Some(next_at);
                schedule
            }
            None => LocalSchedule {
                meta: Meta::new()?,
                key: key.to_owned(),
                job: job.to_owned(),
                payload: payload.to_string(),
                at,
                repeat,
                next_at: Some(next_at),
                owner: self.owner,
            },
        };
        Ok(LocalScheduled(Some(schedule)))
    }

    /// Nothing changes when no schedule has the key.
    pub async fn cancel(&self, key: &str) -> Result<LocalScheduled> {
        let schedule = self.keyed(key).await?.map(|mut schedule| {
            schedule.next_at = None;
            schedule
        });
        Ok(LocalScheduled(schedule))
    }

    async fn keyed(&self, key: &str) -> Result<Option<LocalSchedule>> {
        LocalSchedule::query()
            .where_(KEY.eq(key.to_owned()))
            .first(&self.db.read())
            .await
    }
}

#[must_use = "the schedule changes only when the handler returns it"]
pub struct LocalScheduled(Option<LocalSchedule>);

impl LocalScheduled {
    #[must_use]
    pub fn next_at(&self) -> Option<Timestamp> {
        self.0.as_ref().and_then(|schedule| schedule.next_at)
    }
}

impl Outcome for LocalScheduled {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        self.0
            .as_ref()
            .map_or(Ok(()), |schedule| schedule.plan(plan))
    }

    fn committed(&mut self, stamp: Stamp) {
        if let Some(schedule) = &mut self.0 {
            schedule.committed(stamp);
        }
    }
}

/// For a repeating run, the occurrence after `now` at the earliest, counted in the zone's days.
fn first_run(
    at: LocalDateTime,
    repeat: Repeat,
    now: Timestamp,
) -> Result<(LocalDateTime, Timestamp)> {
    let instant = at.instant()?.timestamp();
    let Some(step) = repeat.step() else {
        return Ok((at, instant));
    };
    if instant >= now {
        return Ok((at, instant));
    }
    following(&at, step, now)
}

/// Skips runs a stopped worker missed rather than running each of them late.
fn following(
    at: &LocalDateTime,
    step: Duration,
    now: Timestamp,
) -> Result<(LocalDateTime, Timestamp)> {
    let behind = u64::try_from((now.as_second() - at.instant()?.timestamp().as_second()).max(0))
        .unwrap_or(0);
    let skipped = (behind / step.as_secs()).saturating_sub(1);
    let mut local = advance(at, step * u32::try_from(skipped).unwrap_or(u32::MAX))?;
    loop {
        let instant = local.instant()?.timestamp();
        if instant > now {
            return Ok((local, instant));
        }
        local = advance(&local, step)?;
    }
}

fn advance(at: &LocalDateTime, by: Duration) -> Result<LocalDateTime> {
    let local = at
        .local()
        .checked_add(by)
        .map_err(|error| Error::internal("moving a local schedule on", error))?;
    Ok(LocalDateTime::new(local, at.zone().clone()))
}

/// A worker that fired a schedule first wins the version check, so a run is queued once.
pub(crate) async fn fire_due(db: &Db, now: Timestamp) -> Result<()> {
    let due = LocalSchedule::query()
        .where_(NEXT_AT.lte(now))
        .limit(BATCH)
        .all(&db.read())
        .await?;
    for schedule in due {
        let (id, key) = (schedule.id(), schedule.key.clone());
        let Err(error) = fire(db, schedule, now).await else {
            continue;
        };
        if matches!(error, Error::Conflict) {
            continue;
        }
        tracing::error!(schedule = %key, error = %error, "firing a local schedule failed");
        if let Err(error) = put_off(db, id, now).await {
            tracing::error!(schedule = %key, error = %error, "putting off a local schedule failed");
        }
    }
    Ok(())
}

async fn put_off(db: &Db, id: Id<LocalSchedule>, now: Timestamp) -> Result<()> {
    let mut schedule = LocalSchedule::find(&db.read(), id).await?;
    schedule.next_at = Some(
        now.checked_add(FAILED_RETRY_AFTER)
            .map_err(|error| Error::internal("putting off a local schedule", error))?,
    );
    db.commit(&mut schedule).await
}

async fn fire(db: &Db, mut schedule: LocalSchedule, now: Timestamp) -> Result<()> {
    // The zone's rules may have moved the time since the run was placed.
    let placed = schedule.at.instant()?.timestamp();
    if placed > now {
        schedule.next_at = Some(placed);
        return db.commit(&mut schedule).await;
    }
    let payload: Value = serde_json::from_str(&schedule.payload)
        .map_err(|error| Error::internal("reading a local schedule's payload", error))?;
    match schedule.repeat.step() {
        None => schedule.next_at = None,
        Some(step) => {
            let (at, next_at) = following(&schedule.at, step, now)?;
            schedule.at = at;
            schedule.next_at = Some(next_at);
        }
    }
    let job: Arc<str> = Arc::from(schedule.job.as_str());
    db.commit(&mut Fired {
        schedule,
        job,
        payload,
    })
    .await
}

struct Fired {
    schedule: LocalSchedule,
    job: Arc<str>,
    payload: Value,
}

impl Outcome for Fired {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        self.schedule.plan(plan)?;
        plan.queue(MessageKind::Job, [(&*self.job, self.payload.clone())])
    }

    fn committed(&mut self, stamp: Stamp) {
        self.schedule.committed(stamp);
    }
}

impl Aggregate for LocalSchedule {
    const NAME: &'static str = "local_schedules";
    const MODULE: &'static str = "sofie";
    const FIELDS: &'static [Field] = &[
        Field::new("key", String::COLUMNS, false),
        Field::new("job", String::COLUMNS, false),
        Field::new("payload", TEXT, false),
        Field::new("at", LocalDateTime::COLUMNS, false),
        Field::new("repeat", variants::COLUMNS, false),
        Field::new("next_at", Timestamp::COLUMNS, true),
        Field::new("owner", Uuid::COLUMNS, true),
    ];
    const UNIQUE: &'static [&'static str] = &["key"];

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn owner(&self) -> Option<Uuid> {
        self.owner
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.key.encode(out);
        self.job.encode(out);
        out.push(SqlValue::Text(self.payload.clone()));
        self.at.encode(out);
        variants::encode(self.repeat, out);
        self.next_at.encode(out);
        self.owner.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            key: String::decode(row)?,
            job: String::decode(row)?,
            payload: row.text()?.to_owned(),
            at: LocalDateTime::decode(row)?,
            repeat: variants::decode(row)?,
            next_at: Option::decode(row)?,
            owner: Option::decode(row)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use sofie_values::TimeZone;

    use super::*;

    fn local(text: &str) -> LocalDateTime {
        LocalDateTime::parse(text, TimeZone::parse("Europe/Berlin").unwrap()).unwrap()
    }

    fn at(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    #[test]
    fn a_daily_run_keeps_its_wall_clock_time_across_a_shift() {
        let nine = local("2026-10-24T09:00");

        let (next, instant) = following(&nine, DAY, at("2026-10-24T08:00:00Z")).unwrap();

        assert_eq!(next.local().to_string(), "2026-10-25T09:00:00");
        assert_eq!(instant, at("2026-10-25T08:00:00Z"));
    }

    #[test]
    fn a_skipped_time_runs_where_the_shift_ends() {
        let half_past_two = local("2026-03-28T02:30");

        let (next, instant) = following(&half_past_two, DAY, at("2026-03-28T12:00:00Z")).unwrap();

        assert_eq!(next.local().to_string(), "2026-03-29T02:30:00");
        assert_eq!(instant, at("2026-03-29T01:00:00Z"));
    }

    #[test]
    fn a_repeating_run_set_in_the_past_starts_at_its_next_time_and_a_single_one_at_once() {
        let now = at("2026-10-14T12:00:00Z");
        let long_ago = local("2025-01-06T09:00");

        let (weekly, _) = first_run(long_ago.clone(), Repeat::Weekly, now).unwrap();
        let (once, instant) = first_run(long_ago, Repeat::Once, now).unwrap();

        assert_eq!(weekly.local().to_string(), "2026-10-19T09:00:00");
        assert_eq!(once.local().to_string(), "2025-01-06T09:00:00");
        assert!(instant < now);
    }
}
