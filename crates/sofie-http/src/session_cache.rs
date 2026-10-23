use std::collections::HashMap;
use std::collections::hash_map::Entry as Slot;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use sofie_core::{Instant, Result, Timestamp, Uuid, unpoisoned};
use sofie_db::{Announced, Db, Readers, Topic};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::{RecvError, TryRecvError};

use crate::{AreaMembers, SessionRecord};

/// About 100 bytes an entry, so some 10 MB at most. When full, expired entries go first once the
/// earliest has; until then a new one is not kept, as in the screen cache.
const CAPACITY: usize = 100_000;
/// A safety net under the invalidation: a notification another instance failed to send is
/// missed silently, so an ended session may live on here at most this long. A minute costs
/// one read per active session and minute instead of one per request.
const TIME_TO_LIVE: Duration = Duration::from_mins(1);

/// Each session a cookie named, so a request asks the store only on a miss.
///
/// Expiry: an entry is trusted for `TIME_TO_LIVE` by the database's clock, and a live record
/// carries its own end. Invalidation: a committed write to the store's sessions, here or on
/// another instance, drops the entries of the account owning it; a missed notification, a
/// lagging reader or listening that is down drops them all. The changes are drained under the
/// lock on each lookup, so the request after a commit on this instance never finds the session
/// that commit ended, and by a task woken by each announcement, so no lookups for a while do not
/// lag the feed.
#[derive(Default)]
pub(crate) struct SessionCache {
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    changes: Option<Receiver<Announced>>,
    held: Held,
}

#[derive(Default)]
struct Held {
    entries: HashMap<Uuid, Entry>,
    by_account: HashMap<Uuid, Vec<Uuid>>,
    /// At or before every entry's end, so a full cache is swept only once one may have ended.
    earliest: Option<Instant>,
    /// Moved by every drop, so a record read across one is not kept.
    generation: u64,
}

#[derive(Clone, Copy)]
struct Entry {
    record: SessionRecord,
    until: Instant,
}

impl SessionCache {
    /// The session's record while it is live. `seen` writes the store's last-seen time, so it
    /// always reaches the store.
    pub(crate) async fn live(
        &self,
        members: &dyn AreaMembers,
        db: &Db,
        session: Uuid,
        seen: Option<Timestamp>,
    ) -> Result<Option<SessionRecord>> {
        let now = db.clock().now();
        let Some(topic) = members.session_topic() else {
            let record = members.session_record(db, session, seen).await?;
            return Ok(record.is_live(now).then_some(record));
        };
        let generation = {
            let mut state = unpoisoned::lock(&self.state);
            if state.changes.is_none() {
                state.changes = Some(sofie_db::committed_changes(db));
                let woken = sofie_db::committed_changes(db);
                tokio::spawn(drain_on_change(Arc::downgrade(&self.state), woken, topic));
            }
            state.catch_up(db, topic);
            if seen.is_none()
                && let Some(record) = state.held.record(session, now)
            {
                return Ok(record.is_live(now).then_some(record));
            }
            state.held.generation
        };
        let record = members.session_record(db, session, seen).await?;
        let mut state = unpoisoned::lock(&self.state);
        state.catch_up(db, topic);
        if state.held.generation == generation {
            state.held.keep(session, record, now)?;
        }
        Ok(record.is_live(now).then_some(record))
    }
}

impl State {
    /// Whether listening holds is read before draining: the listener queues `MaybeMissed`
    /// before it says so, so a reader that saw it hold drains that too.
    fn catch_up(&mut self, db: &Db, topic: Topic) {
        let hearing = sofie_db::hears_other_instances(db);
        self.drain(topic);
        if !hearing {
            self.held.clear();
        }
    }

    fn drain(&mut self, topic: Topic) {
        let Some(receiver) = &mut self.changes else {
            return;
        };
        loop {
            match receiver.try_recv() {
                Ok(Announced::Changed(changes)) => changes
                    .iter()
                    .filter(|change| change.topic == topic)
                    .for_each(|change| self.held.drop_readers(change.readers)),
                Ok(Announced::MaybeMissed) | Err(TryRecvError::Lagged(_)) => self.held.clear(),
                Err(TryRecvError::Empty | TryRecvError::Closed) => break,
            }
        }
    }
}

/// Its own receiver only wakes it; what it heard is drained from the cache's receiver.
async fn drain_on_change(state: Weak<Mutex<State>>, mut woken: Receiver<Announced>, topic: Topic) {
    loop {
        if let Err(RecvError::Closed) = woken.recv().await {
            return;
        }
        while !matches!(
            woken.try_recv(),
            Err(TryRecvError::Empty | TryRecvError::Closed)
        ) {}
        let Some(state) = state.upgrade() else {
            return;
        };
        unpoisoned::lock(&state).drain(topic);
    }
}

impl Held {
    fn record(&mut self, session: Uuid, now: Instant) -> Option<SessionRecord> {
        let Slot::Occupied(slot) = self.entries.entry(session) else {
            return None;
        };
        if slot.get().until > now {
            return Some(slot.get().record);
        }
        slot.remove();
        None
    }

    fn keep(&mut self, session: Uuid, record: SessionRecord, now: Instant) -> Result<()> {
        if self.entries.len() >= CAPACITY && self.earliest.is_some_and(|earliest| earliest <= now) {
            self.sweep(now);
        }
        if self.entries.len() >= CAPACITY {
            return Ok(());
        }
        if let SessionRecord::Live { account, .. } = record {
            let sessions = self.by_account.entry(account).or_default();
            if !sessions.contains(&session) {
                sessions.push(session);
            }
        }
        let until = now.checked_add(TIME_TO_LIVE)?;
        self.earliest = Some(self.earliest.map_or(until, |earliest| earliest.min(until)));
        self.entries.insert(session, Entry { record, until });
        Ok(())
    }

    fn sweep(&mut self, now: Instant) {
        self.entries.retain(|_, entry| entry.until > now);
        let entries = &self.entries;
        self.by_account.retain(|_, sessions| {
            sessions.retain(|session| entries.contains_key(session));
            !sessions.is_empty()
        });
        self.earliest = self.entries.values().map(|entry| entry.until).min();
    }

    /// An ended or unrecorded session stays so, so only a live one is dropped by its account.
    fn drop_readers(&mut self, readers: Readers) {
        let Readers::Owner(account) = readers else {
            self.clear();
            return;
        };
        self.generation = self.generation.wrapping_add(1);
        for session in self.by_account.remove(&account).unwrap_or_default() {
            self.entries.remove(&session);
        }
    }

    fn clear(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.entries.clear();
        self.by_account.clear();
        self.earliest = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_cache_keeps_a_new_session_once_the_earliest_has_ended() {
        let start = Instant::from(Timestamp::from_second(1_700_000_000).unwrap());
        let mut held = Held::default();
        for index in 0..CAPACITY {
            let session = Uuid::from_u128(u128::try_from(index).unwrap() + 1);
            held.keep(session, SessionRecord::Unrecorded, start)
                .unwrap();
        }
        let newcomer = Uuid::from_u128(0);

        held.keep(newcomer, SessionRecord::Unrecorded, start)
            .unwrap();
        assert!(!held.entries.contains_key(&newcomer));
        let ended = start.checked_add(TIME_TO_LIVE).unwrap();
        held.keep(newcomer, SessionRecord::Unrecorded, ended)
            .unwrap();

        assert_eq!(held.entries.len(), 1);
        assert!(held.entries.contains_key(&newcomer));
    }
}
