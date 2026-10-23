use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use sofie_core::unpoisoned;

type Entries = HashMap<TypeId, Arc<dyn Any + Send + Sync>>;

/// Shared by every clone of one `Db`, so what is provided after a clone is seen by it too.
#[derive(Default)]
pub(crate) struct Provided(RwLock<Entries>);

impl Provided {
    pub(crate) fn get<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        let entries = unpoisoned::read(&self.0);
        entries.get(&TypeId::of::<T>())?.clone().downcast().ok()
    }

    pub(crate) fn change<T: Any + Send + Sync + Clone + Default>(
        &self,
        change: impl FnOnce(&mut T),
    ) {
        let mut entries = unpoisoned::write(&self.0);
        let entry = entries
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Arc::new(T::default()));
        let mut value = entry.downcast_ref::<T>().cloned().unwrap_or_default();
        change(&mut value);
        *entry = Arc::new(value);
    }
}
