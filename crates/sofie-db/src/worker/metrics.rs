use sofie_core::Error;
use sofie_telemetry::metrics::{self, Instrument, Kind};

use super::trace::Work;
use super::{Registry, Worker};
use crate::Claimed;

static LAG: Instrument = Instrument {
    name: "messaging.process.lag",
    description: "How long a message waited past its due time before a worker took it.",
    kind: Kind::Duration,
    labels: &["messaging.work"],
};
static OUTCOMES: Instrument = Instrument {
    name: "messaging.process.outcomes",
    description: "Messages handled, by work, registered name and outcome.",
    kind: Kind::Counter,
    labels: &["messaging.work", "messaging.name", "outcome"],
};
const UNREGISTERED: &str = "unregistered";

pub(super) fn measure(
    worker: &Worker,
    message: &Claimed,
    work: Work,
    name: &str,
    failure: Option<&Error>,
) {
    let work = work.label();
    let waited = worker
        .db
        .clock()
        .now()
        .timestamp()
        .duration_since(message.available_at);
    if let Ok(waited) = waited.try_into() {
        metrics::record_duration(&LAG, &[work.into()], waited);
    }
    let outcome = match failure {
        None => "completed",
        Some(_) if message.attempt >= worker.max_attempts => "dead",
        Some(_) => "retried",
    };
    let name = worker
        .registry
        .registered_name(name)
        .unwrap_or(UNREGISTERED);
    metrics::record(&OUTCOMES, &[work.into(), name.into(), outcome.into()], 1.0);
}

impl Registry {
    fn registered_name(&self, name: &str) -> Option<&'static str> {
        let job = self.job(name).map(|entry| entry.name);
        job.or_else(|| {
            self.listeners
                .by_event
                .get_key_value(name)
                .map(|(name, _)| *name)
        })
    }
}
