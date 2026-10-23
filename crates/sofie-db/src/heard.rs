use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use serde_json::{Value, json};
use sofie_core::unpoisoned;

#[derive(Clone, Default)]
pub(crate) struct Listeners {
    pub(crate) by_event: HashMap<&'static str, Vec<&'static str>>,
    pub(crate) taking_pruned: HashSet<&'static str>,
}

impl Listeners {
    pub(crate) fn of(&self, event: &str) -> &[&'static str] {
        self.by_event.get(event).map_or(&[], Vec::as_slice)
    }

    #[cfg(feature = "retention")]
    pub(crate) fn hear(&self, event: &str) -> bool {
        !self.of(event).is_empty()
    }

    #[cfg(all(feature = "retention", feature = "contracts"))]
    pub(crate) fn hear_pruned(&self, event: &str) -> bool {
        self.of(event)
            .iter()
            .any(|listener| self.taking_pruned.contains(listener))
    }

    /// A pruned record's change event goes only to the listeners taking it.
    pub(crate) fn taking(&self, event: &str, payload: &Value) -> Cow<'_, [&'static str]> {
        let listeners = self.of(event);
        if !crate::event::is_pruned(payload) {
            return Cow::Borrowed(listeners);
        }
        let taking = listeners.iter().copied();
        Cow::Owned(
            taking
                .filter(|listener| self.taking_pruned.contains(listener))
                .collect(),
        )
    }

    fn merge(&mut self, other: &Self) {
        for (event, listeners) in &other.by_event {
            let ours = self.by_event.entry(event).or_default();
            for listener in listeners {
                if !ours.contains(listener) {
                    ours.push(listener);
                }
            }
        }
        self.taking_pruned.extend(&other.taking_pruned);
    }
}

#[derive(Clone, Default)]
pub(crate) struct Known {
    pub(crate) listeners: Listeners,
    pub(crate) jobs: HashSet<&'static str>,
}

impl Known {
    pub(crate) fn to_json(&self) -> Value {
        let events: Vec<&str> = self.listeners.by_event.keys().copied().collect();
        json!({ "event": events, "job": self.jobs })
    }
}

#[derive(Default)]
pub(crate) struct Heard(RwLock<Option<Arc<Known>>>);

impl Heard {
    pub(crate) fn add(&self, known: &Known) {
        let mut heard = unpoisoned::write(&self.0);
        let mut merged = heard.as_deref().cloned().unwrap_or_default();
        merged.listeners.merge(&known.listeners);
        merged.jobs.extend(&known.jobs);
        *heard = Some(Arc::new(merged));
    }

    pub(crate) fn known(&self) -> Option<Arc<Known>> {
        unpoisoned::read(&self.0).clone()
    }
}
