use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::clock::Source;
use crate::{Clock, Instant, Result, unpoisoned};

#[derive(Clone, Debug)]
pub struct TestClock {
    time: Arc<Mutex<Instant>>,
}

impl TestClock {
    #[must_use]
    pub fn starting_at(start: impl Into<Instant>) -> Self {
        Self {
            time: Arc::new(Mutex::new(start.into())),
        }
    }

    #[must_use]
    pub fn clock(&self) -> Clock {
        Clock {
            source: Source::Test(Arc::clone(&self.time)),
        }
    }

    pub fn advance(&self, by: Duration) -> Result<()> {
        let mut time = unpoisoned::lock(&self.time);
        *time = time.checked_add(by)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use jiff::Timestamp;

    use super::*;

    #[test]
    fn a_test_clock_moves_only_when_advanced() {
        let start = Timestamp::from_second(1_700_000_000).unwrap();
        let test_clock = TestClock::starting_at(start);
        let clock = test_clock.clock();

        assert_eq!(clock.now().timestamp(), start);
        test_clock.advance(Duration::from_secs(90)).unwrap();
        assert_eq!(
            clock.now().timestamp(),
            start.checked_add(Duration::from_secs(90)).unwrap()
        );
    }
}
