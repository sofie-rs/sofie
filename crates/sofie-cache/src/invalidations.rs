use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;

use sofie_core::unpoisoned;

/// Past this many, the oldest drops are forgotten and computations older than them not stored.
const REMEMBERED: usize = 16_384;

/// When each key and tag was last dropped, so a value read before a drop is not stored after it.
#[derive(Default)]
pub(crate) struct Invalidations {
    inner: Mutex<Recent>,
}

#[derive(Default)]
struct Recent {
    epoch: u64,
    last_dropped: HashMap<String, u64>,
    /// In drop order; an entry whose name was dropped again later is stale and skipped.
    order: VecDeque<(u64, String)>,
    everything_at: u64,
    forgotten_until: u64,
}

impl Invalidations {
    fn recent(&self) -> std::sync::MutexGuard<'_, Recent> {
        unpoisoned::lock(&self.inner)
    }

    pub(crate) fn epoch(&self) -> u64 {
        self.recent().epoch
    }

    pub(crate) fn record(&self, names: &[String]) {
        if names.is_empty() {
            return;
        }
        let mut recent = self.recent();
        recent.epoch += 1;
        let epoch = recent.epoch;
        for name in names {
            recent.last_dropped.insert(name.clone(), epoch);
            recent.order.push_back((epoch, name.clone()));
        }
        recent.forget_oldest();
    }

    #[cfg(feature = "memory")]
    pub(crate) fn record_everything(&self) {
        let mut recent = self.recent();
        recent.epoch += 1;
        recent.everything_at = recent.epoch;
    }

    /// Unsure counts as touched: an entry is better missing than stale.
    pub(crate) fn touched_since(&self, epoch: u64, key: &str, tags: &[String]) -> bool {
        let recent = self.recent();
        if recent.epoch == epoch {
            return false;
        }
        let dropped_since = |name: &str| {
            recent
                .last_dropped
                .get(name)
                .is_some_and(|dropped| *dropped > epoch)
        };
        recent.everything_at > epoch
            || recent.forgotten_until > epoch
            || dropped_since(key)
            || tags.iter().any(|tag| dropped_since(tag))
    }
}

impl Recent {
    fn forget_oldest(&mut self) {
        while self.order.len() > REMEMBERED {
            let Some((epoch, name)) = self.order.pop_front() else {
                return;
            };
            if self.last_dropped.get(&name) == Some(&epoch) {
                self.last_dropped.remove(&name);
                self.forgotten_until = epoch;
            }
        }
    }
}

/// Tags whose drop has not reached the shared store yet, so a value read there under one is a
/// miss. One task sends them merged, so an outage holds one retry loop, not one per commit.
#[derive(Default)]
pub(crate) struct Pending {
    tags: Mutex<PendingTags>,
}

#[derive(Default)]
struct PendingTags {
    waiting: HashSet<String>,
    sending: HashSet<String>,
    sender_running: bool,
}

impl Pending {
    fn tags(&self) -> std::sync::MutexGuard<'_, PendingTags> {
        unpoisoned::lock(&self.tags)
    }

    /// True when no task sends them yet, so the caller starts one.
    pub(crate) fn add(&self, tags: &[String]) -> bool {
        let mut pending = self.tags();
        pending.waiting.extend(tags.iter().cloned());
        !std::mem::replace(&mut pending.sender_running, true)
    }

    /// What to send next, a failed send's tags included; `None` ends the sending task.
    pub(crate) fn next(&self) -> Option<Vec<String>> {
        let mut pending = self.tags();
        let waiting = std::mem::take(&mut pending.waiting);
        pending.sending.extend(waiting);
        if pending.sending.is_empty() {
            pending.sender_running = false;
            return None;
        }
        Some(pending.sending.iter().cloned().collect())
    }

    pub(crate) fn sent(&self) {
        self.tags().sending.clear();
    }

    pub(crate) fn any(&self, tags: &[String]) -> bool {
        let pending = self.tags();
        tags.iter()
            .any(|tag| pending.waiting.contains(tag) || pending.sending.contains(tag))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn only_a_drop_of_the_key_or_one_of_its_tags_since_then_touches_it() {
        let invalidations = Invalidations::default();
        invalidations.record(&names(&["before"]));
        let epoch = invalidations.epoch();
        let tags = names(&["order.orders"]);
        assert!(!invalidations.touched_since(epoch, "key", &tags));

        invalidations.record(&names(&["catalog.products"]));
        assert!(!invalidations.touched_since(epoch, "key", &tags));
        invalidations.record(&names(&["order.orders"]));
        assert!(invalidations.touched_since(epoch, "key", &tags));
    }

    #[test]
    fn a_drop_too_old_to_remember_counts_as_touching() {
        let invalidations = Invalidations::default();
        let epoch = invalidations.epoch();
        for index in 0..=REMEMBERED {
            invalidations.record(&names(&[&format!("elsewhere:{index}")]));
        }

        assert!(invalidations.touched_since(epoch, "key", &[]));
    }

    #[test]
    fn many_drops_of_other_names_do_not_touch_a_key() {
        let invalidations = Invalidations::default();
        let epoch = invalidations.epoch();
        for index in 0..1_000 {
            invalidations.record(&names(&[&format!("catalog.products:{index}")]));
        }

        assert!(!invalidations.touched_since(epoch, "key", &names(&["order.orders"])));
    }

    #[test]
    fn an_empty_drop_moves_no_epoch() {
        let invalidations = Invalidations::default();
        let epoch = invalidations.epoch();
        invalidations.record(&[]);

        assert_eq!(invalidations.epoch(), epoch);
    }

    #[test]
    fn drops_added_while_one_is_sent_or_retried_merge_into_the_next_send() {
        let pending = Pending::default();
        assert!(pending.add(&names(&["a"])));
        assert_eq!(pending.next().unwrap(), names(&["a"]));
        assert!(!pending.add(&names(&["b"])));
        assert!(!pending.add(&names(&["b"])));

        let mut retried = pending.next().unwrap();
        retried.sort();
        assert_eq!(retried, names(&["a", "b"]));
        assert!(pending.any(&names(&["b"])));
        pending.sent();

        assert!(pending.next().is_none());
        assert!(!pending.any(&names(&["a", "b"])));
        assert!(pending.add(&names(&["c"])));
    }

    #[cfg(feature = "memory")]
    #[test]
    fn dropping_everything_touches_every_key() {
        let invalidations = Invalidations::default();
        let epoch = invalidations.epoch();
        invalidations.record_everything();

        assert!(invalidations.touched_since(epoch, "key", &[]));
    }
}
