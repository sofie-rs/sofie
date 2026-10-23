use std::fmt::Write as _;

use sofie_core::{Cast, Result, SqlValue};

use crate::behavior::stored;
use crate::requested::stored_mut;
use crate::{Aggregate, Behavior, Column, Engine, Field, Plan, Query, Source, Step, Uses, Write};

pub struct Active;

impl Behavior for Active {
    fn columns(&self) -> &'static [Field] {
        const { &[Field::new("is_active", bool::COLUMNS, false)] }
    }

    fn written(&self, _write: &Write<'_>, values: &mut [SqlValue]) {
        if values[0] == SqlValue::Null {
            values[0] = SqlValue::Bool(true);
        }
    }
}

pub trait Activates: Aggregate {
    const IS_ACTIVE: Column<Self, bool> = Column::new("is_active");

    fn activate(&mut self) {
        *stored_mut::<Self>(self, "is_active") = SqlValue::Bool(true);
    }

    fn deactivate(&mut self) {
        *stored_mut::<Self>(self, "is_active") = SqlValue::Bool(false);
    }

    fn is_active(&self) -> bool {
        stored::<Self>(self, "is_active") != Some(&SqlValue::Bool(false))
    }
}

impl<A: Uses<Active>> Activates for A {}

#[must_use]
pub fn active<T: Source>(query: Query<T>) -> Query<T>
where
    T::Base: Activates,
{
    query.where_(<T::Base as Activates>::IS_ACTIVE.eq(true))
}

/// Making one the default clears it on the owner's others in the same commit.
pub struct Default;

impl Behavior for Default {
    fn columns(&self) -> &'static [Field] {
        const { &[Field::new("is_default", bool::COLUMNS, false)] }
    }

    fn written(&self, _write: &Write<'_>, values: &mut [SqlValue]) {
        if values[0] == SqlValue::Null {
            values[0] = SqlValue::Bool(false);
        }
    }

    fn planned(&self, write: &Write<'_>, plan: &mut Plan<'_>) -> Result<()> {
        let flag = Some(&SqlValue::Bool(true));
        if write.value("is_default") != flag || write.loaded("is_default") == flag {
            return Ok(());
        }
        let dialect = plan.dialect();
        let owner = write
            .table
            .owner
            .map(|owner| write.value(owner).cloned().unwrap_or(SqlValue::Null));
        if dialect.engine() == Engine::Postgres {
            plan.push(one_default_at_a_time(write, owner.as_ref()));
        }
        let mut params = vec![SqlValue::Uuid(write.id)];
        let mut sql = format!(
            "UPDATE {table} SET {flag} = FALSE, {version} = {version} + 1 \
             WHERE {flag} AND {id} <> {first}",
            table = write.table.qualified(dialect),
            flag = dialect.quote("is_default"),
            version = dialect.quote("version"),
            id = dialect.quote("id"),
            first = dialect.placeholder(1),
        );
        if let (Some(column), Some(owner)) = (write.table.owner, owner) {
            params.push(owner);
            let _ = write!(
                sql,
                " AND {} = {}",
                dialect.quote(column),
                dialect.placeholder(2)
            );
        }
        plan.push(Step::statement(sql, params, None));
        #[cfg(feature = "cache")]
        plan.changed(crate::ChangedRecord {
            module: write.table.schema,
            aggregate: write.table.name,
            id: None,
        });
        Ok(())
    }
}

/// Under READ COMMITTED two commits clearing each other's defaults miss the one the other sets,
/// so both would stay; SQLite's single writer already serializes them.
fn one_default_at_a_time(write: &Write<'_>, owner: Option<&SqlValue>) -> Step {
    let key = format!(
        "sofie.default {}.{} {owner:?}",
        write.table.schema, write.table.name
    );
    Step::statement(
        "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))".to_owned(),
        vec![SqlValue::Text(key)],
        None,
    )
}

pub trait Defaults: Aggregate {
    const IS_DEFAULT: Column<Self, bool> = Column::new("is_default");

    fn make_default(&mut self) {
        *stored_mut::<Self>(self, "is_default") = SqlValue::Bool(true);
    }

    fn is_default(&self) -> bool {
        stored::<Self>(self, "is_default") == Some(&SqlValue::Bool(true))
    }
}

impl<A: Uses<Default>> Defaults for A {}
