use serde_json::{Map, Value};
use sofie_core::{Error, Result, SqlValue, Uuid};

use crate::aggregate::Table;
use crate::{Aggregate, Plan, Write};

/// A soft-deleting record's change event carries when it was deleted, so its mirrors hide it too.
pub(crate) const DELETED_AT: &str = "deleted_at";

pub(crate) fn plan_change_event<A: Aggregate>(
    aggregate: &A,
    write: &Write<'_>,
    plan: &mut Plan<'_>,
) -> Result<()> {
    let Some(fields) = change_fields(aggregate, write.table)? else {
        return Ok(());
    };
    let version = crate::behavior::version(write.value("version"));
    let mut payload = change_payload(fields, write.id, version);
    if let Some(deleted_at) = write.value(DELETED_AT) {
        let deleted_at = match deleted_at {
            SqlValue::Timestamp(at) => Value::String(at.to_string()),
            SqlValue::Null => Value::Null,
            _ => Value::String(write.stamp.now.to_string()),
        };
        payload.insert(DELETED_AT.to_owned(), deleted_at);
    }
    queue_change::<A>(payload, plan)
}

#[cfg(all(feature = "retention", feature = "contracts"))]
pub(crate) fn changes<A: Aggregate>(record: &A) -> Result<bool> {
    Ok(change_fields(record, crate::aggregate::table::<A>())?.is_some())
}

#[cfg(all(feature = "retention", feature = "contracts"))]
pub(crate) fn pruned_change(meta: &crate::Meta) -> Value {
    let mut payload = change_payload(Map::new(), meta.id(), meta.version() + 1);
    payload.insert(crate::event::PRUNED.to_owned(), Value::Bool(true));
    Value::Object(payload)
}

#[cfg_attr(not(feature = "contracts"), expect(unused_variables))]
fn change_fields<A: Aggregate>(aggregate: &A, table: &Table) -> Result<Option<Map<String, Value>>> {
    #[cfg(feature = "contracts")]
    let exposed = aggregate.exposed_fields();
    #[cfg(not(feature = "contracts"))]
    let exposed: Option<Result<Map<String, Value>>> = None;
    match exposed {
        Some(fields) => fields.map(Some),
        None if broadcasts(table) => Ok(Some(Map::new())),
        None => Ok(None),
    }
}

fn queue_change<A: Aggregate>(payload: Map<String, Value>, plan: &mut Plan<'_>) -> Result<()> {
    let name = A::CHANGED.ok_or_else(|| {
        Error::internal_message(format!("`{}` names no change event; derive it", A::NAME))
    })?;
    plan.raise_one(name, Value::Object(payload));
    Ok(())
}

pub(crate) fn change_payload(
    mut fields: Map<String, Value>,
    id: Uuid,
    version: i64,
) -> Map<String, Value> {
    fields.insert("id".to_owned(), Value::String(id.to_string()));
    fields.insert("version".to_owned(), Value::from(version));
    fields
}

fn broadcasts(table: &Table) -> bool {
    table
        .behaviors
        .iter()
        .any(|(behavior, _)| behavior.broadcasts())
}
