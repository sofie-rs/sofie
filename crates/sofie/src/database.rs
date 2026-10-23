//! The database `DATABASE_URL` names, of those the app was built with.

use sofie_core::Result;
use sofie_db::{Db, PoolSettings};

const SQLITE: &str = "sqlite:";

pub(crate) fn connect(url: &str, pool: PoolSettings) -> Result<Db> {
    if url.starts_with(SQLITE) {
        return sqlite(url);
    }
    postgres(url, pool)
}

/// The worker's own connections, which SQLite has no use for: it writes one at a time,
/// and an in-memory database is only the connection holding it.
pub(crate) fn connect_worker(url: &str, pool: PoolSettings) -> Result<Option<Db>> {
    if url.starts_with(SQLITE) {
        return Ok(None);
    }
    postgres(url, pool).map(Some)
}

#[cfg(feature = "sqlite")]
fn sqlite(url: &str) -> Result<Db> {
    sofie_sqlite::Sqlite::connect(url).map(Db::new)
}

#[cfg(not(feature = "sqlite"))]
fn sqlite(_url: &str) -> Result<Db> {
    Err(sofie_core::Error::internal_message(
        "DATABASE_URL names a SQLite database, but the app was built without sofie's `sqlite`",
    ))
}

#[cfg(feature = "postgres")]
fn postgres(url: &str, pool: PoolSettings) -> Result<Db> {
    sofie_postgres::Postgres::connect_with(url, pool).map(Db::new)
}

#[cfg(not(feature = "postgres"))]
fn postgres(_url: &str, _pool: PoolSettings) -> Result<Db> {
    Err(sofie_core::Error::internal_message(
        "DATABASE_URL names no SQLite database, and the app was built without sofie's `postgres`",
    ))
}

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use super::*;

    #[test]
    fn a_sqlite_url_opens_sqlite_and_keeps_the_worker_on_its_connection() {
        let pool = PoolSettings::default();

        assert!(connect("sqlite::memory:", pool).is_ok());
        assert!(connect_worker("sqlite::memory:", pool).unwrap().is_none());
        assert!(connect("sqlite:app.db", pool).is_err());
    }
}
