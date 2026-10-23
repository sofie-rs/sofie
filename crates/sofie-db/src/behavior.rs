use sofie_core::{Cast, Instant, Result, SqlValue, Uuid};

use crate::aggregate::{Snapshot, Table, table};
use crate::{Aggregate, Column, Field, Plan, Stamp};

/// Never runs I/O.
pub trait Behavior: Sync {
    /// Stored after the id and before the aggregate's own fields.
    fn columns(&self) -> &'static [Field];

    fn written(&self, write: &Write<'_>, values: &mut [SqlValue]);

    fn guards(&self) -> bool {
        false
    }

    fn hides(&self) -> Option<&'static str> {
        None
    }

    fn broadcasts(&self) -> bool {
        false
    }

    fn searched(&self) -> bool {
        false
    }

    #[cfg(feature = "search-engine")]
    fn search_fields(&self) -> &'static [crate::SearchedField] {
        &[]
    }

    /// Runs once every behavior has set its columns.
    fn planned(&self, write: &Write<'_>, plan: &mut Plan<'_>) -> Result<()> {
        let _ = (write, plan);
        Ok(())
    }

    /// Names the behavior in errors, which see it only as `dyn Behavior`.
    fn name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }
}

pub struct Write<'a> {
    pub id: Uuid,
    pub owner: Option<Uuid>,
    pub stamp: Stamp,
    pub(crate) table: &'a Table,
    pub(crate) loaded: Option<&'a Snapshot>,
    pub(crate) fields: &'a [SqlValue],
    pub(crate) stored: &'a [SqlValue],
}

impl Write<'_> {
    #[must_use]
    pub fn loaded(&self, column: &str) -> Option<&SqlValue> {
        let loaded = self.loaded?;
        self.lookup(column, &loaded.stored, &loaded.fields)
    }

    #[must_use]
    pub fn value(&self, column: &str) -> Option<&SqlValue> {
        self.lookup(column, self.stored, self.fields)
    }

    #[cfg(any(feature = "codes", feature = "history"))]
    pub(crate) fn deleted(&self) -> (bool, bool) {
        let deleted =
            |value: Option<&SqlValue>| value.is_some_and(|value| *value != SqlValue::Null);
        (
            deleted(self.loaded("deleted_at")),
            deleted(self.value("deleted_at")),
        )
    }

    fn lookup<'v>(
        &self,
        column: &str,
        stored: &'v [SqlValue],
        fields: &'v [SqlValue],
    ) -> Option<&'v SqlValue> {
        let position = |columns: &[String]| columns.iter().position(|name| name == column);
        match position(&self.table.stored) {
            Some(index) => stored.get(index),
            None => fields.get(position(&self.table.columns)?),
        }
    }
}

pub trait Uses<B: Behavior>: Aggregate {}

/// Every aggregate has it, first.
pub struct Versioned;

impl Behavior for Versioned {
    fn columns(&self) -> &'static [Field] {
        const { &[Field::new("version", i64::COLUMNS, false)] }
    }

    fn written(&self, write: &Write<'_>, values: &mut [SqlValue]) {
        values[0] = SqlValue::Int(version(write.loaded("version")) + 1);
    }

    fn guards(&self) -> bool {
        true
    }
}

pub(crate) fn version(stored: Option<&SqlValue>) -> i64 {
    match stored {
        Some(SqlValue::Int(version)) => *version,
        _ => 0,
    }
}

pub struct Timestamps;

impl Behavior for Timestamps {
    fn columns(&self) -> &'static [Field] {
        const {
            &[
                Field::new("created_at", Instant::COLUMNS, false),
                Field::new("updated_at", Instant::COLUMNS, false),
            ]
        }
    }

    fn written(&self, write: &Write<'_>, values: &mut [SqlValue]) {
        let now = SqlValue::Timestamp(write.stamp.now.timestamp());
        values[0] = write.loaded("created_at").unwrap_or(&now).clone();
        values[1] = now;
    }
}

pub trait Timestamped: Aggregate {
    const CREATED_AT: Column<Self, Instant> = Column::new("created_at");
    const UPDATED_AT: Column<Self, Instant> = Column::new("updated_at");

    fn created_at(&self) -> Option<Instant> {
        stored_time::<Self>(self, "created_at")
    }

    fn updated_at(&self) -> Option<Instant> {
        stored_time::<Self>(self, "updated_at")
    }
}

impl<A: Uses<Timestamps>> Timestamped for A {}

pub(crate) fn stored<'a, A: Aggregate>(aggregate: &'a A, column: &str) -> Option<&'a SqlValue> {
    aggregate
        .meta()
        .stored
        .get(table::<A>().stored_index(column))
}

pub(crate) fn stored_time<A: Aggregate>(aggregate: &A, column: &str) -> Option<Instant> {
    match stored(aggregate, column) {
        Some(SqlValue::Timestamp(time)) => Some(Instant::from(*time)),
        _ => None,
    }
}
