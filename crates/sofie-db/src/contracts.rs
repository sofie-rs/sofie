#[cfg(feature = "jobs")]
mod jobs;
#[cfg(feature = "local-time")]
mod local;
mod mirror;
mod provided;

#[cfg(feature = "jobs")]
pub use jobs::*;
#[cfg(feature = "local-time")]
pub(crate) use local::fire_due;
#[cfg(feature = "local-time")]
pub use local::{LocalJobs, LocalScheduled, Repeat};
pub use mirror::{Backfilled, Mirror, MirrorPage, MirrorRow, backfill_mirror, changed_field};
pub(crate) use mirror::{SWEEP_FORGOTTEN, sweep_forgotten};
pub(crate) use provided::Provided;

use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::Arc;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use sofie_core::{Error, Result, Uuid};

use crate::{Aggregate, BoxFuture, Db, Id, Resolve, Select, Writes};

pub struct Dep<T: ?Sized>(Arc<T>);

impl<T: ?Sized> Deref for Dep<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<C: Writes, T: ?Sized + Send + Sync + 'static> Resolve<C> for Dep<T> {
    fn resolve(context: &mut C) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(provided(context.db()).map(Self).ok_or_else(|| {
            Error::internal_message(format!(
                "nothing provides {}; run `sofie gen` so the wiring does",
                std::any::type_name::<T>()
            ))
        }))
    }
}

pub struct Extensions<T: ?Sized>(Arc<Vec<Arc<T>>>);

impl<T: ?Sized> Deref for Extensions<T> {
    type Target = [Arc<T>];

    fn deref(&self) -> &[Arc<T>] {
        &self.0
    }
}

impl<C: Writes, T: ?Sized + Send + Sync + 'static> Resolve<C> for Extensions<T> {
    fn resolve(context: &mut C) -> impl Future<Output = Result<Self>> + Send {
        let provided = context.db().provided.get::<Vec<Arc<T>>>();
        std::future::ready(Ok(Self(provided.unwrap_or_default())))
    }
}

pub fn provide<T: ?Sized + Send + Sync + 'static>(db: &Db, implementation: Arc<T>) {
    db.provided
        .change(|slot: &mut Option<Arc<T>>| *slot = Some(implementation));
}

/// For a remote contract's server, and a package's arguments that resolve in a GET too.
#[must_use]
pub fn provided<T: ?Sized + Send + Sync + 'static>(db: &Db) -> Option<Arc<T>> {
    db.provided
        .get::<Option<Arc<T>>>()
        .and_then(|slot| slot.as_ref().clone())
}

pub fn extend<T: ?Sized + Send + Sync + 'static>(db: &Db, extension: Arc<T>) {
    db.provided
        .change(|extensions: &mut Vec<Arc<T>>| extensions.push(extension));
}

pub trait Lookup<D>: Send + Sync {
    fn get_many<'a>(&'a self, ids: &'a [Uuid]) -> BoxFuture<'a, Result<Vec<D>>>;

    /// Records past `after` by id as change events carry them, deleted ones left out.
    fn changes(
        &self,
        after: Option<Uuid>,
        limit: u64,
    ) -> BoxFuture<'_, Result<Vec<Map<String, Value>>>> {
        let _ = (after, limit);
        Box::pin(std::future::ready(Err(Error::internal_message(
            "this lookup cannot list its records, so a mirror of it fills only from its change \
             events",
        ))))
    }
}

pub struct Exposed<A, D> {
    db: Db,
    types: PhantomData<fn() -> (A, D)>,
}

impl<A, D> Exposed<A, D> {
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self {
            db,
            types: PhantomData,
        }
    }
}

/// Exposed fields are public to every module, so a lookup reaches every record.
impl<A: Aggregate + Serialize, D: DeserializeOwned + Send> Lookup<D> for Exposed<A, D> {
    fn get_many<'a>(&'a self, ids: &'a [Uuid]) -> BoxFuture<'a, Result<Vec<D>>> {
        Box::pin(async move {
            let ids = ids.iter().copied().map(Id::from_uuid);
            let records = A::query()
                .where_(A::ID.in_(ids))
                .all(&self.db.read())
                .await?;
            records.iter().map(exposed).collect()
        })
    }

    fn changes(
        &self,
        after: Option<Uuid>,
        limit: u64,
    ) -> BoxFuture<'_, Result<Vec<Map<String, Value>>>> {
        Box::pin(async move {
            let query = A::query().order_by(A::ID.asc()).limit(limit);
            let query = match after {
                Some(after) => query.where_(A::ID.gt(Id::from_uuid(after))),
                None => query,
            };
            let records = query.all(&self.db.read()).await?;
            records.iter().map(change_of).collect()
        })
    }
}

fn change_of<A: Aggregate>(aggregate: &A) -> Result<Map<String, Value>> {
    let change = aggregate.exposed_fields().ok_or_else(|| {
        Error::internal_message(format!("`{}` exposes no fields to change events", A::NAME))
    })??;
    let meta = aggregate.meta();
    Ok(crate::change_event::change_payload(
        change,
        meta.id(),
        meta.version(),
    ))
}

pub fn exposed<A: Aggregate + Serialize, D: DeserializeOwned>(aggregate: &A) -> Result<D> {
    let fields = serde_json::to_value(aggregate)
        .map_err(|error| Error::internal(format!("serializing a {}", A::NAME), error))?;
    serde_json::from_value(fields).map_err(|error| {
        Error::internal(
            format!("reading the exposed fields of a {}", A::NAME),
            error,
        )
    })
}

pub fn exposed_fields<A: Aggregate + Serialize>(
    aggregate: &A,
    names: &[&str],
) -> Result<Map<String, Value>> {
    let Value::Object(mut fields) = serde_json::to_value(aggregate)
        .map_err(|error| Error::internal(format!("serializing a {}", A::NAME), error))?
    else {
        return Err(Error::internal_message(format!(
            "a {} serializes to no object",
            A::NAME
        )));
    };
    fields.retain(|name, _| names.contains(&name.as_str()));
    Ok(fields)
}
