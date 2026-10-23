use std::fmt;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use sofie_core::{ColumnReader, Error, Result, SqlType, SqlValue};

use crate::{GuardCheck, MessageSql, Migrations};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Long enough for a burst to pass, well short of a request's own timeout.
const DEFAULT_POOL_WAIT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PoolSettings {
    /// Twice the logical CPUs when `None`.
    pub connections: Option<NonZeroUsize>,
    pub wait: Duration,
}

impl Default for PoolSettings {
    fn default() -> Self {
        Self {
            connections: None,
            wait: DEFAULT_POOL_WAIT,
        }
    }
}

#[derive(Debug)]
pub enum Step {
    /// A mismatched `expect_rows` rolls back with its error; a check costs a round trip.
    Statement {
        sql: String,
        params: Vec<SqlValue>,
        expect_rows: Option<ExpectedRows>,
    },
    /// Trusted SQL only (migrations, framework setup), never input.
    Script(String),
}

impl Step {
    /// A mismatched `expect_rows` is a `Conflict`.
    #[must_use]
    pub fn statement(
        sql: impl Into<String>,
        params: Vec<SqlValue>,
        expect_rows: Option<u64>,
    ) -> Self {
        Self::Statement {
            sql: sql.into(),
            params,
            expect_rows: expect_rows.map(ExpectedRows::or_conflict),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpectedRows {
    pub rows: u64,
    pub otherwise: Unmet,
}

/// What a statement that touched another number of rows than expected fails with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unmet {
    Conflict,
    NotFound,
    Refused(Arc<GuardCheck>),
}

impl ExpectedRows {
    #[must_use]
    pub const fn or_conflict(rows: u64) -> Self {
        Self {
            rows,
            otherwise: Unmet::Conflict,
        }
    }

    #[must_use]
    pub const fn or_not_found(rows: u64) -> Self {
        Self {
            rows,
            otherwise: Unmet::NotFound,
        }
    }

    pub fn check(&self, affected: u64) -> Result<()> {
        let met = affected == self.rows;
        match &self.otherwise {
            Unmet::Refused(check) => check.checked(!met),
            _ if met => Ok(()),
            Unmet::Conflict => Err(Error::Conflict),
            Unmet::NotFound => Err(Error::NotFound),
        }
    }
}

pub trait RowSink: Send {
    /// Checked once before any row is read; `None` takes any type, fewer columns are refused.
    fn expects(&self) -> &[Option<SqlType>];

    fn row(&mut self, row: &mut dyn ColumnReader) -> Result<()>;
}

pub trait Driver: Send + Sync + 'static {
    fn dialect(&self) -> &dyn Dialect;

    fn query<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [SqlValue],
        rows: &'a mut dyn RowSink,
    ) -> BoxFuture<'a, Result<()>>;

    /// Read-only, as nothing else keeps raw SQL from writing.
    #[cfg(feature = "raw-sql")]
    fn query_read_only<'a>(
        &'a self,
        role: Option<&'a str>,
        sql: &'a str,
        params: &'a [SqlValue],
        rows: &'a mut dyn RowSink,
    ) -> BoxFuture<'a, Result<()>> {
        let _ = (role, params, rows);
        crate::raw::refuse_read_only(sql)
    }

    /// A unique-index refusal fails as `Internal` with a [`UniqueViolation`] source; a check,
    /// exclusion or foreign key as `Rule` ([`check_refusal`], [`exclusion_refusal`], …).
    fn transaction(&self, steps: Vec<Step>) -> BoxFuture<'_, Result<()>>;

    /// Held over a whole `Schema::migrate` until dropped; else each migration's transaction locks.
    fn lock_migrations(&self) -> Option<BoxFuture<'_, Result<Box<dyn MigrationsLock>>>> {
        None
    }

    /// Outside a transaction, under `lock_migrations`; other migrations get `record` back.
    #[cfg(feature = "concurrent-indexes")]
    fn migrate_alone<'a>(
        &'a self,
        schema: &'a str,
        sql: &'a str,
        record: Step,
    ) -> std::result::Result<BoxFuture<'a, Result<()>>, Step> {
        let _ = (schema, sql);
        Err(record)
    }

    fn catalog(&self) -> Option<&dyn crate::Catalog> {
        None
    }

    fn wakeups(&self) -> Option<BoxFuture<'_, Result<Box<dyn Wakeups>>>> {
        None
    }

    #[cfg(feature = "shield")]
    fn shield_wakeups(&self) -> Option<BoxFuture<'_, Result<Box<dyn Wakeups>>>> {
        None
    }

    /// The payloads sent to the `sofie_cache` channel, such as by `pg_notify`.
    #[cfg(feature = "cache")]
    fn cache_notifications(&self) -> Option<BoxFuture<'_, Result<Box<dyn Notifications>>>> {
        None
    }

    /// The payloads `Dialect::live_notify` sent.
    #[cfg(feature = "live")]
    fn change_notifications(&self) -> Option<BoxFuture<'_, Result<Box<dyn Notifications>>>> {
        None
    }
}

#[derive(Debug)]
pub struct UniqueViolation {
    pub index: String,
}

impl fmt::Display for UniqueViolation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "unique index {} refused the write", self.index)
    }
}

impl std::error::Error for UniqueViolation {}

#[must_use]
pub fn check_refusal(constraint: &str) -> String {
    format!("a value breaks the rule `{constraint}`")
}

/// SQLite names no foreign key it refuses, so neither engine's refusal does.
pub const REFERENCE_REFUSAL: &str =
    "the write names a record that does not exist, or removes one another record still names";

#[must_use]
pub fn exclusion_refusal(constraint: &str) -> String {
    format!("a value overlaps one another record holds, under the rule `{constraint}`")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    Postgres,
    Sqlite,
}

/// Rendered SQL is cached by the dialect's type, so it must not vary per instance.
pub trait Dialect: Send + Sync + std::any::Any {
    /// `index` counts from 1.
    fn push_placeholder(&self, sql: &mut String, index: usize);

    fn push_quoted(&self, sql: &mut String, identifier: &str) {
        sql.push('"');
        for character in identifier.chars() {
            if character == '"' {
                sql.push('"');
            }
            sql.push(character);
        }
        sql.push('"');
    }

    fn placeholder(&self, index: usize) -> String {
        let mut sql = String::new();
        self.push_placeholder(&mut sql, index);
        sql
    }

    fn quote(&self, identifier: &str) -> String {
        let mut sql = String::with_capacity(identifier.len() + 2);
        self.push_quoted(&mut sql, identifier);
        sql
    }

    /// Which of a module's migration files apply: SQLite's where it has its own.
    fn engine(&self) -> Engine {
        Engine::Postgres
    }

    /// Locks its one text parameter until the transaction ends; `None` where commits queue anyway.
    fn key_lock(&self) -> Option<&'static str> {
        None
    }

    fn shared_key_lock(&self) -> Option<&'static str> {
        None
    }

    /// A module's table: in its schema, or named after it where the database has no schemas.
    fn qualified(&self, schema: &str, name: &str) -> String {
        format!("{}.{}", self.quote(schema), self.quote(name))
    }

    /// A bytes column as the JSON of loaded child rows holds it: `\x` and hex.
    fn bytes_in_json(&self, column: &str) -> String {
        column.to_owned()
    }

    fn nested_rows(&self, columns: &str, from: &str) -> String {
        panic!("this database cannot load child rows, so `{columns}` of `{from}` cannot be read")
    }

    fn binds_arrays(&self) -> bool {
        false
    }

    fn migrations_setup(&self) -> &'static str;

    fn migrations_lock(&self) -> &'static str;

    fn enter_schema(&self, schema: &str) -> String;

    /// Answers, one per row, the statements `Schema::fresh` drops every table with.
    fn drop_every_table(&self) -> Option<&'static str> {
        None
    }

    /// Without full-text search, the words are found as one piece of text, ignoring case.
    #[cfg(feature = "search")]
    fn text_search(&self, column: &str, words: &str) -> String {
        let (column, words) = (self.lowercase(column), self.lowercase(words));
        let escaped =
            format!(r"replace(replace(replace({words}, '\', '\\'), '%', '\%'), '_', '\_')");
        format!(r"({column} LIKE '%' || {escaped} || '%' ESCAPE '\')")
    }

    fn lowercase(&self, expression: &str) -> String {
        format!("lower({expression})")
    }

    /// After a nullable column's direction: empty values sort as Postgres's do, last ascending.
    fn empty_order(&self, descending: bool) -> &'static str {
        let _ = descending;
        ""
    }

    /// `None` returns to the app's login.
    #[cfg(feature = "module-roles")]
    fn enter_role(&self, module: Option<&str>) -> Option<String> {
        let _ = module;
        None
    }

    fn framework_migrations(&self) -> Migrations;

    fn messages(&self) -> &'static MessageSql;

    #[cfg(feature = "shield")]
    fn shield(&self) -> Option<&'static crate::ShieldSql> {
        None
    }

    #[cfg(feature = "remote-contracts")]
    fn feed(&self) -> Option<&'static crate::FeedSql> {
        None
    }

    #[cfg(feature = "product-reports")]
    fn product_events(&self) -> Option<&'static crate::ProductSql> {
        None
    }

    /// Sends its one text parameter to every instance's `Driver::change_notifications`.
    #[cfg(feature = "live")]
    fn live_notify(&self) -> Option<&'static str> {
        None
    }
}

pub trait MigrationsLock: Send {}

pub trait Wakeups: Send {
    fn next(&mut self) -> BoxFuture<'_, Result<()>>;
}

#[cfg(any(feature = "cache", feature = "live"))]
pub trait Notifications: Send {
    /// Every payload buffered so far, waiting for at least one.
    fn next(&mut self) -> BoxFuture<'_, Result<Vec<String>>>;
}
