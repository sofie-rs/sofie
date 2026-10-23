use std::future::{pending, poll_fn};
use std::pin::pin;
use std::task::Poll;
use std::time::Duration;

use tokio::time::Instant;

use crate::{BoxFuture, Messages, Wakeups};

type Subscribing = BoxFuture<'static, Option<Box<dyn Wakeups>>>;

/// A busy worker waits many times a second; each must not open a connection the database refuses.
const RESUBSCRIBE_AFTER: Duration = Duration::from_secs(5);

/// Without a subscription the worker polls; a lost one is renewed on the next wait, a refused
/// one after `RESUBSCRIBE_AFTER`.
pub(crate) struct Wake {
    messages: Messages,
    subscription: Option<Box<dyn Wakeups>>,
    /// Kept across waits, so a connection slower than the poll interval still subscribes.
    subscribing: Option<Subscribing>,
    resubscribe_at: Option<Instant>,
}

impl Wake {
    pub(crate) fn new(messages: Messages) -> Self {
        Self {
            messages,
            subscription: None,
            subscribing: None,
            resubscribe_at: None,
        }
    }

    pub(crate) async fn notified(&mut self) {
        let may_subscribe = self.resubscribe_at.is_none_or(|at| Instant::now() >= at);
        if self.subscription.is_none() && self.subscribing.is_none() && may_subscribe {
            let messages = self.messages.clone();
            self.subscribing = Some(Box::pin(async move { subscribe(&messages).await }));
        }
        let Self {
            messages,
            subscription,
            subscribing,
            resubscribe_at,
        } = self;
        let mut arrived = pin!(messages.arrived());
        let mut notified = pin!(async {
            if let Some(connecting) = subscribing.as_mut() {
                *subscription = connecting.await;
                *subscribing = None;
                *resubscribe_at = subscription
                    .is_none()
                    .then(|| Instant::now() + RESUBSCRIBE_AFTER);
            }
            let Some(live) = subscription.as_mut() else {
                return pending().await;
            };
            if let Err(error) = live.next().await {
                tracing::warn!(
                    error.kind = "listen",
                    error = %error,
                    "the subscription to new messages was lost; polling until it is renewed"
                );
                *subscription = None;
            }
        });
        poll_fn(|context| {
            if arrived.as_mut().poll(context).is_ready()
                || notified.as_mut().poll(context).is_ready()
            {
                return Poll::Ready(());
            }
            Poll::Pending
        })
        .await;
    }
}

async fn subscribe(messages: &Messages) -> Option<Box<dyn Wakeups>> {
    let subscribing = messages.wakeups()?;
    subscribing
        .await
        .inspect_err(|error| {
            tracing::warn!(
                error.kind = "listen",
                error = %error,
                "subscribing to new messages failed; polling instead"
            );
        })
        .ok()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use sofie_core::{Result, SqlValue};

    use super::*;
    use crate::aggregate::tests::NUMBERED;
    use crate::{Db, Dialect, Driver, RowSink, Step};

    const CONNECTING: Duration = Duration::from_secs(3);

    struct Silent;

    impl Wakeups for Silent {
        fn next(&mut self) -> BoxFuture<'_, Result<()>> {
            Box::pin(pending())
        }
    }

    struct SlowToListen;

    impl Driver for SlowToListen {
        fn dialect(&self) -> &dyn Dialect {
            &NUMBERED
        }

        fn query<'a>(
            &'a self,
            _: &'a str,
            _: &'a [SqlValue],
            _: &'a mut dyn RowSink,
        ) -> BoxFuture<'a, Result<()>> {
            unimplemented!("waiting only subscribes")
        }

        fn transaction(&self, _: Vec<Step>) -> BoxFuture<'_, Result<()>> {
            unimplemented!("waiting only subscribes")
        }

        fn wakeups(&self) -> Option<BoxFuture<'_, Result<Box<dyn Wakeups>>>> {
            Some(Box::pin(async {
                tokio::time::sleep(CONNECTING).await;
                Ok(Box::new(Silent) as Box<dyn Wakeups>)
            }))
        }
    }

    #[derive(Clone, Default)]
    struct RefusesToListen {
        attempts: Arc<AtomicUsize>,
    }

    impl Driver for RefusesToListen {
        fn dialect(&self) -> &dyn Dialect {
            &NUMBERED
        }

        fn query<'a>(
            &'a self,
            _: &'a str,
            _: &'a [SqlValue],
            _: &'a mut dyn RowSink,
        ) -> BoxFuture<'a, Result<()>> {
            unimplemented!("waiting only subscribes")
        }

        fn transaction(&self, _: Vec<Step>) -> BoxFuture<'_, Result<()>> {
            unimplemented!("waiting only subscribes")
        }

        fn wakeups(&self) -> Option<BoxFuture<'_, Result<Box<dyn Wakeups>>>> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            Some(Box::pin(std::future::ready(Err(
                sofie_core::Error::internal_message("too many connections"),
            ))))
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_refused_subscription_is_not_tried_again_on_every_wait() {
        let driver = RefusesToListen::default();
        let mut wake = Wake::new(Messages::new(Db::new(driver.clone())));

        for _ in 0..50 {
            let _ = tokio::time::timeout(Duration::from_millis(10), wake.notified()).await;
        }
        assert_eq!(driver.attempts.load(Ordering::SeqCst), 1);

        tokio::time::sleep(RESUBSCRIBE_AFTER).await;
        let _ = tokio::time::timeout(Duration::from_millis(10), wake.notified()).await;
        assert_eq!(driver.attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_subscription_slower_than_each_wait_is_still_made() {
        let mut wake = Wake::new(Messages::new(Db::new(SlowToListen)));

        for _ in 0..4 {
            let waited = tokio::time::timeout(Duration::from_secs(1), wake.notified()).await;
            assert!(waited.is_err(), "nothing woke the worker");
        }

        assert!(wake.subscription.is_some());
    }
}
