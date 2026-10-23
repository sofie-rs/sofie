//! Statements run on the calling task over one connection, which SQLite's single
//! writer would serialize anyway, so it suits an app running as one instance.

mod catalog;
mod dialect;
mod messages;
mod rows;
mod transaction;
mod value;

use std::path::Path;
use std::sync::Mutex;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use rusqlite::{Connection, ErrorCode};
use sofie_core::{Error, Result, SqlValue, unpoisoned};
use sofie_db::{BoxFuture, Dialect, Driver, RowSink, Step, UniqueViolation, traced_statement};

const BUSY_WAIT: Duration = Duration::from_secs(5);
/// Enough for an app's statement texts, so each is parsed and planned once per process.
const STATEMENTS_KEPT: usize = 512;
const MEMORY: &str = ":memory:";

pub struct Sqlite {
    connection: Mutex<Connection>,
}

impl Sqlite {
    /// `sqlite://<path>`, or `sqlite::memory:` for a database gone when the app stops.
    pub fn connect(url: &str) -> Result<Self> {
        let place = url.strip_prefix("sqlite:").ok_or_else(|| {
            Error::internal_message("a SQLite database url starts with `sqlite:`")
        })?;
        if place == MEMORY {
            return Self::memory();
        }
        let path = place.strip_prefix("//").ok_or_else(|| {
            Error::internal_message(
                "a SQLite database url is `sqlite://<path>` or `sqlite::memory:`",
            )
        })?;
        Self::open(Path::new(path))
    }

    /// Readers do not wait for the writer, as the file keeps a write-ahead log.
    pub fn open(path: &Path) -> Result<Self> {
        let connection = Connection::open(path)
            .map_err(|error| Error::internal(format!("opening {}", path.display()), error))?;
        Self::configured(
            connection,
            "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;",
        )
    }

    pub fn memory() -> Result<Self> {
        let connection = Connection::open_in_memory()
            .map_err(|error| Error::internal("opening an in-memory SQLite database", error))?;
        Self::configured(connection, "")
    }

    /// `LIKE` tells capitals apart, as on Postgres.
    fn configured(connection: Connection, pragmas: &str) -> Result<Self> {
        connection
            .busy_timeout(BUSY_WAIT)
            .and_then(|()| {
                connection.execute_batch(&format!(
                    "{pragmas}PRAGMA foreign_keys = ON; PRAGMA case_sensitive_like = ON;"
                ))
            })
            .and_then(|()| dialect::add_lower(&connection))
            .map_err(|error| Error::internal("configuring the SQLite connection", error))?;
        connection.set_prepared_statement_cache_capacity(STATEMENTS_KEPT);
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    fn with_connection<T>(&self, work: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        let mut connection = unpoisoned::lock(&self.connection);
        work(&mut connection)
    }
}

impl Driver for Sqlite {
    fn dialect(&self) -> &dyn Dialect {
        self
    }

    fn query<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [SqlValue],
        rows: &'a mut dyn RowSink,
    ) -> BoxFuture<'a, Result<()>> {
        let read = self.with_connection(|connection| {
            traced(sql, || rows::read(connection, sql, params, rows))
        });
        Box::pin(std::future::ready(read.map(drop)))
    }

    #[cfg(feature = "raw-sql")]
    fn query_read_only<'a>(
        &'a self,
        _role: Option<&'a str>,
        sql: &'a str,
        params: &'a [SqlValue],
        rows: &'a mut dyn RowSink,
    ) -> BoxFuture<'a, Result<()>> {
        let read = self.with_connection(|connection| {
            traced(sql, || rows::read_only(connection, sql, params, rows))
        });
        Box::pin(std::future::ready(read.map(drop)))
    }

    fn transaction(&self, steps: Vec<Step>) -> BoxFuture<'_, Result<()>> {
        let committed = self.with_connection(|connection| transaction::run(connection, &steps));
        Box::pin(std::future::ready(committed))
    }

    fn catalog(&self) -> Option<&dyn sofie_db::Catalog> {
        Some(self)
    }
}

/// Runs in `traced_statement`'s span; nothing in it waits, so its one poll ends it.
pub(crate) fn traced(sql: &str, run: impl FnOnce() -> Result<u64>) -> Result<u64> {
    let traced = std::pin::pin!(traced_statement(sql, async { run() }, |count| Some(*count)));
    match traced.poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(outcome) => outcome,
        Poll::Pending => Err(Error::internal_message(format!(
            "`{sql}` waited, which a SQLite statement never does"
        ))),
    }
}

/// Named after its index, as the framework matches field errors by it.
fn statement_error(connection: &Connection, sql: &str, error: rusqlite::Error) -> Error {
    if let Some(constraint) = check_constraint(&error) {
        return Error::Rule(sofie_db::check_refusal(constraint));
    }
    if extended_code(&error) == Some(rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY) {
        return Error::Rule(sofie_db::REFERENCE_REFUSAL.to_owned());
    }
    let context = format!("running `{sql}`");
    let Some(columns) = unique_columns(&error) else {
        return Error::internal(context, error);
    };
    match catalog::unique_index(connection, &columns) {
        Some(index) => Error::internal(context, UniqueViolation { index }),
        None => Error::internal(context, error),
    }
}

/// Named as declared, or the expression an unnamed check tests.
fn check_constraint(error: &rusqlite::Error) -> Option<&str> {
    let rusqlite::Error::SqliteFailure(failure, Some(message)) = error else {
        return None;
    };
    if failure.extended_code != rusqlite::ffi::SQLITE_CONSTRAINT_CHECK {
        return None;
    }
    message.strip_prefix("CHECK constraint failed: ")
}

fn extended_code(error: &rusqlite::Error) -> Option<i32> {
    match error {
        rusqlite::Error::SqliteFailure(failure, _) => Some(failure.extended_code),
        _ => None,
    }
}

fn unique_columns(error: &rusqlite::Error) -> Option<Vec<(String, String)>> {
    let rusqlite::Error::SqliteFailure(failure, Some(message)) = error else {
        return None;
    };
    if failure.code != ErrorCode::ConstraintViolation {
        return None;
    }
    let columns = message.strip_prefix("UNIQUE constraint failed: ")?;
    columns
        .split(", ")
        .map(|qualified| {
            let (table, column) = qualified.split_once('.')?;
            Some((table.to_owned(), column.to_owned()))
        })
        .collect()
}
