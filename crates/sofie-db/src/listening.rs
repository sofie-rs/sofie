use std::time::Duration;

use crate::{BoxFuture, Db, Wakeups};
use sofie_core::Result;

/// Soon enough that a dropped listener misses little, slow enough that a proxy closing every
/// listening connection does not turn resubscribing into a tight loop.
const RESUBSCRIBE_AFTER: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Messages,
    #[cfg(feature = "shield")]
    ShieldBlocks,
}

impl Channel {
    fn name(self) -> &'static str {
        match self {
            Self::Messages => "messages",
            #[cfg(feature = "shield")]
            Self::ShieldBlocks => "shield_blocks",
        }
    }

    fn subscribe(self, db: &Db) -> Option<BoxFuture<'_, Result<Box<dyn Wakeups>>>> {
        match self {
            Self::Messages => db.driver().wakeups(),
            #[cfg(feature = "shield")]
            Self::ShieldBlocks => crate::SharedBlocks::of(db).wakeups(),
        }
    }
}

/// Without a subscription it polls every `interval` and tries to subscribe again; a renewed
/// subscription ends the wait at once, since a change made while nobody listened woke nobody.
pub struct Listening {
    db: Db,
    channel: Channel,
    interval: Duration,
    wakeups: Option<Box<dyn Wakeups>>,
}

impl Listening {
    /// Subscribes before returning, so a change committed after a read that follows is heard.
    pub async fn start(db: Db, channel: Channel, interval: Duration) -> Self {
        let wakeups = subscribed(&db, channel).await;
        Self {
            db,
            channel,
            interval,
            wakeups,
        }
    }

    pub async fn changed(&mut self) {
        let pause = match self.wakeups.as_mut() {
            None => self.interval,
            Some(wakeups) => match tokio::time::timeout(self.interval, wakeups.next()).await {
                Ok(Ok(())) | Err(_) => return,
                Ok(Err(error)) => {
                    tracing::warn!(error = %error, listen.channel = self.channel.name(), "listening for changes failed; polling until it resumes");
                    RESUBSCRIBE_AFTER
                }
            },
        };
        self.wakeups = None;
        tokio::time::sleep(pause).await;
        self.wakeups = subscribed(&self.db, self.channel).await;
    }
}

/// Takes no `&Listening`, whose `Wakeups` are not `Sync`, so its future stays `Send`.
async fn subscribed(db: &Db, channel: Channel) -> Option<Box<dyn Wakeups>> {
    match channel.subscribe(db)?.await {
        Ok(wakeups) => Some(wakeups),
        Err(error) => {
            tracing::warn!(error = %error, listen.channel = channel.name(), "subscribing to changes failed; polling instead");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::pending;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use sofie_core::{Error, SqlValue};

    use super::*;
    use crate::aggregate::tests::NUMBERED;
    use crate::{Dialect, Driver, RowSink, Step};

    const INTERVAL: Duration = Duration::from_secs(10);

    struct Dropped;

    impl Wakeups for Dropped {
        fn next(&mut self) -> BoxFuture<'_, Result<()>> {
            Box::pin(async { Err(Error::internal_message("the listening connection dropped")) })
        }
    }

    struct Silent;

    impl Wakeups for Silent {
        fn next(&mut self) -> BoxFuture<'_, Result<()>> {
            Box::pin(pending())
        }
    }

    #[derive(Clone, Default)]
    struct Flaky {
        subscriptions: Arc<AtomicUsize>,
        refusing: bool,
    }

    impl Driver for Flaky {
        fn dialect(&self) -> &dyn Dialect {
            &NUMBERED
        }

        fn query<'a>(
            &'a self,
            _: &'a str,
            _: &'a [SqlValue],
            _: &'a mut dyn RowSink,
        ) -> BoxFuture<'a, Result<()>> {
            unimplemented!("listening only subscribes")
        }

        fn transaction(&self, _: Vec<Step>) -> BoxFuture<'_, Result<()>> {
            unimplemented!("listening only subscribes")
        }

        fn wakeups(&self) -> Option<BoxFuture<'_, Result<Box<dyn Wakeups>>>> {
            let subscription = self.subscriptions.fetch_add(1, Ordering::SeqCst);
            let refusing = self.refusing;
            Some(Box::pin(async move {
                if refusing {
                    return Err(Error::internal_message("too many connections"));
                }
                if subscription == 0 {
                    return Ok(Box::new(Dropped) as Box<dyn Wakeups>);
                }
                Ok(Box::new(Silent) as Box<dyn Wakeups>)
            }))
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_dropped_subscription_is_renewed_and_ends_the_wait() {
        let driver = Flaky::default();
        let mut listening =
            Listening::start(Db::new(driver.clone()), Channel::Messages, INTERVAL).await;

        let waited = tokio::time::timeout(RESUBSCRIBE_AFTER * 2, listening.changed()).await;

        assert!(waited.is_ok(), "the renewed subscription ended the wait");
        assert_eq!(driver.subscriptions.load(Ordering::SeqCst), 2);
        assert!(listening.wakeups.is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn a_renewed_subscription_is_kept_across_waits() {
        let driver = Flaky::default();
        let mut listening =
            Listening::start(Db::new(driver.clone()), Channel::Messages, INTERVAL).await;
        listening.changed().await;

        for _ in 0..3 {
            listening.changed().await;
        }

        assert_eq!(driver.subscriptions.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_refused_subscription_is_tried_again_once_per_interval() {
        let driver = Flaky {
            refusing: true,
            ..Flaky::default()
        };
        let mut listening =
            Listening::start(Db::new(driver.clone()), Channel::Messages, INTERVAL).await;

        let started = tokio::time::Instant::now();
        for _ in 0..3 {
            listening.changed().await;
        }

        assert_eq!(started.elapsed(), INTERVAL * 3);
        assert_eq!(driver.subscriptions.load(Ordering::SeqCst), 4);
    }
}
