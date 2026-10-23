use std::time::Duration;

use sofie_core::{Error, Result};

use super::Worker;
use super::trace;
use crate::{Claimed, MessageKind};

const CAP: Duration = Duration::from_hours(1);
/// `2^12` seconds already exceeds the cap.
const MAX_EXPONENT: i64 = 12;

fn backoff(attempt: i64) -> Duration {
    let exponent = (attempt - 1).clamp(0, MAX_EXPONENT);
    Duration::from_secs(1 << exponent).min(CAP)
}

/// The longest from a message's first claim to its last attempt's end.
#[cfg(feature = "contracts")]
pub(super) fn retried_for(max_attempts: i64) -> Duration {
    let delays: Duration = (1..max_attempts).map(backoff).sum();
    delays + super::LEASE * u32::try_from(max_attempts).unwrap_or(u32::MAX)
}

/// A retry can outlast a fault or a race; it cannot make a refused or missing record right.
fn may_pass_on_retry(error: &Error) -> bool {
    matches!(
        error,
        Error::Internal { .. }
            | Error::Conflict
            | Error::Unavailable(_)
            | Error::TooManyRequests(_)
    )
}

/// A dead message's row stays for inspection.
pub(super) async fn settle(
    worker: &Worker,
    message: &Claimed,
    listener: Option<&str>,
    outcome: Result<()>,
) {
    let Err(error) = outcome else { return };
    let messages = &worker.messages;
    let unknown_job_retry_would_wait_an_hour =
        message.kind == MessageKind::Job && worker.registry.job(&message.name).is_none();
    let dead = message.attempt >= worker.max_attempts
        || !may_pass_on_retry(&error)
        || unknown_job_retry_would_wait_an_hour;
    trace::failed(message, &error, dead);
    let released = if dead {
        messages.bury(message, listener, &error.to_string()).await
    } else {
        let delay = backoff(message.attempt);
        messages
            .retry(message, listener, &error.to_string(), delay)
            .await
    };
    if let Err(release_error) = released {
        tracing::warn!(
            error.kind = "release",
            error = %release_error,
            "releasing a failed message failed; its lease runs out instead"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_failure_a_retry_can_change_is_retried() {
        let retried = [
            Error::internal_message("connection reset"),
            Error::Conflict,
            Error::Unavailable("no connection came free".to_owned()),
        ];
        let buried = [
            Error::NotFound,
            Error::BadRequest("malformed".to_owned()),
            Error::invalid("total", "must be positive"),
            Error::Rule("an order ships once".to_owned()),
        ];

        assert!(retried.iter().all(may_pass_on_retry));
        assert!(!buried.iter().any(may_pass_on_retry));
    }

    #[test]
    fn doubles_from_one_second_up_to_an_hour() {
        let delays: Vec<u64> = [1, 2, 3, 10, 12, 13, 1000]
            .into_iter()
            .map(|attempt| backoff(attempt).as_secs())
            .collect();

        assert_eq!(delays, [1, 2, 4, 512, 2048, 3600, 3600]);
    }

    #[cfg(feature = "contracts")]
    #[test]
    fn a_message_is_retried_for_its_delays_and_a_lease_per_attempt() {
        assert_eq!(retried_for(3), Duration::from_secs(1 + 2 + 3 * 300));
    }
}
