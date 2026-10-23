use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;

use sofie_core::{Cast, Result, SqlType, SqlValue};
use sofie_telemetry::metrics::{self, Instrument, Kind};

use crate::{BoxFuture, Db, Dialect, Driver, MigrationsLock, RowSink, Step, Wakeups};

static STATEMENT_DURATION: Instrument = Instrument {
    name: "db.client.operation.duration",
    description: "How long a database statement took, by its operation.",
    kind: Kind::Duration,
    labels: &["db.operation.name"],
};
static CONNECTIONS_IN_USE: Instrument = Instrument {
    name: "db.client.connection.count",
    description: "Connections busy with a query or a transaction, in every pool.",
    kind: Kind::Gauge,
    labels: &[],
};

static OUTBOX_DEPTH: Instrument = Instrument {
    name: "messaging.outbox.depth",
    description: "Messages waiting to be handled, dead ones left out, counted up to 10000.",
    kind: Kind::Gauge,
    labels: &[],
};

static IN_USE: AtomicI64 = AtomicI64::new(0);
static WAITING: AtomicI64 = AtomicI64::new(0);
static COUNTING: AtomicBool = AtomicBool::new(false);
/// A backlog past this reads as this, so a poll never scans all of it.
const COUNTED_UP_TO: u32 = 10_000;

/// By the first word: SQL whose lists vary in length must not be a label.
pub(crate) fn statement(sql: &str, elapsed: Duration) {
    let first = sql.split_whitespace().next().unwrap_or_default();
    let operation = ["SELECT", "INSERT", "UPDATE", "DELETE", "WITH"]
        .into_iter()
        .find(|operation| first.eq_ignore_ascii_case(operation))
        .unwrap_or("OTHER");
    metrics::record_duration(&STATEMENT_DURATION, &[operation.into()], elapsed);
}

pub(crate) struct Counted<D>(pub(crate) D);

impl<D: Driver> Counted<D> {
    pub(crate) fn new(driver: D) -> Self {
        metrics::gauge(&CONNECTIONS_IN_USE, in_use);
        Self(driver)
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "a count of connections is far below 2^52"
)]
fn in_use() -> f64 {
    IN_USE.load(Ordering::Relaxed) as f64
}

pub(crate) fn watch_outbox(db: &Db) {
    let (db, runtime) = (db.clone(), tokio::runtime::Handle::current());
    metrics::gauge(&OUTBOX_DEPTH, waiting);
    metrics::poll(move || {
        let Some(counting) = Counting::start() else {
            return;
        };
        let db = db.clone();
        runtime.spawn(async move {
            let _counting = counting;
            match outbox_depth(&db).await {
                Ok(depth) => WAITING.store(depth, Ordering::Relaxed),
                Err(error) => tracing::warn!(error = %error, "counting the outbox failed"),
            }
        });
    });
}

async fn outbox_depth(db: &Db) -> Result<i64> {
    let messages = db.driver().dialect().qualified("sofie", "messages");
    let sql = format!(
        "SELECT count(*) FROM \
         (SELECT 1 FROM {messages} WHERE dead_at IS NULL LIMIT {COUNTED_UP_TO}) AS waiting"
    );
    let types = vec![Some(SqlType::Int)];
    let depths = crate::query_decoded(db.driver(), &sql, &[], types, i64::decode).await?;
    depths
        .first()
        .copied()
        .ok_or_else(|| sofie_core::Error::internal_message("counting the outbox answered no row"))
}

/// A slow count is not overlapped by the next poll's.
struct Counting;

impl Counting {
    fn start() -> Option<Self> {
        COUNTING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self)
    }
}

impl Drop for Counting {
    fn drop(&mut self) {
        COUNTING.store(false, Ordering::Release);
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "a count of messages is far below 2^52"
)]
fn waiting() -> f64 {
    WAITING.load(Ordering::Relaxed) as f64
}

async fn counted<T>(call: impl Future<Output = T>) -> T {
    IN_USE.fetch_add(1, Ordering::Relaxed);
    let _done = Released;
    call.await
}

/// Frees the count even when the call is dropped midway.
struct Released;

impl Drop for Released {
    fn drop(&mut self) {
        IN_USE.fetch_sub(1, Ordering::Relaxed);
    }
}

impl<D: Driver> Driver for Counted<D> {
    fn dialect(&self) -> &dyn Dialect {
        self.0.dialect()
    }

    fn query<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [SqlValue],
        rows: &'a mut dyn RowSink,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(counted(self.0.query(sql, params, rows)))
    }

    #[cfg(feature = "raw-sql")]
    fn query_read_only<'a>(
        &'a self,
        role: Option<&'a str>,
        sql: &'a str,
        params: &'a [SqlValue],
        rows: &'a mut dyn RowSink,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(counted(self.0.query_read_only(role, sql, params, rows)))
    }

    fn transaction(&self, steps: Vec<Step>) -> BoxFuture<'_, Result<()>> {
        Box::pin(counted(self.0.transaction(steps)))
    }

    fn lock_migrations(&self) -> Option<BoxFuture<'_, Result<Box<dyn MigrationsLock>>>> {
        self.0.lock_migrations()
    }

    #[cfg(feature = "concurrent-indexes")]
    fn migrate_alone<'a>(
        &'a self,
        schema: &'a str,
        sql: &'a str,
        record: Step,
    ) -> std::result::Result<BoxFuture<'a, Result<()>>, Step> {
        let running = self.0.migrate_alone(schema, sql, record)?;
        Ok(Box::pin(counted(running)))
    }

    fn catalog(&self) -> Option<&dyn crate::Catalog> {
        self.0.catalog()
    }

    fn wakeups(&self) -> Option<BoxFuture<'_, Result<Box<dyn Wakeups>>>> {
        self.0.wakeups()
    }

    #[cfg(feature = "shield")]
    fn shield_wakeups(&self) -> Option<BoxFuture<'_, Result<Box<dyn Wakeups>>>> {
        self.0.shield_wakeups()
    }

    #[cfg(feature = "cache")]
    fn cache_notifications(&self) -> Option<BoxFuture<'_, Result<Box<dyn crate::Notifications>>>> {
        self.0.cache_notifications()
    }

    #[cfg(feature = "live")]
    fn change_notifications(&self) -> Option<BoxFuture<'_, Result<Box<dyn crate::Notifications>>>> {
        self.0.change_notifications()
    }
}

#[cfg(test)]
mod tests {
    use std::task::{Context, Poll, Waker};

    use super::*;

    struct Idle;

    impl Driver for Idle {
        fn dialect(&self) -> &dyn Dialect {
            unreachable!("the test asks no dialect")
        }

        fn query<'a>(
            &'a self,
            _: &'a str,
            _: &'a [SqlValue],
            _: &'a mut dyn RowSink,
        ) -> BoxFuture<'a, Result<()>> {
            Box::pin(async {
                assert_eq!(in_use(), 1.0);
                Ok(())
            })
        }

        fn transaction(&self, _: Vec<Step>) -> BoxFuture<'_, Result<()>> {
            Box::pin(async { Ok(()) })
        }
    }

    #[test]
    fn a_call_counts_as_busy_while_it_runs_and_free_after() {
        let driver = Counted::new(Idle);
        let mut rows = crate::Decoding::new(Vec::new(), crate::row_values);
        let mut query = driver.query("SELECT 1", &[], &mut rows);
        let mut context = Context::from_waker(Waker::noop());

        let answered = query.as_mut().poll(&mut context);

        assert!(matches!(answered, Poll::Ready(Ok(()))));
        assert_eq!(in_use(), 0.0);
    }

    #[test]
    fn an_outbox_count_starts_only_when_none_is_running() {
        let first = Counting::start();
        let overlapping = Counting::start();
        drop(first);

        assert!(overlapping.is_none());
        assert!(Counting::start().is_some());
    }
}
