//! A screen's loaded reply kept for its path, query, reach and principal until a record it read
//! changes, here or on another instance, or its time runs out.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use serde_json::Value;
use sofie_core::{Result, Shown, Uuid, unpoisoned};
use sofie_db::{Announced, Db, Reach, Reply, ReplyData, Topic, Topics};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::{RecvError, TryRecvError};

use crate::context::Matched;
use crate::target::Target;
use crate::{Area, PageRoute};

/// Bounds the memory a crawl over many paths can take.
const MAX_ENTRIES: usize = 10_000;
const MAX_TIME: Duration = Duration::from_hours(1);

impl<A: Area> PageRoute<'_, A> {
    /// Reuses the loader's reply for the same path, query, reach and principal for at most `time`
    /// (an hour at most), dropping it once a record it read changes. Only for a loader
    /// whose reply depends on nothing else: not on flags, permissions or the clock.
    pub fn cached(&mut self, time: Duration) -> &mut Self {
        self.rule(|rules| rules.cached = Some(time.min(MAX_TIME)))
    }
}

/// The principal too: a permitted area reaches everything for every principal, yet a loader
/// may still answer by who asks. A listing refuses a bad filter to a JSON caller alone.
#[derive(PartialEq, Eq, Hash)]
struct Key {
    target: String,
    reach: Reach,
    principal: Option<Uuid>,
    #[cfg(feature = "listing")]
    prefers_json: bool,
}

/// Its topics are never `Every`, which nothing keeps.
struct Entry {
    reply: Arc<Reply>,
    topics: Topics,
    until: Instant,
}

/// The changes heard last, so a load that spanned some keeps its reply unless one concerns it.
const RECENT_CHANGES: usize = 256;

#[derive(Default)]
struct Kept {
    entries: HashMap<Arc<Key>, Entry>,
    by_topic: HashMap<Topic, HashSet<Arc<Key>>>,
    recent: VecDeque<Announced>,
    heard: u64,
    changes: Option<Receiver<Announced>>,
}

#[derive(Default)]
pub(crate) struct ScreenCache {
    kept: Arc<Mutex<Kept>>,
    listening: OnceLock<()>,
}

impl ScreenCache {
    pub(crate) async fn read(
        &self,
        db: &Db,
        target: &Target,
        time: Duration,
        matched: Matched,
        asked: &http::Uri,
    ) -> (Result<Reply>, Topics) {
        self.listen(db);
        let key = Key {
            target: asked.to_string(),
            reach: matched.reach,
            principal: matched.session.principal(),
            #[cfg(feature = "listing")]
            prefers_json: matched.prefers_json,
        };
        let hearing = sofie_db::hears_other_instances(db);
        let before = match unpoisoned::lock(&self.kept).hit(&key, hearing) {
            Ok((reply, topics)) => return (Ok(shared(reply)), topics),
            Err(heard) => heard,
        };
        let load = (target.endpoint)(db.clone(), matched);
        let (loaded, topics) = sofie_db::track_topics(load).await;
        let Ok(reply) = loaded else {
            return (loaded, topics);
        };
        let reply = Arc::new(reply);
        if topics != Topics::Every {
            let entry = Entry {
                reply: Arc::clone(&reply),
                topics: topics.clone(),
                until: Instant::now() + time,
            };
            let hearing = sofie_db::hears_other_instances(db);
            unpoisoned::lock(&self.kept).keep(key, entry, before, hearing);
        }
        (Ok(shared(reply)), topics)
    }

    fn listen(&self, db: &Db) {
        self.listening.get_or_init(|| {
            unpoisoned::lock(&self.kept).changes = Some(sofie_db::committed_changes(db));
            let woken = sofie_db::committed_changes(db);
            tokio::spawn(drain_on_change(Arc::downgrade(&self.kept), woken));
        });
    }
}

impl Kept {
    /// `hearing` is read before draining: the listener queues `MaybeMissed` before it says so.
    /// While other instances go unheard, nothing kept can be trusted.
    fn catch_up(&mut self, hearing: bool) {
        self.drain();
        if !hearing {
            self.hear(Announced::MaybeMissed);
        }
    }

    fn drain(&mut self) {
        while let Some(changes) = &mut self.changes {
            let announced = match changes.try_recv() {
                Ok(announced) => announced,
                Err(TryRecvError::Lagged(_)) => Announced::MaybeMissed,
                Err(TryRecvError::Empty | TryRecvError::Closed) => return,
            };
            self.hear(announced);
        }
    }

    /// A miss tells how many changes were heard by then.
    fn hit(&mut self, key: &Key, hearing: bool) -> Result<(Arc<Reply>, Topics), u64> {
        self.catch_up(hearing);
        self.entries
            .get(key)
            .filter(|entry| entry.until > Instant::now())
            .map(|entry| (Arc::clone(&entry.reply), entry.topics.clone()))
            .ok_or(self.heard)
    }

    /// Under the lock the listener forgets under, so a change heard later finds the entry.
    fn keep(&mut self, key: Key, entry: Entry, before: u64, hearing: bool) {
        self.catch_up(hearing);
        if self.changed_since(before, &entry.topics, key.reach) {
            return;
        }
        if self.entries.len() >= MAX_ENTRIES {
            self.make_room(Instant::now());
        }
        let key = Arc::new(key);
        self.remove(&key);
        for topic in read(&entry.topics) {
            self.by_topic
                .entry(*topic)
                .or_default()
                .insert(Arc::clone(&key));
        }
        self.entries.insert(key, entry);
    }

    fn changed_since(&self, before: u64, topics: &Topics, reach: Reach) -> bool {
        let Ok(missed) = usize::try_from(self.heard - before) else {
            return true;
        };
        if missed > self.recent.len() {
            return true;
        }
        self.recent
            .iter()
            .rev()
            .take(missed)
            .any(|announced| announced.concerns(topics, reach))
    }

    fn hear(&mut self, announced: Announced) {
        self.heard += 1;
        match &announced {
            Announced::Changed(changes) => {
                for change in changes.iter() {
                    let concerned: Vec<Arc<Key>> = self
                        .by_topic
                        .get(&change.topic)
                        .into_iter()
                        .flatten()
                        .filter(|key| change.reaches(key.reach))
                        .cloned()
                        .collect();
                    for key in concerned {
                        self.remove(&key);
                    }
                }
            }
            Announced::MaybeMissed => {
                self.entries.clear();
                self.by_topic.clear();
            }
        }
        if self.recent.len() == RECENT_CHANGES {
            self.recent.pop_front();
        }
        self.recent.push_back(announced);
    }

    fn remove(&mut self, key: &Key) {
        let Some(entry) = self.entries.remove(key) else {
            return;
        };
        for topic in read(&entry.topics) {
            if let Some(keys) = self.by_topic.get_mut(topic) {
                keys.remove(key);
                if keys.is_empty() {
                    self.by_topic.remove(topic);
                }
            }
        }
    }

    /// Expired entries first, then the sixteenth closest to expiring, so a crawl over many
    /// queries cannot keep anything new from being cached.
    fn make_room(&mut self, now: Instant) {
        self.drop_until(now);
        if self.entries.len() < MAX_ENTRIES {
            return;
        }
        let mut untils: Vec<Instant> = self.entries.values().map(|entry| entry.until).collect();
        let (_, cutoff, _) = untils.select_nth_unstable(MAX_ENTRIES / 16);
        self.drop_until(*cutoff);
    }

    fn drop_until(&mut self, cutoff: Instant) {
        let dropped: Vec<Arc<Key>> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.until <= cutoff)
            .map(|(key, _)| Arc::clone(key))
            .collect();
        for key in dropped {
            self.remove(&key);
        }
    }
}

fn read(topics: &Topics) -> impl Iterator<Item = &Topic> {
    match topics {
        Topics::Of(read) => Some(read),
        Topics::Every => None,
    }
    .into_iter()
    .flatten()
}

/// Its own receiver only wakes it, so entries are forgotten between requests too; what it heard
/// is drained from the cache's receiver, which a request drains as well.
async fn drain_on_change(kept: Weak<Mutex<Kept>>, mut woken: Receiver<Announced>) {
    loop {
        if let Err(RecvError::Closed) = woken.recv().await {
            return;
        }
        while !matches!(
            woken.try_recv(),
            Err(TryRecvError::Empty | TryRecvError::Closed)
        ) {}
        let Some(kept) = kept.upgrade() else {
            return;
        };
        unpoisoned::lock(&kept).drain();
    }
}

fn shared(reply: Arc<Reply>) -> Reply {
    Reply::Data(Box::new(Shared(reply)))
}

/// Many requests answer from one kept reply.
struct Shared(Arc<Reply>);

impl ReplyData for Shared {
    fn write_json(&self, out: &mut Vec<u8>) -> serde_json::Result<()> {
        self.0.write_json(out).map_err(json_error)
    }

    fn to_json(&self) -> serde_json::Result<Value> {
        self.0.to_json().map_err(json_error)
    }

    fn type_name(&self) -> &'static str {
        match self.0.as_ref() {
            Reply::Data(data) => data.type_name(),
            _ => std::any::type_name::<Reply>(),
        }
    }

    fn shown(&self) -> Option<Shown<'_>> {
        self.0.shown()
    }
}

fn json_error(error: sofie_core::Error) -> serde_json::Error {
    serde_json::Error::io(std::io::Error::other(error))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use sofie_db::{Readers, TableChange};
    use tokio::sync::broadcast;

    use super::*;

    fn key(target: &str) -> Key {
        Key {
            target: target.to_owned(),
            reach: Reach::All,
            principal: None,
            #[cfg(feature = "listing")]
            prefers_json: false,
        }
    }

    fn entry(tables: &[&str], time: Duration) -> Entry {
        let topics = tables
            .iter()
            .map(|table| Topic::named("shop", table))
            .collect::<BTreeSet<_>>();
        Entry {
            reply: Arc::new(Reply::Empty),
            topics: Topics::Of(topics),
            until: Instant::now() + time,
        }
    }

    fn changed(table: &str) -> Announced {
        Announced::Changed(Arc::from([TableChange {
            topic: Topic::named("shop", table),
            readers: Readers::Anyone,
        }]))
    }

    #[test]
    fn a_hit_hears_a_change_already_sent_and_nothing_is_kept_while_others_go_unheard() {
        let (sender, receiver) = broadcast::channel(16);
        let mut kept = Kept {
            changes: Some(receiver),
            ..Kept::default()
        };
        kept.keep(key("/orders"), entry(&["order"], MAX_TIME), 0, true);
        sender.send(changed("order")).unwrap();

        assert!(kept.hit(&key("/orders"), true).is_err());

        let before = kept.heard;
        kept.keep(key("/orders"), entry(&["order"], MAX_TIME), before, true);
        assert!(kept.hit(&key("/orders"), false).is_err());
        let before = kept.heard;
        kept.keep(key("/orders"), entry(&["order"], MAX_TIME), before, false);
        assert!(kept.hit(&key("/orders"), true).is_err());
    }

    #[test]
    fn a_reply_loaded_across_a_change_is_kept_unless_the_change_concerns_it() {
        let mut kept = Kept::default();
        let before = kept.heard;
        kept.hear(changed("customer"));

        kept.keep(key("/orders"), entry(&["order"], MAX_TIME), before, true);
        assert!(kept.hit(&key("/orders"), true).is_ok());

        kept.hear(changed("order"));
        kept.keep(
            key("/orders?page=2"),
            entry(&["order"], MAX_TIME),
            before,
            true,
        );
        assert!(kept.hit(&key("/orders?page=2"), true).is_err());

        kept.hear(Announced::MaybeMissed);
        kept.keep(
            key("/customers"),
            entry(&["customer"], MAX_TIME),
            before + 2,
            true,
        );
        assert!(kept.hit(&key("/customers"), true).is_err());
    }

    #[test]
    fn a_change_forgets_only_the_replies_that_read_its_table() {
        let mut kept = Kept::default();
        kept.keep(
            key("/orders"),
            entry(&["order", "customer"], MAX_TIME),
            0,
            true,
        );
        kept.keep(key("/products"), entry(&["product"], MAX_TIME), 0, true);

        kept.hear(changed("customer"));

        assert!(kept.hit(&key("/orders"), true).is_err());
        assert!(kept.hit(&key("/products"), true).is_ok());
        assert!(!kept.by_topic.contains_key(&Topic::named("shop", "order")));
    }

    #[test]
    fn a_full_cache_forgets_the_entries_closest_to_expiring() {
        let mut kept = Kept::default();
        let page = |index: usize| key(&format!("/orders?page={index}"));
        for index in 0..MAX_ENTRIES {
            let time = Duration::from_millis(u64::try_from(index).unwrap() + 1);
            kept.keep(page(index), entry(&["order"], time), 0, true);
        }

        kept.keep(page(MAX_ENTRIES), entry(&["order"], MAX_TIME), 0, true);

        assert!(kept.hit(&page(MAX_ENTRIES), true).is_ok());
        assert!(kept.hit(&page(0), true).is_err());
        assert!(kept.hit(&page(MAX_ENTRIES - 1), true).is_ok());
    }
}
