#[cfg(feature = "testing")]
use std::sync::{Arc, Mutex};

use jiff::Timestamp;

use crate::Instant;

#[cfg(feature = "development")]
mod shifted;

/// One source of time, so a test moves it for handlers, unit of work and worker at once.
#[derive(Clone, Debug, Default)]
pub struct Clock {
    pub(crate) source: Source,
}

#[derive(Clone, Debug, Default)]
pub(crate) enum Source {
    #[default]
    System,
    #[cfg(feature = "testing")]
    Test(Arc<Mutex<Instant>>),
    #[cfg(feature = "development")]
    Shifted(jiff::SignedDuration),
}

impl Clock {
    #[must_use]
    pub fn system() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn now(&self) -> Instant {
        match &self.source {
            Source::System => Instant::from(Timestamp::now()),
            #[cfg(feature = "development")]
            Source::Shifted(offset) => shifted::now(*offset),
            #[cfg(feature = "testing")]
            Source::Test(time) => *crate::unpoisoned::lock(time),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_clock_follows_real_time() {
        let before = Timestamp::now();

        assert!(Clock::system().now().timestamp() >= before);
    }
}
