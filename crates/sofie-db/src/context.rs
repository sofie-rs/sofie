use serde_json::Value;
use sofie_core::{Clock, Error, Result, Uuid};

use crate::{Aggregate, Db, Id, InputValue, Reach, Read, Select, Source, optional_lone_value};

pub trait Resolve<C>: Sized {
    fn resolve(context: &mut C) -> impl Future<Output = Result<Self>> + Send;
}

pub trait Context: Send {
    fn db(&self) -> &Db;

    fn reach(&self) -> Reach;

    /// The only way a handler gets a `Read`, so it never reaches further than its context.
    fn read(&self) -> Read {
        Read::within(self.db(), self.reach())
    }

    fn clock(&self) -> Clock {
        self.db().clock()
    }

    fn bound(&self, name: &str) -> Result<Uuid> {
        Err(Error::internal_message(format!(
            "nothing here names a record, so it cannot take a `{name}`"
        )))
    }

    fn input(&mut self) -> Result<Value> {
        Err(Error::internal_message(
            "nothing here carries an input, so it cannot take one",
        ))
    }

    fn event(&self, name: &str) -> Result<Value> {
        Err(Error::internal_message(format!(
            "only a listener receives an event, so it cannot take a `{name}`"
        )))
    }
}

/// A GET's context leaves it out, so a GET cannot take a write-side argument.
pub trait Writes: Context {}

/// Named by the route segment after the aggregate, and within the parents the context names.
pub async fn bind<S: Source, C: Context>(context: &mut C) -> Result<S> {
    let id: Id<S::Base> = Id::from_uuid(context.bound(<S::Base as Aggregate>::NAME)?);
    let query = S::query().where_(<S::Base as Aggregate>::ID.eq(id));
    #[cfg(feature = "children")]
    let query = crate::children::through::within_parents(query, context);
    query.first(&context.read()).await?.ok_or(Error::NotFound)
}

impl<C: Context> Resolve<C> for Read {
    fn resolve(context: &mut C) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(Ok(context.read()))
    }
}

impl<C: Context> Resolve<C> for Clock {
    fn resolve(context: &mut C) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(Ok(context.clock()))
    }
}

impl<C: Writes, V: InputValue + Send> Resolve<C> for Option<V> {
    fn resolve(context: &mut C) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(context.input().and_then(optional_lone_value))
    }
}
