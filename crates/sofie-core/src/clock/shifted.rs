use jiff::{SignedDuration, Timestamp};

use super::{Clock, Source};
use crate::Instant;

impl Clock {
    #[must_use]
    pub fn starting_at(start: Instant) -> Self {
        Self {
            source: Source::Shifted(start.timestamp().duration_since(Timestamp::now())),
        }
    }

    /// Runs on from `seconds` past this clock's now; a test clock's shifted copy no longer stands still.
    #[must_use]
    pub fn shifted(&self, seconds: i64) -> Self {
        let offset = self.now().timestamp().duration_since(Timestamp::now());
        Self {
            source: Source::Shifted(offset.saturating_add(SignedDuration::from_secs(seconds))),
        }
    }
}

/// Saturating by a `SignedDuration` cannot fail; only calendar units could.
pub(super) fn now(offset: SignedDuration) -> Instant {
    Instant::from(
        Timestamp::now()
            .saturating_add(offset)
            .unwrap_or(Timestamp::MAX),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shifted_clock_starts_at_its_moment_and_runs_on() {
        let start = Instant::parse("2027-01-01T09:00").unwrap();

        let clock = Clock::starting_at(start);

        let now = clock.now().timestamp();
        assert!(now >= start.timestamp(), "{now}");
        assert!(now.duration_since(start.timestamp()) < SignedDuration::from_secs(5));
    }

    #[test]
    fn a_shifted_clock_runs_the_given_seconds_ahead_of_its_source() {
        let start = Instant::parse("2027-01-01T09:00").unwrap();

        let clock = Clock::starting_at(start).shifted(-3600);

        let behind = start.timestamp().duration_since(clock.now().timestamp());
        assert!(behind > SignedDuration::from_mins(59), "{behind}");
        assert!(behind <= SignedDuration::from_mins(60), "{behind}");
    }
}
