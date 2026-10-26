use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sofie_core::{Error, unpoisoned};

const FAILURES_TO_OPEN: u32 = 5;
const OPEN_FOR: Duration = Duration::from_secs(30);
/// Hosts tracked at most: untrusted URLs name hosts without end.
const MAX_TRACKED: usize = 1024;

const OPEN: &str = "the circuit to";

/// `Error` has no kind for it, so its message tells a refusal by an open circuit apart: one is
/// not a new failure of whatever waited on it.
pub(crate) fn refused_open(error: &Error) -> bool {
    matches!(error, Error::Unavailable(message) if message.starts_with(OPEN))
}

/// How a call got through, which decides whether its success may close an open circuit: one
/// let through before the circuit opened can succeed after, and is no probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Passage {
    Closed,
    Probe,
}

#[derive(Default)]
pub(crate) struct Breakers {
    circuits: Mutex<HashMap<String, Circuit>>,
}

struct Circuit {
    failures: u32,
    open_until: Option<Instant>,
}

impl Breakers {
    /// Only the first caller probes; the rest wait another period, so an abandoned probe cannot
    /// hold it half open.
    pub(crate) fn permit(&self, host: &str, now: Instant) -> Result<Passage, Duration> {
        let mut circuits = self.circuits();
        let Some(circuit) = circuits.get_mut(host) else {
            return Ok(Passage::Closed);
        };
        match circuit.open_until {
            Some(until) if now < until => Err(until - now),
            Some(_) => {
                circuit.open_until = Some(now + OPEN_FOR);
                Ok(Passage::Probe)
            }
            None => Ok(Passage::Closed),
        }
    }

    pub(crate) fn admit(&self, host: &str) -> sofie_core::Result<Passage> {
        self.permit(host, Instant::now()).map_err(|open_for| {
            Error::Unavailable(format!(
                "{OPEN} `{host}` is open after repeated failures; it is tried again in {} s",
                open_for.as_secs()
            ))
        })
    }

    pub(crate) fn record(&self, host: &str, passage: Passage, succeeded: bool, now: Instant) {
        let mut circuits = self.circuits();
        if succeeded {
            let is_open = circuits
                .get(host)
                .is_some_and(|circuit| circuit.open_until.is_some());
            if passage == Passage::Probe || !is_open {
                circuits.remove(host);
            }
            return;
        }
        if circuits.len() >= MAX_TRACKED && !circuits.contains_key(host) {
            circuits.retain(|_, circuit| circuit.open_until.is_some());
            if circuits.len() >= MAX_TRACKED {
                return;
            }
        }
        let circuit = circuits.entry(host.to_owned()).or_insert(Circuit {
            failures: 0,
            open_until: None,
        });
        circuit.failures += 1;
        if circuit.failures >= FAILURES_TO_OPEN && circuit.open_until.is_none() {
            circuit.open_until = Some(now + OPEN_FOR);
        }
    }

    fn circuits(&self) -> std::sync::MutexGuard<'_, HashMap<String, Circuit>> {
        unpoisoned::lock(&self.circuits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_failures_open_the_circuit_until_a_probe_succeeds() {
        let breakers = Breakers::default();
        let start = Instant::now();
        for _ in 0..FAILURES_TO_OPEN {
            assert!(breakers.permit("vendor.test", start).is_ok());
            breakers.record("vendor.test", Passage::Closed, false, start);
        }

        assert_eq!(breakers.permit("vendor.test", start), Err(OPEN_FOR));
        assert!(breakers.permit("other.test", start).is_ok());

        let later = start + OPEN_FOR;
        assert_eq!(
            breakers.permit("vendor.test", later),
            Ok(Passage::Probe),
            "one probe"
        );
        assert!(breakers.permit("vendor.test", later).is_err(), "only one");
        breakers.record("vendor.test", Passage::Probe, true, later);
        assert!(breakers.permit("vendor.test", later).is_ok());
    }

    #[test]
    fn a_failed_probe_keeps_the_circuit_open() {
        let breakers = Breakers::default();
        let start = Instant::now();
        for _ in 0..FAILURES_TO_OPEN {
            breakers.record("vendor.test", Passage::Closed, false, start);
        }
        let later = start + OPEN_FOR;

        assert!(breakers.permit("vendor.test", later).is_ok());
        breakers.record("vendor.test", Passage::Probe, false, later);

        assert!(
            breakers
                .permit("vendor.test", later + OPEN_FOR / 2)
                .is_err()
        );
    }

    #[test]
    fn a_success_let_through_before_the_circuit_opened_leaves_it_open() {
        let breakers = Breakers::default();
        let start = Instant::now();
        let stale = breakers.permit("vendor.test", start).unwrap();
        for _ in 0..FAILURES_TO_OPEN {
            breakers.record("vendor.test", Passage::Closed, false, start);
        }

        breakers.record("vendor.test", stale, true, start);

        assert_eq!(breakers.permit("vendor.test", start), Err(OPEN_FOR));
    }
}
