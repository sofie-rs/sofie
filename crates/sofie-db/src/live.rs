use std::cell::RefCell;
use std::collections::BTreeSet;
use std::fmt;
use std::hash::{BuildHasher, RandomState};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use sofie_core::{SqlValue, Uuid, unpoisoned};
use tokio::sync::{broadcast, mpsc};

use crate::arrivals::coalesced;
use crate::{Aggregate, Db, Driver, Notifications, Reach, query_values};

const BACKLOG: usize = 256;
/// Postgres refuses a payload of 8000 bytes or more.
const PAYLOAD_LIMIT: usize = 7900;
const LISTEN_RETRY: Duration = Duration::from_secs(5);
const ALIVE_CHECK: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Topic(u64);

impl Topic {
    /// FNV-1a, stable across builds, so instances running different versions agree.
    #[must_use]
    pub fn named(schema: &str, table: &str) -> Self {
        let hash = sofie_core::Fnv1a64::new()
            .write(schema.as_bytes())
            .write(b".")
            .write(table.as_bytes());
        Self(hash.finish())
    }

    #[must_use]
    pub fn of<A: Aggregate>() -> Self {
        Self::named(A::MODULE, A::NAME)
    }

    #[must_use]
    pub fn parse(hex: &str) -> Option<Self> {
        if hex.len() != 16 {
            return None;
        }
        u64::from_str_radix(hex, 16).ok().map(Self)
    }
}

impl fmt::Display for Topic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:016x}", self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Readers {
    Anyone,
    Owner(Uuid),
    /// An owned record whose owner the write did not name: only an unrestricted reach hears it.
    Unrestricted,
}

/// Ordered by topic first, so sorted changes group by table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TableChange {
    pub topic: Topic,
    pub readers: Readers,
}

impl TableChange {
    pub(crate) fn of<A: Aggregate>(owner: Option<Uuid>, reach: Reach) -> Self {
        let readers = match (A::OWNER, owner, reach) {
            (None, _, _) => Readers::Anyone,
            (Some(_), Some(owner), _) | (Some(_), None, Reach::OwnedBy(owner)) => {
                Readers::Owner(owner)
            }
            (Some(_), None, Reach::All | Reach::AllAs(_)) => Readers::Unrestricted,
        };
        Self {
            topic: Topic::of::<A>(),
            readers,
        }
    }

    #[must_use]
    pub fn reaches(&self, reach: Reach) -> bool {
        match (self.readers, reach) {
            (_, Reach::All | Reach::AllAs(_)) | (Readers::Anyone, _) => true,
            (Readers::Owner(owner), Reach::OwnedBy(principal)) => owner == principal,
            (Readers::Unrestricted, Reach::OwnedBy(_)) => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Topics {
    Every,
    Of(BTreeSet<Topic>),
}

impl Topics {
    #[must_use]
    pub fn covers(&self, topic: Topic) -> bool {
        match self {
            Self::Every => true,
            Self::Of(topics) => topics.contains(&topic),
        }
    }
}

tokio::task_local! {
    static HEARD: RefCell<Topics>;
}

pub async fn track_topics<F: Future>(future: F) -> (F::Output, Topics) {
    HEARD
        .scope(RefCell::new(Topics::Of(BTreeSet::new())), async {
            let output = future.await;
            let topics = HEARD.with(|heard| heard.borrow().clone());
            (output, topics)
        })
        .await
}

pub(crate) fn tracking() -> bool {
    HEARD.try_with(|_| ()).is_ok()
}

pub(crate) fn read_tables<'t>(tables: impl IntoIterator<Item = (&'t str, &'t str)>) {
    let _ = HEARD.try_with(|heard| {
        if let Topics::Of(heard) = &mut *heard.borrow_mut() {
            heard.extend(
                tables
                    .into_iter()
                    .map(|(schema, table)| Topic::named(schema, table)),
            );
        }
    });
}

#[cfg(any(feature = "raw-sql", test))]
pub(crate) fn read_anything() {
    let _ = HEARD.try_with(|heard| *heard.borrow_mut() = Topics::Every);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Announced {
    Changed(Arc<[TableChange]>),
    /// Listening to other instances was down, so any change of theirs may have been missed.
    MaybeMissed,
}

impl Announced {
    #[must_use]
    pub fn concerns(&self, topics: &Topics, reach: Reach) -> bool {
        match self {
            Self::Changed(changes) => changes
                .iter()
                .any(|change| topics.covers(change.topic) && change.reaches(reach)),
            Self::MaybeMissed => true,
        }
    }
}

pub(crate) struct Changes {
    driver: Weak<dyn Driver>,
    here: broadcast::Sender<Announced>,
    instance: u64,
    outgoing: Arc<Mutex<Vec<TableChange>>>,
    sending: OnceLock<mpsc::Sender<()>>,
    listening: OnceLock<()>,
    /// Always set where the database tells no other instance; elsewhere only while subscribed.
    hearing: Arc<AtomicBool>,
}

impl Changes {
    pub(crate) fn new(driver: &Arc<dyn Driver>) -> Self {
        Self {
            driver: Arc::downgrade(driver),
            here: broadcast::channel(BACKLOG).0,
            instance: RandomState::new().hash_one(std::process::id()),
            outgoing: Arc::default(),
            sending: OnceLock::new(),
            listening: OnceLock::new(),
            hearing: Arc::new(AtomicBool::new(driver.change_notifications().is_none())),
        }
    }

    pub(crate) fn announce(&self, driver: &Arc<dyn Driver>, changes: Vec<TableChange>) {
        if changes.is_empty() {
            return;
        }
        let changes = distinct(changes);
        // Fails only while no screen listens, which is no failure.
        let _ = self.here.send(Announced::Changed(changes.clone().into()));
        let Some(notify) = driver.dialect().live_notify() else {
            return;
        };
        unpoisoned::lock(&self.outgoing).extend(changes);
        let sender = self.sending.get_or_init(|| {
            let (outgoing, instance) = (Arc::clone(&self.outgoing), self.instance);
            coalesced(Arc::downgrade(driver), move |driver| {
                send(driver, Arc::clone(&outgoing), notify, instance)
            })
        });
        if sender.try_send(()).is_err() {
            tracing::trace!("a notification of changes is already pending");
        }
    }

    fn subscribe(&self) -> broadcast::Receiver<Announced> {
        self.listening.get_or_init(|| {
            tokio::spawn(listen_elsewhere(
                Weak::clone(&self.driver),
                self.here.clone(),
                Arc::clone(&self.hearing),
                self.instance,
            ));
        });
        self.here.subscribe()
    }
}

/// Every change committed from now on, here or by another instance.
#[must_use]
pub fn committed_changes(db: &Db) -> broadcast::Receiver<Announced> {
    db.changes.subscribe()
}

/// Whether another instance's commits reach `committed_changes` now: always where the
/// database tells no other instance (SQLite), elsewhere only while the listener the first
/// `committed_changes` started is subscribed. A cache trusts what it holds only while true.
#[must_use]
pub fn hears_other_instances(db: &Db) -> bool {
    db.changes.hearing.load(Ordering::Acquire)
}

async fn send(
    driver: Arc<dyn Driver>,
    outgoing: Arc<Mutex<Vec<TableChange>>>,
    notify: &'static str,
    instance: u64,
) {
    let changes = distinct(std::mem::take(&mut *unpoisoned::lock(&outgoing)));
    for payload in payloads(instance, &changes) {
        let params = [SqlValue::Text(payload)];
        if let Err(error) = query_values(driver.as_ref(), notify, &params).await {
            tracing::warn!(
                error.kind = "notify",
                error = %error,
                "telling other instances of changes failed; their open screens miss them"
            );
        }
    }
}

/// `<instance> <topic>:<readers> …`, readers `*` for anyone, `!` for an unrestricted reach.
fn distinct(mut changes: Vec<TableChange>) -> Vec<TableChange> {
    changes.sort_unstable();
    changes.dedup();
    changes
}

fn payloads(instance: u64, changes: &[TableChange]) -> Vec<String> {
    let start = format!("{instance:016x}");
    let mut payloads = Vec::new();
    let mut payload = start.clone();
    for change in changes {
        let entry = match change.readers {
            Readers::Anyone => format!(" {}:*", change.topic),
            Readers::Unrestricted => format!(" {}:!", change.topic),
            Readers::Owner(owner) => format!(" {}:{owner}", change.topic),
        };
        if payload.len() + entry.len() > PAYLOAD_LIMIT {
            payloads.push(std::mem::replace(&mut payload, start.clone()));
        }
        payload.push_str(&entry);
    }
    payloads.push(payload);
    payloads
}

fn parsed(payload: &str) -> Option<(u64, Vec<TableChange>)> {
    let mut parts = payload.split(' ');
    let instance = u64::from_str_radix(parts.next()?, 16).ok()?;
    let changes = parts
        .map(|entry| {
            let (topic, readers) = entry.split_once(':')?;
            let readers = match readers {
                "*" => Readers::Anyone,
                "!" => Readers::Unrestricted,
                owner => Readers::Owner(owner.parse().ok()?),
            };
            Some(TableChange {
                topic: Topic::parse(topic)?,
                readers,
            })
        })
        .collect::<Option<Vec<TableChange>>>()?;
    Some((instance, changes))
}

async fn listen_elsewhere(
    driver: Weak<dyn Driver>,
    here: broadcast::Sender<Announced>,
    hearing: Arc<AtomicBool>,
    instance: u64,
) {
    let mut listened_before = false;
    loop {
        let subscribed = {
            let Some(driver) = driver.upgrade() else {
                return;
            };
            let Some(subscribing) = driver.change_notifications() else {
                return;
            };
            subscribing.await
        };
        match subscribed {
            Ok(notices) => {
                // Nothing was missed before the first subscription: no screen was listening.
                if std::mem::replace(&mut listened_before, true) {
                    let _ = here.send(Announced::MaybeMissed);
                }
                // After `MaybeMissed`, so a reader seeing it set finds that queued first.
                hearing.store(true, Ordering::Release);
                let error = follow(notices, &driver, &here, instance).await;
                hearing.store(false, Ordering::Release);
                let Some(error) = error else {
                    return;
                };
                tracing::warn!(error = %error, "listening for other instances' changes failed; listening again");
            }
            Err(error) => {
                tracing::warn!(error = %error, "listening for other instances' changes failed; trying again");
            }
        }
        tokio::time::sleep(LISTEN_RETRY).await;
    }
}

async fn follow(
    mut notices: Box<dyn Notifications>,
    driver: &Weak<dyn Driver>,
    here: &broadcast::Sender<Announced>,
    instance: u64,
) -> Option<sofie_core::Error> {
    loop {
        if driver.strong_count() == 0 {
            return None;
        }
        let payloads = match tokio::time::timeout(ALIVE_CHECK, notices.next()).await {
            Err(_) => continue,
            Ok(Err(error)) => return Some(error),
            Ok(Ok(payloads)) => payloads,
        };
        for payload in payloads {
            match parsed(&payload) {
                Some((sender, _)) if sender == instance => {}
                Some((_, changes)) => drop(here.send(Announced::Changed(changes.into()))),
                None => tracing::warn!(
                    live.payload_bytes = payload.len(),
                    "a change notification this version cannot read was skipped"
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: Uuid = Uuid::from_u128(0x0190_a7d2_0000_7000_8000_0000_0000_0001);
    const OTHER: Uuid = Uuid::from_u128(0x0190_a7d2_0000_7000_8000_0000_0000_0002);

    #[test]
    fn a_topic_is_fnv_1a_of_the_qualified_table_as_every_version_names_it() {
        assert_eq!(
            Topic::named("shop", "order").to_string(),
            "814cce8dc5ccd897"
        );
    }

    fn change(readers: Readers) -> TableChange {
        TableChange {
            topic: Topic::named("shop", "order"),
            readers,
        }
    }

    #[test]
    fn changes_of_interleaved_owners_are_announced_once_each() {
        let changes = [OWNER, OTHER, OWNER, OTHER]
            .map(|owner| change(Readers::Owner(owner)))
            .to_vec();

        assert_eq!(
            distinct(changes),
            [change(Readers::Owner(OWNER)), change(Readers::Owner(OTHER))]
        );
    }

    #[test]
    fn a_change_reaches_only_those_who_may_read_the_record() {
        let owned = change(Readers::Owner(OWNER));

        assert!(owned.reaches(Reach::All));
        assert!(owned.reaches(Reach::OwnedBy(OWNER)));
        assert!(!owned.reaches(Reach::OwnedBy(OTHER)));
        assert!(change(Readers::Anyone).reaches(Reach::OwnedBy(OTHER)));
        assert!(change(Readers::Unrestricted).reaches(Reach::All));
        assert!(change(Readers::Unrestricted).reaches(Reach::AllAs(OTHER)));
        assert!(!change(Readers::Unrestricted).reaches(Reach::OwnedBy(OWNER)));
    }

    #[test]
    fn payloads_read_back_as_written_and_stay_under_the_limit() {
        let changes: Vec<TableChange> = (0..400_u32)
            .map(|index| TableChange {
                topic: Topic::named("shop", &index.to_string()),
                readers: match index % 3 {
                    0 => Readers::Anyone,
                    1 => Readers::Unrestricted,
                    _ => Readers::Owner(OWNER),
                },
            })
            .collect();

        let written = payloads(7, &changes);

        assert!(written.len() > 1);
        assert!(written.iter().all(|payload| payload.len() <= PAYLOAD_LIMIT));
        let read: Vec<TableChange> = written
            .iter()
            .flat_map(|payload| {
                let (instance, changes) = parsed(payload).unwrap();
                assert_eq!(instance, 7);
                changes
            })
            .collect();
        assert_eq!(read, changes);
        assert_eq!(parsed("7 nonsense"), None);
    }

    #[test]
    fn a_topic_is_stable_and_round_trips_as_hex() {
        let topic = Topic::named("shop", "order");

        assert_eq!(topic, Topic::named("shop", "order"));
        assert_ne!(topic, Topic::named("shop", "orders"));
        assert_eq!(Topic::parse(&topic.to_string()), Some(topic));
        assert_eq!(Topic::parse("xyz"), None);
    }

    #[tokio::test]
    async fn reads_are_heard_until_one_cannot_name_its_tables() {
        let order = Topic::named("shop", "order");
        let ((), named) = track_topics(async { read_tables([("shop", "order")]) }).await;
        let ((), unnamed) = track_topics(async {
            read_tables([("shop", "order")]);
            read_anything();
        })
        .await;

        assert_eq!(named, Topics::Of(BTreeSet::from([order])));
        assert_eq!(unnamed, Topics::Every);
        assert!(unnamed.covers(Topic::named("other", "table")));
        read_anything();
    }
}
