use std::cell::Cell;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use sofie_core::unpoisoned;
use tracing::Metadata;
use tracing::span::Id;

use crate::TraceParent;
use crate::context::Trace;
use crate::fields::Fields;

static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);
static NEXT_THREAD_START: AtomicUsize = AtomicUsize::new(0);
/// Ids tell nothing of order across shards, so a capture lists spans by this instead.
#[cfg(feature = "capture")]
static OPENED: AtomicU64 = AtomicU64::new(0);

const SHARD_BITS: u32 = 6;
const SHARDS: usize = 1 << SHARD_BITS;

thread_local! {
    static NEXT_SHARD: Cell<usize> = Cell::new(NEXT_THREAD_START.fetch_add(1, Ordering::Relaxed));
}

/// Holds a reference to its parent, so the spans around any open span stay open too.
pub(crate) struct SpanEntry {
    pub(crate) metadata: &'static Metadata<'static>,
    pub(crate) parent: Option<Id>,
    pub(crate) fields: Arc<Fields>,
    pub(crate) trace: Option<Trace>,
    #[cfg(feature = "product-events")]
    pub(crate) feature_flags: Option<crate::product::EvaluatedFlags>,
    #[cfg(feature = "product-events")]
    pub(crate) principal: Option<sofie_core::Uuid>,
    #[cfg(feature = "development")]
    pub(crate) opened: std::time::Instant,
    #[cfg(feature = "capture")]
    pub(crate) opened_as: u64,
    references: usize,
}

impl SpanEntry {
    pub(crate) fn new(
        metadata: &'static Metadata<'static>,
        parent: Option<Id>,
        fields: Fields,
        trace: Option<Trace>,
    ) -> Self {
        Self {
            metadata,
            parent,
            fields: Arc::new(fields),
            trace,
            #[cfg(feature = "product-events")]
            feature_flags: None,
            #[cfg(feature = "product-events")]
            principal: None,
            #[cfg(feature = "development")]
            opened: std::time::Instant::now(),
            #[cfg(feature = "capture")]
            opened_as: OPENED.fetch_add(1, Ordering::Relaxed),
            references: 1,
        }
    }
}

/// Each thread deals the spans it opens out over the shards in turn, and a span's id names its
/// shard, so threads rarely meet on one lock even as tasks move between them; keeping a thread's
/// spans in one shard measured worse, as a moved task then contends with its old thread. Ids are
/// never reused and a parent opens before its children, so walking up ends.
pub(crate) struct SpanStore {
    owner: u64,
    shards: Box<[Shard; SHARDS]>,
}

/// Aligned to a cache line, so two shards' locks never share one.
#[repr(align(128))]
#[derive(Default)]
struct Shard(Mutex<Spans>);

#[derive(Default)]
struct Spans {
    opened: u64,
    entries: HashMap<u64, SpanEntry, BuildHasherDefault<IdHasher>>,
}

/// Ids are already unique; one multiplication spreads their bits over the table.
#[derive(Default)]
struct IdHasher(u64);

impl Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.write_u64(self.0 << 8 | u64::from(*byte));
        }
    }

    fn write_u64(&mut self, id: u64) {
        self.0 = id.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
}

impl SpanStore {
    pub(crate) fn new() -> Self {
        Self {
            owner: NEXT_OWNER.fetch_add(1, Ordering::Relaxed),
            shards: Box::new(std::array::from_fn(|_| Shard::default())),
        }
    }

    pub(crate) fn owner(&self) -> u64 {
        self.owner
    }

    pub(crate) fn insert(&self, entry: SpanEntry) -> Id {
        let shard = NEXT_SHARD.with(|next| {
            let shard = next.get() % SHARDS;
            next.set(shard + 1);
            shard
        });
        let mut spans = self.lock(shard);
        spans.opened += 1;
        let id = spans.opened << SHARD_BITS | shard as u64;
        spans.entries.insert(id, entry);
        Id::from_u64(id)
    }

    pub(crate) fn with<T>(&self, id: &Id, read: impl FnOnce(&mut SpanEntry) -> T) -> Option<T> {
        let id = id.into_u64();
        self.lock(shard_of(id)).entries.get_mut(&id).map(read)
    }

    pub(crate) fn add_reference(&self, id: &Id) -> bool {
        self.with(id, |entry| entry.references += 1).is_some()
    }

    pub(crate) fn release(&self, id: &Id) -> Option<SpanEntry> {
        let id = id.into_u64();
        let mut spans = self.lock(shard_of(id));
        let entry = spans.entries.get_mut(&id)?;
        entry.references -= 1;
        if entry.references > 0 {
            return None;
        }
        spans.entries.remove(&id)
    }

    pub(crate) fn record(&self, id: &Id, recorded: Fields) {
        self.with(id, |entry| Arc::make_mut(&mut entry.fields).merge(recorded));
    }

    pub(crate) fn metadata(&self, id: &Id) -> Option<&'static Metadata<'static>> {
        self.with(id, |entry| entry.metadata)
    }

    pub(crate) fn adopt(&self, id: &Id, parent: Option<TraceParent>) -> Option<TraceParent> {
        self.with(id, |entry| {
            let trace = entry.trace.as_mut()?;
            if let Some(parent) = parent {
                trace.continue_from(parent);
            }
            Some(trace.traceparent())
        })?
    }

    pub(crate) fn nearest_trace(&self, id: &Id) -> Option<Trace> {
        let mut nearest = None;
        self.walk_up(Some(id), |entry| match entry.trace {
            Some(trace) => {
                nearest = Some(trace);
                ControlFlow::Break(())
            }
            None => ControlFlow::Continue(()),
        });
        nearest
    }

    pub(crate) fn scope(
        &self,
        innermost: Option<&Id>,
    ) -> (Vec<(&'static str, Arc<Fields>)>, Option<TraceParent>) {
        let mut scope = Vec::new();
        let mut nearest = None;
        self.walk_up(innermost, |entry| {
            scope.push((entry.metadata.name(), Arc::clone(&entry.fields)));
            if nearest.is_none() {
                nearest = entry.trace.map(|trace| trace.traceparent());
            }
            ControlFlow::Continue(())
        });
        scope.reverse();
        (scope, nearest)
    }

    #[cfg(feature = "capture")]
    pub(crate) fn each(&self, mut visit: impl FnMut(u64, &SpanEntry)) {
        for shard in 0..SHARDS {
            for (id, entry) in &self.lock(shard).entries {
                visit(*id, entry);
            }
        }
    }

    /// Each step locks only the shard of the span it reads.
    pub(crate) fn walk_up(
        &self,
        innermost: Option<&Id>,
        mut visit: impl FnMut(&SpanEntry) -> ControlFlow<()>,
    ) {
        let mut next = innermost.cloned();
        while let Some(id) = next {
            next = self
                .with(&id, |entry| match visit(entry) {
                    ControlFlow::Continue(()) => entry.parent.clone(),
                    ControlFlow::Break(()) => None,
                })
                .flatten();
        }
    }

    fn lock(&self, shard: usize) -> MutexGuard<'_, Spans> {
        unpoisoned::lock(&self.shards[shard].0)
    }
}

fn shard_of(id: u64) -> usize {
    const { assert!(SHARD_BITS <= u8::BITS) };
    usize::from(id.to_le_bytes()[0]) & (SHARDS - 1)
}
