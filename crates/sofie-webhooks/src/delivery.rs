use std::time::Duration;

use sofie::core::Uuid;
use sofie::prelude::*;

use crate::endpoint::{WebhookEndpointId, WebhookOwner};

/// Waits after each failed attempt, as Standard Webhooks suggests: eight attempts over about
/// 27 hours, so a receiver down for a night still gets every event.
const RETRY_DELAYS: [Duration; 7] = [
    Duration::from_secs(5),
    Duration::from_mins(5),
    Duration::from_mins(30),
    Duration::from_hours(2),
    Duration::from_hours(5),
    Duration::from_hours(10),
    Duration::from_hours(10),
];
const MAXIMUM_ERROR: usize = 500;

/// One event bound for one endpoint, with its attempts so far; its id is the `webhook-id`
/// every attempt carries, so a receiver drops a repeat.
#[derive(Aggregate)]
pub struct WebhookDelivery {
    meta: Meta,
    #[owner]
    owner_id: Id<WebhookOwner>,
    endpoint: WebhookEndpointId,
    event: String,
    /// What is signed and sent, the same on every attempt.
    body: String,
    attempts: i64,
    attempted_at: Option<Instant>,
    status: Option<i64>,
    error: Option<String>,
    next_attempt_at: Option<Instant>,
    delivered_at: Option<Instant>,
    failed_at: Option<Instant>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Attempt {
    Delivered(u16),
    Refused(u16),
    /// `retry: false` when the address was refused before sending, as every attempt would be.
    Unanswered {
        error: String,
        retry: bool,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Next {
    Done,
    RetryAt(Instant),
    GaveUp,
}

impl WebhookDelivery {
    pub(crate) fn new(
        owner: Uuid,
        endpoint: WebhookEndpointId,
        event: &str,
        body: String,
    ) -> Result<Self> {
        Ok(Self {
            meta: Meta::new()?,
            owner_id: Id::from_uuid(owner),
            endpoint,
            event: event.to_owned(),
            body,
            attempts: 0,
            attempted_at: None,
            status: None,
            error: None,
            next_attempt_at: None,
            delivered_at: None,
            failed_at: None,
        })
    }

    #[must_use]
    pub fn endpoint(&self) -> WebhookEndpointId {
        self.endpoint
    }

    #[must_use]
    pub fn event(&self) -> &str {
        &self.event
    }

    #[must_use]
    pub fn body(&self) -> &str {
        &self.body
    }

    #[must_use]
    pub fn attempts(&self) -> i64 {
        self.attempts
    }

    #[must_use]
    pub fn attempted_at(&self) -> Option<Instant> {
        self.attempted_at
    }

    #[must_use]
    pub fn status(&self) -> Option<i64> {
        self.status
    }

    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    #[must_use]
    pub fn next_attempt_at(&self) -> Option<Instant> {
        self.next_attempt_at
    }

    #[must_use]
    pub fn delivered_at(&self) -> Option<Instant> {
        self.delivered_at
    }

    #[must_use]
    pub fn failed_at(&self) -> Option<Instant> {
        self.failed_at
    }

    pub(crate) fn is_settled(&self) -> bool {
        self.delivered_at.is_some() || self.failed_at.is_some()
    }

    pub(crate) fn attempted(&mut self, attempt: Attempt, now: Instant) -> Result<Next> {
        self.attempts += 1;
        self.attempted_at = Some(now);
        let (status, error, retry) = match attempt {
            Attempt::Delivered(status) => {
                self.status = Some(i64::from(status));
                self.error = None;
                self.next_attempt_at = None;
                self.delivered_at = Some(now);
                return Ok(Next::Done);
            }
            Attempt::Refused(status) => (Some(i64::from(status)), None, true),
            Attempt::Unanswered { error, retry } => (None, Some(error), retry),
        };
        self.status = status;
        self.error = error.map(|error| error.chars().take(MAXIMUM_ERROR).collect());
        let delay = usize::try_from(self.attempts - 1)
            .ok()
            .and_then(|index| RETRY_DELAYS.get(index))
            .filter(|_| retry);
        let Some(delay) = delay else {
            self.give_up(now);
            return Ok(Next::GaveUp);
        };
        let at = now.checked_add(*delay)?;
        self.next_attempt_at = Some(at);
        Ok(Next::RetryAt(at))
    }

    pub(crate) fn give_up(&mut self, now: Instant) {
        self.next_attempt_at = None;
        self.failed_at = Some(now);
    }

    pub(crate) fn resend(&mut self, now: Instant) {
        self.attempts = 0;
        self.failed_at = None;
        self.delivered_at = None;
        self.next_attempt_at = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delivery() -> WebhookDelivery {
        WebhookDelivery::new(
            Uuid::now_v7().unwrap(),
            Id::from_uuid(Uuid::now_v7().unwrap()),
            "order.paid",
            "{}".to_owned(),
        )
        .unwrap()
    }

    fn at(second: i64) -> Instant {
        Instant::from(sofie::core::Timestamp::from_second(second).unwrap())
    }

    #[test]
    fn a_failing_delivery_is_tried_eight_times_then_given_up() {
        let mut delivery = delivery();
        let mut waits = Vec::new();
        let mut now = at(1_800_000_000);

        loop {
            match delivery.attempted(Attempt::Refused(500), now).unwrap() {
                Next::RetryAt(next) => {
                    waits.push(next.timestamp().as_second() - now.timestamp().as_second());
                    now = next;
                }
                Next::GaveUp => break,
                Next::Done => unreachable!("a refusal is not a delivery"),
            }
        }

        assert_eq!(waits, [5, 300, 1_800, 7_200, 18_000, 36_000, 36_000]);
        assert_eq!(delivery.attempts(), 8);
        assert_eq!(delivery.failed_at(), Some(now));
        assert_eq!(delivery.status(), Some(500));
    }

    #[test]
    fn a_refused_address_is_not_tried_again() {
        let mut delivery = delivery();
        let unanswered = Attempt::Unanswered {
            error: "the URL is refused".to_owned(),
            retry: false,
        };

        assert_eq!(
            delivery.attempted(unanswered, at(10)).unwrap(),
            Next::GaveUp
        );
        assert_eq!(delivery.error(), Some("the URL is refused"));
    }

    #[test]
    fn a_resent_delivery_starts_over() {
        let mut delivery = delivery();
        delivery.attempted(Attempt::Refused(404), at(10)).unwrap();
        delivery.give_up(at(11));

        delivery.resend(at(20));

        assert!(!delivery.is_settled());
        assert_eq!(delivery.attempts(), 0);
        assert_eq!(
            delivery.attempted(Attempt::Delivered(204), at(21)).unwrap(),
            Next::Done
        );
        assert_eq!(delivery.delivered_at(), Some(at(21)));
    }
}
