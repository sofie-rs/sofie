#[cfg(feature = "testing")]
use std::sync::Mutex;
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

#[cfg(feature = "testing")]
use serde_json::Value;
#[cfg(feature = "testing")]
use sofie_core::unpoisoned;
use tokio::sync::{Notify, mpsc};

#[cfg(feature = "testing")]
use crate::Db;
use crate::heard::Heard;
use crate::{Driver, RaisedEvent, Step};

pub(crate) struct Raised(#[cfg(feature = "testing")] Option<Vec<RaisedEvent>>);

impl Raised {
    #[cfg(feature = "testing")]
    fn copy(arrivals: &Arrivals, events: &[RaisedEvent]) -> Self {
        Self(arrivals.raised.get().map(|_| events.to_vec()))
    }

    #[cfg(not(feature = "testing"))]
    fn copy(_: &Arrivals, _: &[RaisedEvent]) -> Self {
        Self()
    }

    #[cfg(feature = "testing")]
    fn keep(raised: Self, arrivals: &Arrivals) {
        if let (Some(log), Some(raised)) = (arrivals.raised.get(), raised.0) {
            unpoisoned::lock(log).extend(raised);
        }
    }

    #[cfg(not(feature = "testing"))]
    fn keep(_: Self, _: &Arrivals) {}
}

/// A burst of commits sends one notification per gap, not one each.
const NOTIFY_GAP: Duration = Duration::from_millis(5);

#[derive(Default)]
pub(crate) struct Arrivals {
    here: Notify,
    elsewhere: OnceLock<mpsc::Sender<()>>,
    heard: Arc<Heard>,
    #[cfg(feature = "testing")]
    raised: OnceLock<Mutex<Vec<RaisedEvent>>>,
}

impl Arrivals {
    pub(crate) fn heard(&self) -> &Arc<Heard> {
        &self.heard
    }

    pub(crate) fn copy_raised(&self, events: &[RaisedEvent]) -> Raised {
        Raised::copy(self, events)
    }

    pub(crate) fn committed(&self, driver: &Arc<dyn Driver>, queued: bool, raised: Raised) {
        Raised::keep(raised, self);
        if queued {
            self.announce(driver);
        }
    }

    fn announce(&self, driver: &Arc<dyn Driver>) {
        self.here.notify_one();
        let Some(notify) = driver.dialect().messages().notify else {
            return;
        };
        let sender = self.elsewhere.get_or_init(|| {
            coalesced(Arc::downgrade(driver), move |driver| async move {
                let sent = driver.transaction(vec![Step::Script(notify.to_owned())]);
                if let Err(error) = sent.await {
                    tracing::warn!(
                        error.kind = "notify",
                        error = %error,
                        "notifying other workers of new messages failed; they find them by polling"
                    );
                }
            })
        });
        // Full means a notification is already waiting to be sent, which covers this commit too.
        if sender.try_send(()).is_err() {
            tracing::trace!("a notification for new messages is already pending");
        }
    }

    /// Also returns at once for a commit made while nobody waited.
    pub(crate) async fn arrived(&self) {
        self.here.notified().await;
    }
}

/// Every clone of `db` then records each event a successful commit raised.
#[cfg(feature = "testing")]
#[doc(hidden)]
pub fn keep_raised(db: &Db) {
    db.arrivals().raised.get_or_init(Mutex::default);
}

#[cfg(feature = "testing")]
#[doc(hidden)]
#[must_use]
pub fn raised(db: &Db, name: &str) -> Vec<Value> {
    let Some(raised) = db.arrivals().raised.get() else {
        return Vec::new();
    };
    let raised = unpoisoned::lock(raised);
    let named = raised.iter().filter(|event| event.name == name);
    named.map(|event| event.payload.clone()).collect()
}

/// One `send` per gap however often poked; ends with its database.
pub(crate) fn coalesced<F: Future<Output = ()> + Send>(
    driver: Weak<dyn Driver>,
    send: impl Fn(Arc<dyn Driver>) -> F + Send + 'static,
) -> mpsc::Sender<()> {
    let (sender, mut pending) = mpsc::channel(1);
    tokio::spawn(async move {
        while pending.recv().await.is_some() {
            let Some(driver) = driver.upgrade() else {
                return;
            };
            send(driver).await;
            tokio::time::sleep(NOTIFY_GAP).await;
        }
    });
    sender
}

#[cfg(test)]
pub(crate) mod tests {
    use sofie_core::{Error, Result, SqlValue};

    use super::*;
    use crate::aggregate::tests::Numbered;
    use crate::heard::{Known, Listeners};
    use crate::{BoxFuture, Db, Dialect, MessageKind, MessageSql, Outcome, Plan, RowSink};

    const NOTIFY: &str = "NOTIFY sofie_messages";

    const MESSAGES: MessageSql = MessageSql {
        renew: "renew",
        fan_out_events: "fan out",
        notify: Some(NOTIFY),
        ..crate::aggregate::tests::MESSAGES
    };

    #[derive(Clone, Default)]
    pub(crate) struct Recording {
        transactions: Arc<Mutex<Vec<Vec<String>>>>,
        is_down: bool,
    }

    impl Recording {
        pub(crate) fn transactions(&self) -> Vec<Vec<String>> {
            self.transactions.lock().unwrap().clone()
        }
    }

    impl Driver for Recording {
        fn dialect(&self) -> &dyn Dialect {
            &Numbered(&MESSAGES)
        }

        fn query<'a>(
            &'a self,
            _: &'a str,
            _: &'a [SqlValue],
            _: &'a mut dyn RowSink,
        ) -> BoxFuture<'a, Result<()>> {
            unimplemented!("the tests only commit")
        }

        fn transaction(&self, steps: Vec<Step>) -> BoxFuture<'_, Result<()>> {
            let sql = steps.into_iter().map(|step| match step {
                Step::Statement { sql, .. } | Step::Script(sql) => sql,
            });
            self.transactions.lock().unwrap().push(sql.collect());
            let outcome = if self.is_down {
                Err(Error::internal_message("connection refused"))
            } else {
                Ok(())
            };
            Box::pin(std::future::ready(outcome))
        }
    }

    struct QueuesJob;

    impl Outcome for QueuesJob {
        fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
            plan.queue(MessageKind::Job, [("send_receipt", serde_json::json!({}))])
        }
    }

    struct Raises(&'static str);

    impl Outcome for Raises {
        fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
            let payload = serde_json::json!({});
            plan.raise(&[RaisedEvent {
                name: self.0,
                payload,
            }]);
            Ok(())
        }
    }

    fn publish_listener(db: &Db, event: &'static str, listener: &'static str) {
        let mut listeners = Listeners::default();
        listeners.by_event.insert(event, vec![listener]);
        let known = Known {
            listeners,
            jobs: [listener].into(),
        };
        db.arrivals().heard().add(&known);
    }

    async fn settle() {
        tokio::time::sleep(NOTIFY_GAP * 4).await;
    }

    #[tokio::test(start_paused = true)]
    async fn the_notification_follows_the_commit_instead_of_joining_it() {
        let driver = Recording::default();
        let db = Db::new(driver.clone());

        db.commit(&mut QueuesJob).await.unwrap();
        settle().await;

        assert_eq!(
            driver.transactions(),
            [vec![MESSAGES.insert.to_owned()], vec![NOTIFY.to_owned()]]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_burst_of_commits_sends_one_notification() {
        let driver = Recording::default();
        let db = Db::new(driver.clone());

        for _ in 0..50 {
            db.commit(&mut QueuesJob).await.unwrap();
        }
        settle().await;

        let notifications = driver
            .transactions()
            .into_iter()
            .filter(|steps| steps == &[NOTIFY])
            .count();
        assert_eq!(notifications, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_worker_sharing_the_arrivals_wakes_for_a_commit_made_before_it_waited() {
        let web = Db::new(Recording::default());
        let worker = Db::new(Recording::default()).sharing_arrivals(&web);

        web.commit(&mut QueuesJob).await.unwrap();

        tokio::time::timeout(Duration::from_millis(1), worker.arrivals().arrived())
            .await
            .expect("the commit woke the worker");
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_commit_wakes_nobody() {
        let driver = Recording {
            is_down: true,
            ..Recording::default()
        };
        let db = Db::new(driver.clone());

        assert!(db.commit(&mut QueuesJob).await.is_err());
        settle().await;

        assert_eq!(driver.transactions().len(), 1, "no notification was sent");
        let woken = tokio::time::timeout(Duration::from_secs(1), db.arrivals().arrived()).await;
        assert!(woken.is_err(), "no worker was woken");
    }

    #[tokio::test(start_paused = true)]
    async fn a_published_commit_writes_only_the_jobs_of_events_a_listener_takes() {
        let driver = Recording::default();
        let db = Db::new(driver.clone());
        publish_listener(&db, "shop.Restocked", "shop.restock");

        db.commit(&mut Raises("shop.Restocked")).await.unwrap();
        db.commit(&mut Raises("shop.Unheard")).await.unwrap();
        settle().await;

        assert_eq!(
            driver.transactions(),
            [vec!["fan out".to_owned()], vec![NOTIFY.to_owned()]]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_raised_log_keeps_a_commit_s_events_only_once_it_succeeds() {
        let failing = Db::new(Recording {
            is_down: true,
            ..Recording::default()
        });
        let working = Db::new(Recording::default());
        keep_raised(&failing);
        keep_raised(&working.clone());

        assert!(failing.commit(&mut Raises("shop.Restocked")).await.is_err());
        working.commit(&mut Raises("shop.Restocked")).await.unwrap();

        assert_eq!(raised(&failing, "shop.Restocked"), Vec::<Value>::new());
        assert_eq!(raised(&working, "shop.Restocked"), [serde_json::json!({})]);
    }
}
