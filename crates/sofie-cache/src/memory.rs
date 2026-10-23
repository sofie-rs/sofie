use std::collections::{HashMap, HashSet, VecDeque};
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};

use sofie_core::unpoisoned;
use tokio::time::Instant;

/// Bounded by entry count and by the bytes of keys and values; a full cache evicts by the clock
/// (second chance) algorithm, so a read only sets a flag instead of reordering a list.
pub(crate) struct Memory {
    capacity: NonZeroUsize,
    byte_capacity: usize,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    entries: HashMap<String, Slot>,
    tagged: HashMap<String, HashSet<String>>,
    /// Holds each key once per write; a stale generation is skipped.
    clock: VecDeque<(String, u64)>,
    writes: u64,
    bytes: usize,
}

struct Slot {
    value: Arc<str>,
    tags: Vec<String>,
    expires_at: Instant,
    generation: u64,
    referenced: bool,
}

impl Memory {
    pub(crate) fn new(capacity: NonZeroUsize, byte_capacity: usize) -> Self {
        Self {
            capacity,
            byte_capacity,
            inner: Mutex::default(),
        }
    }

    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        unpoisoned::lock(&self.inner)
    }

    pub(crate) fn get(&self, key: &str) -> Option<Arc<str>> {
        let mut inner = self.inner();
        if inner.entries.get(key)?.expires_at <= Instant::now() {
            inner.remove(key);
            return None;
        }
        let slot = inner.entries.get_mut(key)?;
        slot.referenced = true;
        Some(Arc::clone(&slot.value))
    }

    pub(crate) fn set(&self, key: String, value: Arc<str>, tags: Vec<String>, expires_at: Instant) {
        let mut inner = self.inner();
        inner.remove(&key);
        let size = key.len() + value.len();
        if size > self.byte_capacity {
            return;
        }
        while inner.entries.len() >= self.capacity.get() || inner.bytes + size > self.byte_capacity
        {
            inner.evict_one();
        }
        inner.writes += 1;
        inner.bytes += size;
        let generation = inner.writes;
        for tag in &tags {
            inner
                .tagged
                .entry(tag.clone())
                .or_default()
                .insert(key.clone());
        }
        inner.clock.push_back((key.clone(), generation));
        if inner.clock.len() > self.capacity.get() * 2 {
            inner.compact_clock();
        }
        let slot = Slot {
            value,
            tags,
            expires_at,
            generation,
            referenced: false,
        };
        inner.entries.insert(key, slot);
    }

    pub(crate) fn delete(&self, keys: &[String]) {
        let mut inner = self.inner();
        for key in keys {
            inner.remove(key);
        }
    }

    pub(crate) fn drop_tagged(&self, tags: &[String]) {
        let mut inner = self.inner();
        for tag in tags {
            for key in inner.tagged.remove(tag).unwrap_or_default() {
                inner.remove(&key);
            }
        }
    }

    pub(crate) fn clear(&self) {
        *self.inner() = Inner::default();
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.inner().entries.len()
    }
}

impl Inner {
    fn remove(&mut self, key: &str) {
        let Some(slot) = self.entries.remove(key) else {
            return;
        };
        self.bytes -= key.len() + slot.value.len();
        for tag in &slot.tags {
            if let Some(keys) = self.tagged.get_mut(tag) {
                keys.remove(key);
                if keys.is_empty() {
                    self.tagged.remove(tag);
                }
            }
        }
    }

    fn evict_one(&mut self) {
        let now = Instant::now();
        while let Some((key, generation)) = self.clock.pop_front() {
            let Some(slot) = self.entries.get_mut(&key) else {
                continue;
            };
            if slot.generation != generation {
                continue;
            }
            if slot.referenced && slot.expires_at > now {
                slot.referenced = false;
                self.clock.push_back((key, generation));
                continue;
            }
            self.remove(&key);
            return;
        }
    }

    fn compact_clock(&mut self) {
        let entries = &self.entries;
        self.clock.retain(|(key, generation)| {
            entries
                .get(key)
                .is_some_and(|slot| slot.generation == *generation)
        });
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn later(seconds: u64) -> Instant {
        Instant::now() + Duration::from_secs(seconds)
    }

    fn memory(capacity: usize) -> Memory {
        Memory::new(NonZeroUsize::new(capacity).unwrap(), usize::MAX)
    }

    fn set(memory: &Memory, key: &str, tags: &[&str]) {
        let tags = tags.iter().map(ToString::to_string).collect();
        memory.set(key.to_owned(), Arc::from(key), tags, later(60));
    }

    #[test]
    fn a_full_cache_evicts_what_was_not_read_since_it_was_last_passed() {
        let memory = memory(2);
        set(&memory, "a", &[]);
        set(&memory, "b", &[]);
        assert!(memory.get("a").is_some());

        set(&memory, "c", &[]);

        assert!(memory.get("a").is_some());
        assert!(memory.get("b").is_none());
        assert!(memory.get("c").is_some());
        assert_eq!(memory.len(), 2);
    }

    #[test]
    fn a_cache_full_by_bytes_evicts_and_never_holds_a_value_past_the_bound() {
        let memory = Memory::new(NonZeroUsize::new(100).unwrap(), 12);
        for key in ["aaa", "bbb", "ccc"] {
            memory.set(key.to_owned(), Arc::from("xxx"), Vec::new(), later(60));
        }

        assert!(memory.get("aaa").is_none());
        assert!(memory.get("ccc").is_some());
        assert_eq!(memory.inner().bytes, 12);

        memory.set(
            "big".to_owned(),
            Arc::from("x".repeat(10)),
            Vec::new(),
            later(60),
        );
        assert!(memory.get("big").is_none());
        assert!(memory.get("ccc").is_some());
    }

    #[test]
    fn rewriting_one_key_never_grows_past_the_bound() {
        let memory = memory(3);
        for _ in 0..100 {
            set(&memory, "same", &["tag"]);
        }
        assert_eq!(memory.len(), 1);
        assert!(memory.inner().clock.len() <= 6);
    }

    #[tokio::test(start_paused = true)]
    async fn an_entry_is_gone_once_it_expires() {
        let memory = memory(4);
        memory.set("a".to_owned(), Arc::from("1"), Vec::new(), later(5));

        tokio::time::advance(Duration::from_secs(4)).await;
        assert!(memory.get("a").is_some());
        tokio::time::advance(Duration::from_secs(1)).await;

        assert!(memory.get("a").is_none());
        assert_eq!(memory.len(), 0);
    }

    #[test]
    fn dropping_a_tag_drops_every_entry_under_it_and_nothing_else() {
        let memory = memory(8);
        set(&memory, "a", &["order.orders"]);
        set(&memory, "b", &["order.orders", "order.orders/1"]);
        set(&memory, "c", &["catalog.products"]);

        memory.drop_tagged(&["order.orders".to_owned()]);

        assert!(memory.get("a").is_none());
        assert!(memory.get("b").is_none());
        assert!(memory.get("c").is_some());
        assert!(!memory.inner().tagged.contains_key("order.orders/1"));
    }
}
