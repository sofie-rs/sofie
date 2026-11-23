use std::sync::Arc;

use crate::{Db, Driver};

#[must_use]
pub fn with_replica(db: Db, replica: impl Driver) -> Db {
    #[cfg(feature = "metrics")]
    let replica = crate::metrics::Counted::new(replica);
    Db {
        replica: Some(Arc::new(replica)),
        ..db
    }
}

/// Not for a read answered again on a change notification, which a lagging replica would miss.
#[must_use]
pub fn on_replica(db: &Db) -> Db {
    #[cfg(feature = "live")]
    if crate::live::tracking() {
        return db.clone();
    }
    match &db.replica {
        Some(replica) => Db {
            driver: Arc::clone(replica),
            replica: None,
            ..db.clone()
        },
        None => db.clone(),
    }
}

#[must_use]
pub fn has_replica(db: &Db) -> bool {
    db.replica.is_some()
}
