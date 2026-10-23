use std::collections::HashMap;
use std::sync::Arc;

use sofie_core::registrations::{INTERVALS_ALLOWED, Key, Problem, validate};
use sofie_core::{Error, Result};

use super::schedules::Schedules;
use super::trace::Work;
use crate::heard::{Known, Listeners};
use crate::worker::Cadence;
use crate::worker::{JobContext, Worker};
use crate::{Claimed, Entry, MessageKind, Trigger};

fn describe(trigger: Trigger) -> String {
    match trigger {
        Trigger::On(event) => format!("a listener of `{event}`"),
        Trigger::Every(_) => "a schedule".to_owned(),
        Trigger::Queued => "a job".to_owned(),
    }
}

fn key(entry: &Entry<JobContext>) -> Key<'static> {
    let every = match entry.trigger {
        Trigger::Every(Cadence::Interval(interval)) => Some(interval),
        _ => None,
    };
    Key::Job {
        name: entry.name,
        every,
    }
}

pub fn publish_heard(worker: &Worker) {
    worker.db.arrivals().heard().add(&worker.registry.known());
}

#[derive(Default)]
pub(crate) struct Registry {
    entries: Vec<Entry<JobContext>>,
    by_name: HashMap<&'static str, usize>,
    pub(super) listeners: Arc<Listeners>,
    pub(super) schedules: Schedules,
}

impl Registry {
    pub(super) fn known(&self) -> Known {
        Known {
            listeners: (*self.listeners).clone(),
            jobs: self.by_name.keys().copied().collect(),
        }
    }

    pub(crate) fn add(&mut self, entry: Entry<JobContext>) {
        match entry.trigger {
            Trigger::On(event) => {
                Arc::make_mut(&mut self.listeners)
                    .by_event
                    .entry(event)
                    .or_default()
                    .push(entry.name);
            }
            Trigger::Every(cadence) => self.schedules.add(entry.name, cadence),
            Trigger::Queued => {}
        }
        self.by_name.insert(entry.name, self.entries.len());
        self.entries.push(entry);
    }

    #[cfg(feature = "contracts")]
    pub(super) fn add_taking_pruned(&mut self, entry: Entry<JobContext>) {
        Arc::make_mut(&mut self.listeners)
            .taking_pruned
            .insert(entry.name);
        self.add(entry);
    }

    /// Before any work, so a worker registered wrongly stops at boot.
    pub(crate) fn validate(&self) -> Result<()> {
        let keys: Vec<Key> = self.entries.iter().map(key).collect();
        let problems: Vec<String> = validate(&keys)
            .into_iter()
            .map(|problem| self.describe(problem))
            .chain(self.schedules.unknown_zones())
            .collect();
        if problems.is_empty() {
            return Ok(());
        }
        Err(Error::internal_message(problems.join("; ")))
    }

    fn describe(&self, problem: Problem) -> String {
        match problem {
            Problem::SameJobName(first, second) => format!(
                "job `{}` is registered twice: as {} and as {}",
                self.entries[first].name,
                describe(self.entries[first].trigger),
                describe(self.entries[second].trigger)
            ),
            Problem::Interval(index) => {
                let Entry { name, trigger, .. } = &self.entries[index];
                let Trigger::Every(Cadence::Interval(interval)) = trigger else {
                    unreachable!("only a schedule has an interval")
                };
                format!("schedule `{name}` runs every {interval:?}; {INTERVALS_ALLOWED}")
            }
            Problem::SamePath(..) | Problem::SameRouteName(..) => {
                unreachable!("a worker validates no routes")
            }
        }
    }

    pub(crate) fn job(&self, name: &str) -> Option<&Entry<JobContext>> {
        self.by_name.get(name).map(|index| &self.entries[*index])
    }

    /// The process's lone listener when this worker has it: it commits with the event, no job row.
    pub(super) fn only_listener(
        &self,
        message: &Claimed,
        heard: &Listeners,
    ) -> Option<&'static str> {
        let taking = heard.taking(&message.name, &message.payload);
        match (message.kind, taking.as_ref()) {
            (MessageKind::Event, [only]) if self.job(only).is_some() => Some(only),
            _ => None,
        }
    }

    pub(super) fn work_of(&self, message: &Claimed, only_listener: Option<&str>) -> Work {
        match message.kind {
            MessageKind::Event if only_listener.is_some() => Work::Listener,
            MessageKind::Event => Work::Event,
            MessageKind::Job => match self.job(&message.name) {
                Some(job) if matches!(job.trigger, Trigger::On(_)) => Work::Listener,
                _ => Work::Job,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use sofie_core::Result;

    use super::*;
    use crate::erase;

    async fn tidy() -> Result<()> {
        Ok(())
    }

    fn entry(name: &'static str, trigger: Trigger) -> Entry<JobContext> {
        Entry {
            name,
            trigger,
            run: erase(tidy),
        }
    }

    fn refusal(entries: Vec<Entry<JobContext>>) -> String {
        let mut registry = Registry::default();
        for entry in entries {
            registry.add(entry);
        }
        registry
            .validate()
            .expect_err("the registration is refused")
            .to_string()
    }

    #[test]
    fn a_second_job_of_one_name_is_refused() {
        let refusal = refusal(vec![
            entry("tidy", Trigger::Every(Duration::from_secs(60).into())),
            entry("tidy", Trigger::Queued),
        ]);
        assert!(
            refusal.contains("job `tidy` is registered twice: as a schedule and as a job"),
            "{refusal}"
        );
    }

    #[test]
    fn a_listener_and_a_schedule_cannot_share_a_name() {
        let refusal = refusal(vec![
            entry("answer", Trigger::On("test.Ping")),
            entry("answer", Trigger::Every(Duration::from_secs(60).into())),
        ]);
        assert!(
            refusal.contains(
                "job `answer` is registered twice: as a listener of `test.Ping` and as a schedule"
            ),
            "{refusal}"
        );
    }

    #[test]
    fn a_schedule_more_often_than_once_a_second_is_refused() {
        let refusal = refusal(vec![entry(
            "eager",
            Trigger::Every(Duration::from_millis(10).into()),
        )]);
        assert!(
            refusal.contains("schedule `eager` runs every 10ms; a schedule runs at most once"),
            "{refusal}"
        );
    }

    #[cfg(feature = "local-time")]
    #[test]
    fn a_schedule_in_an_unknown_zone_is_refused_at_start() {
        let at = sofie_values::TimeOfDay::constant("03:00");
        let refusal = refusal(vec![entry(
            "nightly",
            Trigger::Every(crate::worker::WallClock::daily(at, "Mars/Olympus").into()),
        )]);
        assert!(
            refusal
                .contains("schedule `nightly` cannot run: `Mars/Olympus` is not an IANA time zone"),
            "{refusal}"
        );
    }

    #[test]
    fn distinct_jobs_are_accepted() {
        let mut registry = Registry::default();
        registry.add(entry(
            "tidy",
            Trigger::Every(Duration::from_secs(60).into()),
        ));
        registry.add(entry("answer", Trigger::On("test.Ping")));
        assert!(registry.validate().is_ok());
        assert_eq!(registry.job("answer").map(|job| job.name), Some("answer"));
    }
}
