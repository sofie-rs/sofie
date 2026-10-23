use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{LazyLock, RwLock};

use sofie_core::unpoisoned;

/// Bounded by the program's types, so never expiring.
pub(crate) struct TypeCache<K, V>(LazyLock<RwLock<HashMap<K, V>>>);

impl<K: Eq + Hash, V: Clone> TypeCache<K, V> {
    pub(crate) const fn new() -> Self {
        Self(LazyLock::new(RwLock::default))
    }

    /// Values are only added, so a poisoned lock still guards a whole map.
    pub(crate) fn get_or_insert_with(&self, key: K, build: impl FnOnce() -> V) -> V {
        let values = unpoisoned::read(&self.0);
        if let Some(value) = values.get(&key) {
            return value.clone();
        }
        drop(values);
        let value = build();
        let mut values = unpoisoned::write(&self.0);
        values.entry(key).or_insert(value).clone()
    }
}
