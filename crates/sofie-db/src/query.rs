use std::marker::PhantomData;

use sofie_core::Result;

use crate::statement::{Criteria, SortKey};
use crate::{Condition, Read, Sort, Source};

pub struct Query<T: Source> {
    pub(crate) criteria: Criteria,
    marker: PhantomData<fn() -> T>,
}

impl<T: Source> Clone for Query<T> {
    fn clone(&self) -> Self {
        Self {
            criteria: self.criteria.clone(),
            marker: PhantomData,
        }
    }
}

impl<T: Source> Default for Query<T> {
    fn default() -> Self {
        Self {
            criteria: Criteria::default(),
            marker: PhantomData,
        }
    }
}

impl<T: Source> Query<T> {
    #[must_use]
    pub fn where_(mut self, condition: Condition<T::Base>) -> Self {
        self.criteria.conditions.push(condition.kind);
        self
    }

    #[must_use]
    pub fn scope(self, scope: impl FnOnce(Self) -> Self) -> Self {
        scope(self)
    }

    #[must_use]
    pub fn order_by(mut self, sort: Sort<T::Base>) -> Self {
        self.criteria.sorts.push(sort.key);
        self
    }

    /// Ids are `UUIDv7`, so they sort by creation time.
    #[must_use]
    pub fn latest(mut self) -> Self {
        self.criteria.sorts.push(SortKey::id(true));
        self
    }

    #[must_use]
    pub fn limit(mut self, limit: u64) -> Self {
        self.criteria.limit = Some(limit);
        self
    }

    pub async fn all(self, read: &Read) -> Result<Vec<T>> {
        self.criteria.all(read).await
    }

    pub async fn first(self, read: &Read) -> Result<Option<T>> {
        Ok(self.limit(1).all(read).await?.into_iter().next())
    }

    pub async fn count(self, read: &Read) -> Result<u64> {
        self.criteria.count::<T::Base>(read).await
    }

    pub async fn exists(self, read: &Read) -> Result<bool> {
        self.criteria.exists::<T::Base>(read).await
    }

    #[cfg(feature = "streams")]
    pub fn stream(self, read: &Read) -> Result<crate::Stream<T>> {
        crate::Stream::new(self.criteria, read)
    }
}

#[cfg(feature = "soft-delete")]
impl<T: Source> Query<T>
where
    T::Base: crate::SoftDeletes,
{
    #[must_use]
    pub fn with_trashed(mut self) -> Self {
        self.criteria.with_hidden = true;
        self
    }
}
