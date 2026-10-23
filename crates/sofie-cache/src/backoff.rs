use std::time::Duration;

const FIRST: Duration = Duration::from_millis(100);
const LONGEST: Duration = Duration::from_secs(10);

/// Doubles the wait after each failure in a row, so an unreachable server is not hammered.
pub(crate) struct Backoff {
    delay: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self { delay: FIRST }
    }
}

impl Backoff {
    pub(crate) fn failed(&mut self) -> Duration {
        let wait = self.delay;
        self.delay = (self.delay * 2).min(LONGEST);
        wait
    }

    pub(crate) fn succeeded(&mut self) {
        self.delay = FIRST;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_failure_doubles_the_wait_up_to_the_longest() {
        let mut backoff = Backoff::default();
        let waits: Vec<Duration> = (0..9).map(|_| backoff.failed()).collect();

        assert_eq!(waits[0], FIRST);
        assert_eq!(waits[1], FIRST * 2);
        assert_eq!(waits[8], LONGEST);
        backoff.succeeded();
        assert_eq!(backoff.failed(), FIRST);
    }
}
