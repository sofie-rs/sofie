use std::sync::Mutex;
use std::time::Duration;

use sofie_core::{Error, Result, Timestamp, unpoisoned};

use super::Worker;
use crate::Messages;

/// Queues the schedule `name`'s job of `worker` now, for a worker running beside this process.
pub async fn queue_schedule(worker: &Worker, name: &str) -> Result<()> {
    let schedules = &worker.registry.schedules;
    if !schedules.names().any(|known| known == name) {
        let mut known: Vec<&str> = schedules.names().collect();
        known.sort_unstable();
        let known = if known.is_empty() {
            "none".to_owned()
        } else {
            known.join(", ")
        };
        return Err(Error::internal_message(format!(
            "the app has no schedule `{name}`; its schedules: {known}"
        )));
    }
    worker.messages.queue_schedule(name).await
}

#[cfg(feature = "local-time")]
use super::POLL_INTERVAL;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cadence {
    Interval(Duration),
    #[cfg(feature = "local-time")]
    At(super::WallClock),
}

impl From<Duration> for Cadence {
    fn from(interval: Duration) -> Self {
        Self::Interval(interval)
    }
}

#[cfg(feature = "local-time")]
impl From<super::WallClock> for Cadence {
    fn from(wall_clock: super::WallClock) -> Self {
        Self::At(wall_clock)
    }
}

impl Cadence {
    /// An interval schedule first runs as soon as it is new; a wall-clock one at its next time.
    #[cfg_attr(
        not(feature = "local-time"),
        expect(clippy::unnecessary_wraps, reason = "a wall-clock schedule can fail")
    )]
    fn first_at(self, now: Timestamp) -> Result<Timestamp> {
        match self {
            Self::Interval(_) => Ok(now),
            #[cfg(feature = "local-time")]
            Self::At(wall_clock) => wall_clock.next_after(now),
        }
    }

    fn unknown_zone(self) -> Option<Error> {
        match self {
            Self::Interval(_) => None,
            #[cfg(feature = "local-time")]
            Self::At(wall_clock) => wall_clock.zone().err(),
        }
    }

    /// Stored beside the schedule: a different key starts the schedule over.
    fn key(self) -> String {
        match self {
            Self::Interval(interval) => format!("every {}ms", interval.as_millis()),
            #[cfg(feature = "local-time")]
            Self::At(wall_clock) => wall_clock.key(),
        }
    }

    fn next_after(self, now: Timestamp) -> Result<Timestamp> {
        match self {
            Self::Interval(interval) => now
                .checked_add(interval)
                .map_err(|error| Error::internal("computing a schedule's next run", error)),
            #[cfg(feature = "local-time")]
            Self::At(wall_clock) => wall_clock.next_after(now),
        }
    }
}

#[derive(Default)]
pub(super) struct Schedules {
    every: Vec<(&'static str, Cadence, Mutex<Option<Timestamp>>)>,
}

impl Schedules {
    pub(super) fn add(&mut self, name: &'static str, cadence: Cadence) {
        self.every.push((name, cadence, Mutex::new(None)));
    }

    pub(super) fn names(&self) -> impl Iterator<Item = &'static str> {
        self.every.iter().map(|(name, ..)| *name)
    }

    pub(super) fn unknown_zones(&self) -> impl Iterator<Item = String> + '_ {
        self.every.iter().filter_map(|(name, cadence, _)| {
            let error = cadence.unknown_zone()?;
            Some(format!("schedule `{name}` cannot run: {error}"))
        })
    }

    pub(super) async fn store(&self, messages: &Messages, now: Timestamp) -> Result<()> {
        for (name, cadence, _) in &self.every {
            let first_at = cadence.first_at(now)?;
            // Keys were the Debug text before; a schedule still holding it keeps its time.
            let previous_key = format!("{cadence:?}");
            messages
                .add_schedule(name, first_at, &cadence.key(), &previous_key)
                .await?;
        }
        Ok(())
    }

    /// Another worker may have fired it since; the answer says when it is due next.
    pub(super) async fn fire_due(&self, messages: &Messages, now: Timestamp) -> Result<()> {
        for (name, cadence, next_due) in &self.every {
            let next_due = || unpoisoned::lock(next_due);
            if next_due().is_some_and(|due| due > now) {
                continue;
            }
            let fired = messages
                .fire_schedule(name, now, cadence.next_after(now)?)
                .await?;
            *next_due() = fired.map(|fired| fired.next_at);
        }
        Ok(())
    }
}

/// Any process may add a local schedule, so no due time is known; a running worker looks for
/// them at most once per poll interval instead of before every claim.
#[cfg(feature = "local-time")]
#[derive(Default)]
pub(super) struct LocalLooks(Mutex<Option<Timestamp>>);

#[cfg(feature = "local-time")]
impl LocalLooks {
    /// A clock moved back looks again at once.
    pub(super) fn due(&self, now: Timestamp) -> bool {
        let mut last = unpoisoned::lock(&self.0);
        let due = last.is_none_or(|last| {
            now < last
                || last
                    .checked_add(POLL_INTERVAL)
                    .is_ok_and(|next| now >= next)
        });
        if due {
            *last = Some(now);
        }
        due
    }
}

#[cfg(all(test, feature = "local-time"))]
mod tests {
    use super::*;

    fn at(milliseconds: i64) -> Timestamp {
        Timestamp::from_millisecond(milliseconds).unwrap()
    }

    #[test]
    fn local_schedules_are_looked_for_at_most_once_per_poll_interval() {
        let looks = LocalLooks::default();

        let seen: Vec<bool> = [0, 10, 999, 1_000, 1_500, 2_000, 1_200]
            .into_iter()
            .map(|milliseconds| looks.due(at(milliseconds)))
            .collect();

        assert_eq!(seen, [true, false, false, true, false, true, true]);
    }
}
