use sofie_core::{Error, Result};

use crate::{Aggregate, Id, Query, Read, Source};

pub trait Select: Source {
    #[must_use]
    fn query() -> Query<Self> {
        Query::default()
    }
}

impl<T: Source> Select for T {}

pub trait Load: Aggregate {
    fn find(read: &Read, id: Id<Self>) -> impl Future<Output = Result<Self>> + Send {
        async move {
            Self::query()
                .where_(Self::ID.eq(id))
                .first(read)
                .await?
                .ok_or(Error::NotFound)
        }
    }
}

impl<A: Aggregate> Load for A {}
