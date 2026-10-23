use sofie_core::{ColumnSpec, SqlValue};

use crate::Dialect;

pub(crate) fn column_name(field: &str, column: &ColumnSpec) -> String {
    format!("{field}{}", column.suffix)
}

pub(crate) fn insert(dialect: &dyn Dialect, table: &str, columns: &[impl AsRef<str>]) -> String {
    let columns: Vec<String> = columns
        .iter()
        .map(|column| dialect.quote(column.as_ref()))
        .collect();
    let placeholders: Vec<String> = (1..=columns.len())
        .map(|index| dialect.placeholder(index))
        .collect();
    format!(
        "INSERT INTO {table} ({}) VALUES ({})",
        columns.join(", "),
        placeholders.join(", ")
    )
}

#[cfg(feature = "children")]
pub(crate) fn equals(
    dialect: &dyn Dialect,
    column: &str,
    value: SqlValue,
    params: &mut Vec<SqlValue>,
) -> String {
    params.push(value);
    format!(
        "{} = {}",
        dialect.quote(column),
        dialect.placeholder(params.len())
    )
}

/// By bits, so a loaded NaN reads as unchanged and `-0.0` is written over `0.0`.
pub(crate) fn same_value(old: &SqlValue, new: &SqlValue) -> bool {
    match (old, new) {
        (SqlValue::Float(old), SqlValue::Float(new)) => old.to_bits() == new.to_bits(),
        _ => old == new,
    }
}

pub(crate) fn same_values(old: &[SqlValue], new: &[SqlValue]) -> bool {
    old.len() == new.len() && old.iter().zip(new).all(|(old, new)| same_value(old, new))
}
