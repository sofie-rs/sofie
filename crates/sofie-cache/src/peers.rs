//! Other instances' memory tiers dropping what this one dropped, through the
//! database's notifications or Redis's pub/sub.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sofie_core::{Error, Result, SqlValue, unpoisoned};
use sofie_db::{Db, Notifications, Step};
use tokio::sync::mpsc;

use crate::backoff::Backoff;
use crate::cache::State;
use crate::key::widened_tag;

pub(crate) const CHANNEL: &str = "sofie_cache";
/// Postgres refuses a notification payload of 8000 bytes; past this, record tags widen to their
/// aggregate's, and past that again peers drop everything.
const LONGEST_NOTIFICATION: usize = 7900;
/// Redis takes far longer messages; this bounds what each subscriber reads at once.
#[cfg(feature = "redis")]
const LONGEST_MESSAGE: usize = 1 << 20;
/// Wakes an idle listener to notice its cache was dropped.
const IDLE: Duration = Duration::from_secs(60);
/// A burst of commits sends one notification per gap, as the messages' `Arrivals` do.
const SEND_GAP: Duration = Duration::from_millis(5);

#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Dropped {
    pub(crate) from: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) tags: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) everything: bool,
}

impl Dropped {
    fn payload(&self, longest: usize) -> String {
        let payload = self.encoded();
        if payload.len() <= longest {
            return payload;
        }
        let widened = Self {
            from: self.from.clone(),
            keys: self.keys.clone(),
            tags: self
                .tags
                .iter()
                .map(|tag| widened_tag(tag))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            everything: false,
        }
        .encoded();
        if widened.len() <= longest {
            return widened;
        }
        let everything = Self {
            from: self.from.clone(),
            everything: true,
            ..Self::default()
        };
        everything.encoded()
    }

    fn encoded(&self) -> String {
        serde_json::to_string(self).expect("strings and a flag always encode as JSON")
    }
}

pub(crate) enum Peers {
    Database {
        db: Db,
        batched: Batched,
    },
    /// SQLite serves one instance, so no other memory tier holds what this one drops.
    Alone,
    #[cfg(feature = "redis")]
    Redis(Arc<crate::redis::Client>),
}

impl Peers {
    pub(crate) fn database(db: &Db) -> Self {
        let sender = db.clone();
        let batched = Batched::new(move |payload| {
            let db = sender.clone();
            async move {
                let params = vec![SqlValue::Text(CHANNEL.to_owned()), SqlValue::Text(payload)];
                let notify = Step::statement("SELECT pg_notify($1, $2)", params, None);
                let sent = sofie_db::cache_driver(&db).transaction(vec![notify]).await;
                if let Err(error) = sent {
                    tracing::warn!(error = %error, "telling other instances what the cache dropped failed");
                }
            }
        });
        Self::Database {
            db: db.clone(),
            batched,
        }
    }

    #[cfg_attr(not(feature = "redis"), expect(clippy::unused_async_trait_impl))]
    pub(crate) async fn publish(&self, dropped: &Dropped) -> Result<()> {
        match self {
            Self::Database { batched, .. } => {
                batched.add(dropped);
                Ok(())
            }
            Self::Alone => Ok(()),
            #[cfg(feature = "redis")]
            Self::Redis(client) => {
                client
                    .publish(CHANNEL, &dropped.payload(LONGEST_MESSAGE))
                    .await
            }
        }
    }

    async fn subscribe(&self) -> Result<Box<dyn Notifications>> {
        match self {
            Self::Database { db, .. } => match sofie_db::cache_driver(db).cache_notifications() {
                Some(subscribing) => subscribing.await,
                None => Err(Error::internal_message(
                    "this database sends no notifications, so a memory cache cannot learn what other instances drop",
                )),
            },
            Self::Alone => Err(Error::internal_message("a lone instance hears no other")),
            #[cfg(feature = "redis")]
            Self::Redis(client) => client.subscribe(CHANNEL).await,
        }
    }
}

/// The drops added since the last send, merged; the sending task ends with this value.
pub(crate) struct Batched {
    pending: Arc<Mutex<Pending>>,
    wake: mpsc::Sender<()>,
}

#[derive(Default)]
struct Pending {
    from: String,
    keys: BTreeSet<String>,
    tags: BTreeSet<String>,
    everything: bool,
    /// About the encoded length of `keys` and `tags`.
    length: usize,
}

impl Pending {
    /// Past one payload the send would drop everything anyway, so a stalled send holds no more.
    fn add(&mut self, dropped: &Dropped) {
        self.from.clone_from(&dropped.from);
        self.everything |= dropped.everything;
        if self.everything {
            self.keys.clear();
            self.tags.clear();
            return;
        }
        for (held, names) in [
            (&mut self.keys, &dropped.keys),
            (&mut self.tags, &dropped.tags),
        ] {
            for name in names {
                if held.insert(name.clone()) {
                    self.length += name.len() + 3;
                }
            }
        }
        if self.length > LONGEST_NOTIFICATION {
            self.tags = self.tags.iter().map(|tag| widened_tag(tag)).collect();
            self.length = self
                .keys
                .iter()
                .chain(&self.tags)
                .map(|name| name.len() + 3)
                .sum();
        }
        if self.length > LONGEST_NOTIFICATION {
            self.everything = true;
            self.keys.clear();
            self.tags.clear();
        }
    }

    fn take(&mut self) -> Option<Dropped> {
        if self.keys.is_empty() && self.tags.is_empty() && !self.everything {
            return None;
        }
        self.length = 0;
        Some(Dropped {
            from: self.from.clone(),
            keys: std::mem::take(&mut self.keys).into_iter().collect(),
            tags: std::mem::take(&mut self.tags).into_iter().collect(),
            everything: std::mem::take(&mut self.everything),
        })
    }
}

impl Batched {
    fn new<F: Future<Output = ()> + Send + 'static>(
        send: impl Fn(String) -> F + Send + 'static,
    ) -> Self {
        let pending = Arc::new(Mutex::new(Pending::default()));
        let (wake, mut woken) = mpsc::channel(1);
        let taken = Arc::clone(&pending);
        tokio::spawn(async move {
            while woken.recv().await.is_some() {
                let batch = unpoisoned::lock(&taken).take();
                if let Some(batch) = batch {
                    send(batch.payload(LONGEST_NOTIFICATION)).await;
                    tokio::time::sleep(SEND_GAP).await;
                }
            }
        });
        Self { pending, wake }
    }

    fn add(&self, dropped: &Dropped) {
        unpoisoned::lock(&self.pending).add(dropped);
        // Full means a send is already due, which takes this drop too.
        let _ = self.wake.try_send(());
    }
}

/// Subscribed before the cache is handed out, so its first values are not dropped by a late start.
pub(crate) async fn subscribed(peers: &Peers) -> Result<Option<Box<dyn Notifications>>> {
    if matches!(peers, Peers::Alone) {
        return Ok(None);
    }
    peers.subscribe().await.map(Some)
}

/// Everything held is dropped on each resubscription, as drops sent while unsubscribed are lost.
pub(crate) async fn listen(state: Weak<State>, peers: Arc<Peers>, first: Box<dyn Notifications>) {
    let mut notifications = first;
    let mut backoff = Backoff::default();
    loop {
        if let Err(error) = receive(&state, notifications).await {
            tracing::warn!(error = %error, "listening for other instances' cache drops stopped; subscribing again");
        }
        let Some(again) = resubscribe(&state, &peers, &mut backoff).await else {
            return;
        };
        backoff.succeeded();
        notifications = again;
        if let Some(state) = state.upgrade() {
            state.drop_everything();
        }
    }
}

/// `None` once the cache is gone.
async fn resubscribe(
    state: &Weak<State>,
    peers: &Peers,
    backoff: &mut Backoff,
) -> Option<Box<dyn Notifications>> {
    while state.strong_count() > 0 {
        match peers.subscribe().await {
            Ok(notifications) => return Some(notifications),
            Err(error) => {
                let wait = backoff.failed();
                tracing::warn!(error = %error, retry_in_ms = wait.as_millis(), "listening for other instances' cache drops failed");
                tokio::time::sleep(wait).await;
            }
        }
    }
    None
}

async fn receive(state: &Weak<State>, mut notifications: Box<dyn Notifications>) -> Result<()> {
    loop {
        let payloads = match tokio::time::timeout(IDLE, notifications.next()).await {
            Ok(payloads) => payloads?,
            Err(_) => Vec::new(),
        };
        let Some(state) = state.upgrade() else {
            return Ok(());
        };
        for payload in payloads {
            match serde_json::from_str::<Dropped>(&payload) {
                Ok(dropped) => state.received(&dropped),
                Err(error) => {
                    tracing::warn!(error = %error, "a cache drop from another instance is unreadable; dropping everything");
                    state.drop_everything();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dropped_tag(tag: &str) -> Dropped {
        Dropped {
            from: "a".to_owned(),
            tags: vec![tag.to_owned()],
            ..Dropped::default()
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_burst_of_drops_is_sent_as_one_notification() {
        let sent = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = Arc::clone(&sent);
        let batched = Batched::new(move |payload| {
            recorded.lock().unwrap().push(payload);
            std::future::ready(())
        });

        batched.add(&dropped_tag("first"));
        tokio::time::sleep(SEND_GAP * 4).await;
        for tag in ["second", "third", "second"] {
            batched.add(&dropped_tag(tag));
        }
        tokio::time::sleep(SEND_GAP * 4).await;

        let sent: Vec<Dropped> = sent
            .lock()
            .unwrap()
            .iter()
            .map(|payload| serde_json::from_str(payload).unwrap())
            .collect();
        assert_eq!(sent.len(), 2, "{sent:?}");
        assert_eq!(sent[0].tags, ["first"]);
        let mut burst = sent[1].tags.clone();
        burst.sort();
        assert_eq!(burst, ["second", "third"]);
    }

    #[test]
    fn a_drop_too_long_to_notify_becomes_dropping_everything() {
        let dropped = Dropped {
            from: "a".to_owned(),
            keys: vec!["k".repeat(LONGEST_NOTIFICATION)],
            ..Dropped::default()
        };

        let payload = dropped.payload(LONGEST_NOTIFICATION);

        assert_eq!(payload, r#"{"from":"a","everything":true}"#);
    }

    #[tokio::test]
    async fn drops_held_while_a_send_stalls_stay_within_one_payload() {
        let batched = Batched::new(|_| std::future::pending::<()>());
        for record in 0..10_000 {
            batched.add(&Dropped {
                from: "a".to_owned(),
                tags: vec![format!("shop.order/{record:032}")],
                ..Dropped::default()
            });
            tokio::task::yield_now().await;
        }

        let pending = unpoisoned::lock(&batched.pending);
        assert!(!pending.everything);
        assert!(
            pending.tags.len() * 40 < LONGEST_NOTIFICATION,
            "{}",
            pending.tags.len()
        );
        assert!(pending.tags.contains("shop.order/*"), "{:?}", pending.tags);
    }

    #[test]
    fn a_drop_of_too_many_records_widens_to_their_aggregates() {
        let dropped = Dropped {
            from: "a".to_owned(),
            tags: (0..1000)
                .map(|record| format!("shop.order/{record:032}"))
                .chain(["shop.order/*".to_owned(), "shop.address/7".to_owned()])
                .collect(),
            ..Dropped::default()
        };

        let payload = dropped.payload(LONGEST_NOTIFICATION);

        assert_eq!(
            payload,
            r#"{"from":"a","tags":["shop.address/*","shop.order/*"]}"#
        );
    }
}
