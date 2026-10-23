use std::any::Any;
use std::sync::{Arc, RwLock};

use sofie_core::{Uuid, unpoisoned};

use crate::{Db, Driver};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChangedRecord {
    pub module: &'static str,
    pub aggregate: &'static str,
    /// `None` when the commit changed records it cannot name, such as the default it cleared.
    pub id: Option<Uuid>,
}

/// Runs after a commit that wrote aggregates, which already stands, so it cannot fail it. It runs
/// inline, so a caller cancelled once the commit stands cannot skip it; slow work goes to a task.
pub trait CommitHook: Any + Send + Sync {
    fn committed(&self, changes: &[ChangedRecord]);
}

/// Seen by every clone of `db`, made before or after.
pub fn add_commit_hook(db: &Db, hook: Arc<dyn CommitHook>) {
    unpoisoned::write(&db.commit_hooks.0).push(hook);
}

#[must_use]
pub fn commit_hook<T: CommitHook>(db: &Db) -> Option<Arc<T>> {
    let hooks = unpoisoned::read(&db.commit_hooks.0);
    hooks.iter().find_map(|hook| {
        let hook: Arc<dyn Any + Send + Sync> = hook.clone();
        hook.downcast().ok()
    })
}

/// For a cache keeping a table of its own and notifying other instances.
#[must_use]
pub fn cache_driver(db: &Db) -> &dyn Driver {
    db.driver()
}

#[derive(Default)]
pub(crate) struct CommitHooks(RwLock<Vec<Arc<dyn CommitHook>>>);

impl CommitHooks {
    pub(crate) fn run(&self, changes: &[ChangedRecord]) {
        if changes.is_empty() {
            return;
        }
        let hooks = unpoisoned::read(&self.0);
        for hook in hooks.iter() {
            hook.committed(changes);
        }
    }
}
