mod alone;
mod catalog;
mod dialect;
mod error;
#[cfg(feature = "remote-contracts")]
mod feed;
mod idle;
mod messages;
mod migrations_lock;
#[cfg(not(feature = "tls"))]
mod plain;
#[cfg(feature = "product-reports")]
mod product_events;
#[cfg(feature = "raw-sql")]
mod read_only;
mod rows;
#[cfg(feature = "shield")]
mod shield;
#[cfg(feature = "tls")]
mod tls;
mod transaction;
mod value;

use std::time::Duration;

use deadpool_postgres::{
    Manager, ManagerConfig, Pool, PoolError, RecyclingMethod, Runtime, TimeoutType,
};
use sofie_core::{Error, Result, SqlValue};
pub use sofie_db::PoolSettings;
use sofie_db::{
    BoxFuture, Dialect, Driver, MigrationsLock, RowSink, Step, Wakeups, traced_statement,
};
use tokio_postgres::Statement;

use crate::error::{classified, statement_error};
#[cfg(not(feature = "tls"))]
use crate::plain as tls;
use crate::rows::read_rows;

pub struct Postgres {
    pool: Pool,
    pool_wait: Duration,
    config: tokio_postgres::Config,
    tls: tls::Tls,
}

impl Postgres {
    /// Connections open on first use, encrypted as the url's `sslmode` asks: `disable`,
    /// `prefer` (the default), `require`, `verify-ca` or `verify-full`. A verifying mode trusts
    /// the PEM certificates in `sslrootcert=<path>` when it is given, the public roots otherwise.
    /// Without the `tls` feature only `disable` and `prefer` connect, in plain text.
    pub fn connect(url: &str) -> Result<Self> {
        Self::connect_with(url, PoolSettings::default())
    }

    #[cfg_attr(
        not(feature = "tls"),
        expect(
            clippy::clone_on_copy,
            reason = "`NoTls` is `Copy`, rustls's connector is not"
        )
    )]
    pub fn connect_with(url: &str, settings: PoolSettings) -> Result<Self> {
        let (url, tls) = tls::configure(url)?;
        let mut config = url
            .parse::<tokio_postgres::Config>()
            .map_err(|error| Error::internal("parsing the Postgres connection url", error))?;
        if config.get_keepalives_idle() > idle::KEEPALIVE_IDLE {
            config.keepalives_idle(idle::KEEPALIVE_IDLE);
        }
        let manager = Manager::from_config(
            config.clone(),
            tls.clone(),
            ManagerConfig {
                recycling_method: RecyclingMethod::Fast,
            },
        );
        let mut pool = Pool::builder(manager)
            .runtime(Runtime::Tokio1)
            .wait_timeout(Some(settings.wait));
        if let Some(connections) = settings.connections {
            pool = pool.max_size(connections.get());
        }
        let pool = pool
            .build()
            .map_err(|error| Error::internal("building the Postgres pool", error))?;
        idle::close_idle_in_background(&pool);
        Ok(Self {
            pool,
            pool_wait: settings.wait,
            config,
            tls,
        })
    }

    async fn connection(&self) -> Result<deadpool_postgres::Object> {
        self.pool.get().await.map_err(|error| match error {
            PoolError::Timeout(TimeoutType::Wait) => Error::Unavailable(format!(
                "no database connection came free within {} ms",
                self.pool_wait.as_millis()
            )),
            PoolError::Backend(error) => classified("getting a Postgres connection", error),
            other => Error::internal("getting a Postgres connection", other),
        })
    }

    async fn query_rows(
        &self,
        sql: &str,
        params: &[SqlValue],
        rows: &mut dyn RowSink,
    ) -> Result<()> {
        let client = self.connection().await?;
        let query = async {
            let statement = prepared(&client, sql).await?;
            read_rows(&client, &statement, sql, params, rows).await
        };
        traced_statement(sql, query, |count| Some(*count))
            .await
            .map(drop)
    }
}

impl Driver for Postgres {
    fn dialect(&self) -> &dyn Dialect {
        self
    }

    fn query<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [SqlValue],
        rows: &'a mut dyn RowSink,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(self.query_rows(sql, params, rows))
    }

    #[cfg(feature = "raw-sql")]
    fn query_read_only<'a>(
        &'a self,
        role: Option<&'a str>,
        sql: &'a str,
        params: &'a [SqlValue],
        rows: &'a mut dyn RowSink,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(self.rows_read_only(role, sql, params, rows))
    }

    fn transaction(&self, steps: Vec<Step>) -> BoxFuture<'_, Result<()>> {
        Box::pin(self.run_in_transaction(steps))
    }

    fn lock_migrations(&self) -> Option<BoxFuture<'_, Result<Box<dyn MigrationsLock>>>> {
        Some(Box::pin(migrations_lock::lock_migrations(self)))
    }

    fn migrate_alone<'a>(
        &'a self,
        schema: &'a str,
        sql: &'a str,
        record: Step,
    ) -> std::result::Result<BoxFuture<'a, Result<()>>, Step> {
        alone::migration_alone(self, schema, sql, record)
    }

    fn catalog(&self) -> Option<&dyn sofie_db::Catalog> {
        Some(self)
    }

    fn wakeups(&self) -> Option<BoxFuture<'_, Result<Box<dyn Wakeups>>>> {
        Some(self.subscribe(messages::LISTEN))
    }

    #[cfg(feature = "shield")]
    fn shield_wakeups(&self) -> Option<BoxFuture<'_, Result<Box<dyn Wakeups>>>> {
        Some(self.subscribe(shield::LISTEN))
    }

    #[cfg(feature = "cache")]
    fn cache_notifications(
        &self,
    ) -> Option<BoxFuture<'_, Result<Box<dyn sofie_db::Notifications>>>> {
        Some(self.notifications("LISTEN sofie_cache"))
    }

    #[cfg(feature = "live")]
    fn change_notifications(
        &self,
    ) -> Option<BoxFuture<'_, Result<Box<dyn sofie_db::Notifications>>>> {
        Some(self.notifications("LISTEN sofie_live"))
    }
}

/// Enough for an app's statement texts; past it the connection starts over, as texts a request
/// shapes (listing filters) would otherwise keep a server-side statement each forever.
const STATEMENTS_KEPT: usize = 512;

async fn prepared(connection: &deadpool_postgres::Object, sql: &str) -> Result<Statement> {
    if connection.statement_cache.size() >= STATEMENTS_KEPT {
        connection.statement_cache.clear();
    }
    connection
        .prepare_cached(sql)
        .await
        .map_err(|error| statement_error(sql, error))
}
