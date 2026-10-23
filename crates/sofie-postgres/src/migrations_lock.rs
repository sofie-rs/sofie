//! The migrations lock, held by one session across a whole `Schema::migrate`.
//!
//! A process waiting in `pg_advisory_lock` holds a snapshot, which a `CREATE INDEX
//! CONCURRENTLY` run under the lock waits out: a deadlock. Waiting between tries holds none.

use std::time::Duration;

use sofie_core::Result;
use sofie_db::MigrationsLock;
use tokio_postgres::Client;

use crate::Postgres;
use crate::dialect::MIGRATIONS_LOCK;
use crate::error::{classified, statement_error};

const RETRY_AFTER: Duration = Duration::from_millis(100);

/// Outside the pool, so a pool of one still migrates, and the lock ends with the session.
struct Held(#[expect(dead_code, reason = "dropping it ends the session")] Client);

impl MigrationsLock for Held {}

#[cfg_attr(
    not(feature = "tls"),
    expect(
        clippy::clone_on_copy,
        reason = "`NoTls` is `Copy`, rustls's connector is not"
    )
)]
pub(crate) async fn lock_migrations(postgres: &Postgres) -> Result<Box<dyn MigrationsLock>> {
    let (client, connection) = postgres
        .config
        .connect(postgres.tls.clone())
        .await
        .map_err(|error| classified("opening the migrations lock connection", error))?;
    tokio::spawn(connection);
    let try_lock = MIGRATIONS_LOCK.replace("pg_advisory_xact_lock", "pg_try_advisory_lock");
    if !try_lock_once(&client, &try_lock).await? {
        tracing::info!("waiting for another process's migrations to finish");
        while !try_lock_once(&client, &try_lock).await? {
            tokio::time::sleep(RETRY_AFTER).await;
        }
    }
    Ok(Box::new(Held(client)))
}

async fn try_lock_once(client: &Client, sql: &str) -> Result<bool> {
    let row = client
        .query_one(sql, &[])
        .await
        .map_err(|error| statement_error(sql, error))?;
    Ok(row.get(0))
}
