use std::sync::{Arc, Mutex, Weak};

use serde::Serialize;
use serde::de::DeserializeOwned;
use sofie_core::{Error, Result, unpoisoned};
use sofie_db::{ChangedRecord, CommitHook, Context, Db, Resolve};

use crate::Entry;
use crate::backoff::Backoff;
use crate::flight::Flights;
use crate::invalidations::{Invalidations, Pending};
use crate::key::{Key, aggregate_tag, any_record_tag, record_tag};
use crate::settings::Store;
use crate::shared::{SharedStore, Stored};
use crate::telemetry::{self, Operation};

/// Lives as long as this value or a clone of it does; the database it was started on
/// holds only a weak reference, through which handlers resolve it.
#[derive(Clone)]
pub struct Cache {
    state: Arc<State>,
}

pub(crate) struct State {
    #[cfg(feature = "memory")]
    near: crate::memory::Memory,
    shared: Option<Arc<dyn SharedStore>>,
    #[cfg(feature = "memory")]
    peers: Arc<crate::peers::Peers>,
    pub(crate) flights: Flights,
    pub(crate) invalidations: Invalidations,
    pending: Pending,
    #[cfg(feature = "memory")]
    instance: String,
}

/// One per database, repointed by a later start, as a database cannot drop a hook.
struct Attached(Mutex<Weak<State>>);

/// With the backoff's doubling from 100 ms, about 20 s before giving up.
const DROP_RETRIES: usize = 8;

#[cfg(feature = "memory")]
const NEAR_ENTRIES: std::num::NonZeroUsize = std::num::NonZeroUsize::new(10_000).unwrap();
#[cfg(feature = "memory")]
const NEAR_BYTES: usize = 64 * 1024 * 1024;

impl Cache {
    /// Every commit through `db`, or a clone of it, then drops what depended on what it wrote.
    pub async fn start(db: &Db, store: Store) -> Result<Self> {
        let attached = sofie_db::commit_hook::<Attached>(db);
        if attached
            .as_ref()
            .and_then(|attached| attached.state())
            .is_some()
        {
            return Err(Error::internal_message(
                "a cache is already started on this database",
            ));
        }
        let parts = Parts::of(db, store).await?;
        #[cfg(feature = "memory")]
        let notifications = crate::peers::subscribed(&parts.peers).await?;
        let state = Arc::new(State {
            #[cfg(feature = "memory")]
            near: crate::memory::Memory::new(NEAR_ENTRIES, NEAR_BYTES),
            shared: parts.shared,
            #[cfg(feature = "memory")]
            peers: Arc::clone(&parts.peers),
            flights: Flights::default(),
            invalidations: Invalidations::default(),
            pending: Pending::default(),
            #[cfg(feature = "memory")]
            instance: sofie_core::Uuid::new_v4()
                .map_err(|error| Error::internal("naming the cache instance", error))?
                .to_string(),
        });
        #[cfg(feature = "memory")]
        if let Some(notifications) = notifications {
            tokio::spawn(crate::peers::listen(
                Arc::downgrade(&state),
                parts.peers,
                notifications,
            ));
        }
        let weak = Arc::downgrade(&state);
        match attached {
            Some(attached) => *unpoisoned::lock(&attached.0) = weak,
            None => sofie_db::add_commit_hook(db, Arc::new(Attached(Mutex::new(weak)))),
        }
        Ok(Self { state })
    }

    pub async fn from_environment(db: &Db) -> Result<Self> {
        Self::start(db, Store::from_environment()?).await
    }

    #[must_use]
    pub fn entry<T: Serialize + DeserializeOwned>(&self, key: &Key<T>) -> Entry<'_, T> {
        Entry::new(&self.state, key.text())
    }

    fn of(db: &Db) -> Result<Self> {
        let state = sofie_db::commit_hook::<Attached>(db).and_then(|attached| attached.state());
        state.map(|state| Self { state }).ok_or_else(|| {
            Error::internal_message(
                "no cache is started on this database; enable sofie's `cache` feature, or start one with `Cache::start`",
            )
        })
    }
}

impl<C: Context> Resolve<C> for Cache {
    fn resolve(context: &mut C) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(Self::of(context.db()))
    }
}

struct Parts {
    shared: Option<Arc<dyn SharedStore>>,
    #[cfg(feature = "memory")]
    peers: Arc<crate::peers::Peers>,
}

impl Parts {
    #[cfg_attr(
        not(any(feature = "memory", feature = "database")),
        expect(unused_variables)
    )]
    #[cfg_attr(
        not(any(feature = "database", feature = "redis")),
        expect(clippy::unused_async_trait_impl)
    )]
    async fn of(db: &Db, store: Store) -> Result<Self> {
        #[cfg(feature = "memory")]
        let peers = Arc::new(match sofie_db::cache_driver(db).dialect().engine() {
            sofie_db::Engine::Sqlite => crate::peers::Peers::Alone,
            sofie_db::Engine::Postgres => crate::peers::Peers::database(db),
        });
        match store {
            #[cfg(feature = "memory")]
            Store::Memory => Ok(Self {
                shared: None,
                peers,
            }),
            #[cfg(feature = "database")]
            Store::Database => Ok(Self {
                shared: Some(crate::database::Database::start(db).await? as Arc<dyn SharedStore>),
                #[cfg(feature = "memory")]
                peers,
            }),
            #[cfg(feature = "redis")]
            Store::Redis(url) => {
                let client = Arc::new(crate::redis::Client::connect(url).await?);
                Ok(Self {
                    #[cfg(feature = "memory")]
                    peers: Arc::new(crate::peers::Peers::Redis(Arc::clone(&client))),
                    shared: Some(client as Arc<dyn SharedStore>),
                })
            }
        }
    }
}

impl Attached {
    fn state(&self) -> Option<Arc<State>> {
        unpoisoned::lock(&self.0).upgrade()
    }
}

impl CommitHook for Attached {
    /// This instance's memory forgets at once; the shared store and other instances are told off
    /// the request's path, so its latency excludes their round trips.
    fn committed(&self, changes: &[ChangedRecord]) {
        let Some(state) = self.state() else {
            return;
        };
        let tags = tags_of(changes);
        state.forget_tagged(&tags);
        if state.reaches_elsewhere() && state.pending.add(&tags) {
            tokio::spawn(drop_elsewhere(state));
        }
    }
}

fn tags_of(changes: &[ChangedRecord]) -> Vec<String> {
    let mut tags = Vec::with_capacity(changes.len() * 2);
    for change in changes {
        let whole = aggregate_tag(change.module, change.aggregate);
        if !tags.contains(&whole) {
            tags.push(whole);
        }
        tags.push(match change.id {
            Some(id) => record_tag(change.module, change.aggregate, id),
            None => any_record_tag(change.module, change.aggregate),
        });
    }
    tags
}

/// The first try holds the cache: the shared store must hear of a commit even if it goes away.
async fn drop_elsewhere(state: Arc<State>) {
    let weak = Arc::downgrade(&state);
    let mut held = Some(state);
    let mut backoff = Backoff::default();
    let mut failures = 0;
    loop {
        let Some(state) = held.take().or_else(|| weak.upgrade()) else {
            return;
        };
        let Some(tags) = state.pending.next() else {
            return;
        };
        let Err(error) = state.drop_elsewhere(&tags).await else {
            state.pending.sent();
            backoff.succeeded();
            failures = 0;
            held = Some(state);
            continue;
        };
        failures += 1;
        if failures > DROP_RETRIES {
            tracing::error!(error = %error, cache.tags = tags.len(), "gave up dropping cache entries that depended on commits; they are served until they expire");
            state.pending.sent();
            failures = 0;
            continue;
        }
        tracing::warn!(error = %error, cache.tags = tags.len(), "dropping cache entries that depended on commits failed; retrying");
        drop(state);
        tokio::time::sleep(backoff.failed()).await;
    }
}

impl State {
    /// A value read from the shared store is held in memory only if nothing dropped it meanwhile.
    pub(crate) async fn lookup(&self, key: &str) -> Result<Option<Arc<str>>> {
        #[cfg(feature = "memory")]
        if let Some(value) = self.near.get(key) {
            telemetry::looked_up("memory", true);
            return Ok(Some(value));
        }
        let Some(shared) = &self.shared else {
            telemetry::looked_up("memory", false);
            return Ok(None);
        };
        #[cfg(feature = "memory")]
        let epoch = self.invalidations.epoch();
        let found = telemetry::call(shared.name(), Operation::Get, shared.get(key)).await?;
        telemetry::looked_up(shared.name(), found.is_some());
        let Some(stored) = found.filter(|stored| !self.pending.any(&stored.tags)) else {
            return Ok(None);
        };
        let value = stored.value;
        #[cfg(feature = "memory")]
        if !self.invalidations.touched_since(epoch, key, &stored.tags) {
            let expires_at = tokio::time::Instant::now() + stored.expires_in;
            self.near
                .set(key.to_owned(), Arc::clone(&value), stored.tags, expires_at);
        }
        Ok(Some(value))
    }

    /// `announce` is for a value set rather than computed: it drops other instances' copies, and
    /// keeps this one's computations and lookups that read older data meanwhile from storing it.
    pub(crate) async fn write(&self, key: &str, stored: Stored, announce: bool) -> Result<()> {
        if announce {
            self.invalidations.record(&[key.to_owned()]);
        }
        let shared_written = match &self.shared {
            Some(shared) => {
                telemetry::call(shared.name(), Operation::Set, shared.set(key, &stored)).await
            }
            None => Ok(()),
        };
        if announce {
            self.invalidations.record(&[key.to_owned()]);
        }
        #[cfg(feature = "memory")]
        {
            let expires_at = tokio::time::Instant::now() + stored.expires_in;
            let value = stored.value;
            self.near
                .set(key.to_owned(), value, stored.tags, expires_at);
            if announce {
                self.announce(vec![key.to_owned()], Vec::new()).await;
            }
        }
        #[cfg(not(feature = "memory"))]
        let _ = announce;
        shared_written
    }

    pub(crate) async fn delete(&self, key: &str) -> Result<()> {
        let keys = vec![key.to_owned()];
        self.invalidations.record(&keys);
        let shared_deleted = match &self.shared {
            Some(shared) => {
                telemetry::call(shared.name(), Operation::Delete, shared.delete(&keys)).await
            }
            None => Ok(()),
        };
        self.invalidations.record(&keys);
        #[cfg(feature = "memory")]
        self.near.delete(&keys);
        shared_deleted?;
        #[cfg(feature = "memory")]
        self.announce(keys, Vec::new()).await;
        Ok(())
    }

    fn forget_tagged(&self, tags: &[String]) {
        self.invalidations.record(tags);
        #[cfg(feature = "memory")]
        self.near.drop_tagged(tags);
    }

    fn reaches_elsewhere(&self) -> bool {
        #[cfg(feature = "memory")]
        if !matches!(*self.peers, crate::peers::Peers::Alone) {
            return true;
        }
        self.shared.is_some()
    }

    /// Announced even when the shared store failed, so other instances' memory copies go.
    async fn drop_elsewhere(&self, tags: &[String]) -> Result<()> {
        let dropped = match &self.shared {
            Some(shared) => {
                let dropping = shared.drop_tagged(tags);
                telemetry::call(shared.name(), Operation::DropTagged, dropping).await
            }
            None => Ok(()),
        };
        #[cfg(feature = "memory")]
        self.announce(Vec::new(), tags.to_vec()).await;
        dropped
    }

    /// Other instances keep a stale copy until it expires should this fail, which the warning says.
    #[cfg(feature = "memory")]
    async fn announce(&self, keys: Vec<String>, tags: Vec<String>) {
        let dropped = crate::peers::Dropped {
            from: self.instance.clone(),
            keys,
            tags,
            everything: false,
        };
        if let Err(error) = self.peers.publish(&dropped).await {
            tracing::warn!(error = %error, "telling other instances what the cache dropped failed");
        }
    }

    #[cfg(feature = "memory")]
    pub(crate) fn received(&self, dropped: &crate::peers::Dropped) {
        if dropped.from == self.instance {
            return;
        }
        if dropped.everything {
            self.drop_everything();
            return;
        }
        self.invalidations.record(&dropped.keys);
        self.invalidations.record(&dropped.tags);
        self.near.delete(&dropped.keys);
        self.near.drop_tagged(&dropped.tags);
    }

    #[cfg(feature = "memory")]
    pub(crate) fn drop_everything(&self) {
        self.invalidations.record_everything();
        self.near.clear();
    }
}

#[cfg(test)]
mod tests {
    use sofie_core::Uuid;
    use sofie_db::ChangedRecord;

    use super::tags_of;

    #[test]
    fn a_commit_that_cannot_name_its_records_drops_every_record_s_entries() {
        let id = Uuid::now_v7().unwrap();
        let named = ChangedRecord {
            module: "shop",
            aggregate: "address",
            id: Some(id),
        };
        let unnamed = ChangedRecord { id: None, ..named };

        assert_eq!(
            tags_of(&[named, unnamed]),
            [
                "shop.address".to_owned(),
                format!("shop.address/{id}"),
                "shop.address/*".to_owned()
            ]
        );
    }
}
