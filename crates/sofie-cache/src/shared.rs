use std::sync::Arc;
use std::time::Duration;

use sofie_core::Result;
use sofie_db::BoxFuture;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Stored {
    /// Shared with the memory tier, so a read copies it once.
    pub(crate) value: Arc<str>,
    pub(crate) tags: Vec<String>,
    /// Left until it expires: the whole expiry when written, what remains when read.
    pub(crate) expires_in: Duration,
}

/// A store every instance reaches, which therefore needs no message to stay coherent.
pub(crate) trait SharedStore: Send + Sync {
    fn name(&self) -> &'static str;

    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<Option<Stored>>>;

    fn set<'a>(&'a self, key: &'a str, stored: &'a Stored) -> BoxFuture<'a, Result<()>>;

    fn delete<'a>(&'a self, keys: &'a [String]) -> BoxFuture<'a, Result<()>>;

    fn drop_tagged<'a>(&'a self, tags: &'a [String]) -> BoxFuture<'a, Result<()>>;
}

#[cfg(any(feature = "database", feature = "redis"))]
pub(crate) fn milliseconds(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}
