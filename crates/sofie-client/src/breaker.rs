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
    pub(crate) fn permit(&self, host: &str, now: Instant) -> Result<(), Duration> {
        let mut circuits = self.circuits();
        let Some(circuit) = circuits.get_mut(host) else {
            return Ok(());
        };
        match circuit.open_until {
            Some(until) if now < until => Err(until - now),
            Some(_) => {
                circuit.open_until = Some(now + OPEN_FOR);
                Ok(())
            }
            None => Ok(()),
        }
    }

    pub(crate) fn admit(&self, host: &str) -> sofie_core::Result<()> {
        self.permit(host, Instant::now()).map_err(|open_for| {
            Error::Unavailable(format!(
                "{OPEN} `{host}` is open after repeated failures; it is tried again in {} s",
                open_for.as_secs()
            ))
        })
    }

    pub(crate) fn record(&self, host: &str, succeeded: bool, now: Instant) {
        let mut circuits = self.circuits();
        if succeeded {
            circuits.remove(host);
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
            breakers.record("vendor.test", false, start);
        }

        assert_eq!(breakers.permit("vendor.test", start), Err(OPEN_FOR));
        assert!(breakers.permit("other.test", start).is_ok());

        let later = start + OPEN_FOR;
        assert!(breakers.permit("vendor.test", later).is_ok(), "one probe");
        assert!(breakers.permit("vendor.test", later).is_err(), "only one");
        breakers.record("vendor.test", true, later);
        assert!(breakers.permit("vendor.test", later).is_ok());
    }

    #[test]
    fn a_failed_probe_keeps_the_circuit_open() {
        let breakers = Breakers::default();
        let start = Instant::now();
        for _ in 0..FAILURES_TO_OPEN {
            breakers.record("vendor.test", false, start);
        }
        let later = start + OPEN_FOR;

        assert!(breakers.permit("vendor.test", later).is_ok());
        breakers.record("vendor.test", false, later);

        assert!(
            breakers
                .permit("vendor.test", later + OPEN_FOR / 2)
                .is_err()
        );
    }
}
