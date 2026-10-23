use std::any::Any;
use std::future::poll_fn;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::pin;
use std::task::Poll;

use sofie_core::{Error, Result};
use tokio::time::{Instant, interval_at, timeout};

use super::{JOB_TIMEOUT, LEASE};
use crate::{BoxFuture, Claimed, Messages};

/// Renews the lease every third of it while `work` runs, which ends after `JOB_TIMEOUT`.
pub(super) async fn held(
    messages: &Messages,
    message: &Claimed,
    work: impl Future<Output = Result<()>>,
) -> Result<()> {
    let mut work = pin!(timeout(JOB_TIMEOUT, work));
    let every = LEASE / 3;
    let mut beats = interval_at(Instant::now() + every, every);
    let mut renewing: Option<BoxFuture<'_, Result<()>>> = None;
    poll_fn(|context| {
        if let Poll::Ready(done) = work.as_mut().poll(context) {
            return Poll::Ready(done.unwrap_or_else(|_| Err(timed_out())));
        }
        while beats.poll_tick(context).is_ready() {
            renewing.get_or_insert_with(|| Box::pin(messages.renew(message, LEASE)));
        }
        if let Some(Poll::Ready(renewed)) =
            renewing.as_mut().map(|renew| renew.as_mut().poll(context))
        {
            renewing = None;
            if let Err(error) = renewed {
                tracing::warn!(
                    error.kind = "lease",
                    error = %error,
                    message.id = %message.id,
                    "renewing a message's lease failed; it may run out while its job runs"
                );
            }
        }
        Poll::Pending
    })
    .await
}

fn timed_out() -> Error {
    Error::internal_message(format!("the job ran longer than {JOB_TIMEOUT:?}"))
}

/// A panicking job fails like any other, instead of ending every job the worker runs.
pub(super) async fn caught(work: impl Future<Output = Result<()>>) -> Result<()> {
    let mut work = pin!(work);
    poll_fn(
        |context| match catch_unwind(AssertUnwindSafe(|| work.as_mut().poll(context))) {
            Ok(polled) => polled,
            Err(panic) => Poll::Ready(Err(Error::internal_message(format!(
                "the job panicked: {}",
                panic_message(panic.as_ref())
            )))),
        },
    )
    .await
}

fn panic_message(panic: &(dyn Any + Send)) -> &str {
    if let Some(message) = panic.downcast_ref::<&str>() {
        return message;
    }
    panic
        .downcast_ref::<String>()
        .map_or("a value that is not text", String::as_str)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use sofie_core::{Timestamp, Uuid};

    use super::*;
    use crate::arrivals::tests::Recording;
    use crate::{Db, MessageKind};

    fn claimed() -> Claimed {
        Claimed {
            id: Uuid::from_u128(1),
            kind: MessageKind::Job,
            name: "shop.tick".to_owned(),
            payload: serde_json::json!({}),
            attempt: 1,
            traceparent: None,
            available_at: Timestamp::UNIX_EPOCH,
            actor: None,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_long_job_renews_its_lease_until_it_times_out_below_it() {
        let driver = Recording::default();
        let messages = Messages::new(Db::new(driver.clone()));

        let outcome = held(&messages, &claimed(), async {
            tokio::time::sleep(LEASE * 2).await;
            Ok(())
        })
        .await;

        assert!(
            matches!(outcome, Err(Error::Internal { .. })),
            "{outcome:?}"
        );
        assert!(JOB_TIMEOUT < LEASE);
        let renewals = JOB_TIMEOUT.as_secs() / (LEASE / 3).as_secs();
        assert_eq!(
            driver.transactions(),
            vec![vec!["renew".to_owned()]; usize::try_from(renewals).unwrap()]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_short_job_ends_with_its_own_outcome_and_no_renewal() {
        let driver = Recording::default();
        let messages = Messages::new(Db::new(driver.clone()));

        let outcome = held(&messages, &claimed(), async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            Err(Error::Conflict)
        })
        .await;

        assert!(matches!(outcome, Err(Error::Conflict)));
        assert_eq!(driver.transactions(), Vec::<Vec<String>>::new());
    }

    #[tokio::test]
    async fn a_panicking_job_fails_with_the_panic_s_message() {
        let failed = caught(async { panic!("out of stock") }).await;

        assert!(
            matches!(&failed, Err(Error::Internal { context, .. }) if context == "the job panicked: out of stock"),
            "{failed:?}"
        );
    }
}
