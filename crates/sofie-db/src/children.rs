pub(crate) mod through;

use std::ops::{Deref, DerefMut};

use serde_json::Value;
use sofie_core::encoding::hex;
use sofie_core::{ColumnReader, ColumnSpec, Error, Result, RowReader, SqlType, SqlValue, Uuid};

use crate::aggregate::{Table, field_columns};
use crate::selection::Selection;
use crate::sql::{equals, same_values};
use crate::{Aggregate, Dialect, Field, Plan, Selected, Step};
pub use through::{owned_through, refuse_parent_beyond_reach};

#[derive(Debug)]
pub struct ChildTable {
    pub module: &'static str,
    pub name: &'static str,
    pub fields: &'static [Field],
}

pub trait Child: Sized + Send + Sync + 'static {
    const TABLE: ChildTable;

    fn id(&self) -> Uuid;

    fn encode(&self, out: &mut Vec<SqlValue>);

    fn decode(id: Uuid, row: &mut dyn ColumnReader) -> Result<Self>;

    fn refuse_unlisted_moves(&self, loaded: &mut dyn ColumnReader) -> Result<()> {
        let _ = loaded;
        Ok(())
    }
}

pub struct Children<T> {
    rows: Vec<T>,
    loaded: Vec<(Uuid, Vec<SqlValue>)>,
}

impl<T> Default for Children<T> {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            loaded: Vec::new(),
        }
    }
}

impl<T: Child> Children<T> {
    pub fn push(&mut self, child: T) {
        self.rows.push(child);
    }

    pub fn retain(&mut self, keep: impl FnMut(&T) -> bool) {
        self.rows.retain(keep);
    }

    pub fn decode(row: &mut dyn ColumnReader) -> Result<Self> {
        let Value::Array(rows) = row.json()? else {
            return Err(Error::internal_message(format!(
                "expected the rows of `{}` as a JSON array",
                T::TABLE.name
            )));
        };
        let loaded = rows
            .into_iter()
            .map(child_columns::<T>)
            .collect::<Result<Vec<_>>>()?;
        let rows = loaded
            .iter()
            .map(|(id, values)| T::decode(*id, &mut RowReader::new(values)))
            .collect::<Result<_>>()?;
        Ok(Self { rows, loaded })
    }

    fn current(&self) -> Vec<(Uuid, Vec<SqlValue>)> {
        self.rows
            .iter()
            .map(|child| {
                let mut values = Vec::new();
                child.encode(&mut values);
                (child.id(), values)
            })
            .collect()
    }
}

impl<T> Deref for Children<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        &self.rows
    }
}

impl<T> DerefMut for Children<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        &mut self.rows
    }
}

pub trait ChildRows: Send + Sync {
    fn changed(&self) -> bool;

    fn steps(&self, parent: &str, parent_id: Uuid, dialect: &dyn Dialect) -> Vec<Step>;

    fn committed(&mut self);

    fn refuse_unlisted_moves(&self) -> Result<()>;
}

impl<T: Child> ChildRows for Children<T> {
    fn changed(&self) -> bool {
        let current = self.current();
        current.len() != self.loaded.len()
            || current
                .iter()
                .zip(&self.loaded)
                .any(|((id, values), (loaded, old))| id != loaded || !same_values(old, values))
    }

    fn steps(&self, parent: &str, parent_id: Uuid, dialect: &dyn Dialect) -> Vec<Step> {
        let current = self.current();
        let rows = ChildStatements {
            table: &T::TABLE,
            parent,
            parent_id,
            dialect,
        };
        let loaded = |id: &Uuid| self.loaded.iter().find(|(loaded, _)| loaded == id);
        let deletes = self
            .loaded
            .iter()
            .filter(|(id, _)| !current.iter().any(|(kept, _)| kept == id))
            .map(|(id, _)| rows.delete(*id));
        let updates = current.iter().filter_map(|(id, values)| {
            let (_, old) = loaded(id)?;
            (!same_values(old, values)).then(|| rows.update(*id, values))
        });
        let inserts = current
            .iter()
            .filter(|(id, _)| loaded(id).is_none())
            .map(|(id, values)| rows.insert(*id, values));
        deletes.chain(updates).chain(inserts).collect()
    }

    fn committed(&mut self) {
        self.loaded = self.current();
    }

    fn refuse_unlisted_moves(&self) -> Result<()> {
        for child in &self.rows {
            let id = child.id();
            if let Some((_, loaded)) = self.loaded.iter().find(|(loaded, _)| *loaded == id) {
                child.refuse_unlisted_moves(&mut RowReader::new(loaded))?;
            }
        }
        Ok(())
    }
}

/// After their parent, whose owner it checks.
pub(crate) fn plan_children<A: Aggregate>(
    aggregate: &A,
    plan: &mut Plan<'_>,
    owned_through_parent: Option<Uuid>,
) {
    let id = aggregate.meta().id();
    for children in aggregate.children() {
        for step in children.steps(A::NAME, id, plan.dialect()) {
            plan.push(step);
        }
    }
    if let Some(owner) = owned_through_parent {
        plan.push(through::ownership_check::<A>(id, owner, plan.dialect()));
    }
}

/// Run before the write, so the row still holds the parent it was loaded with.
pub(crate) fn check_loaded_parent<A: Aggregate>(
    aggregate: &A,
    plan: &mut Plan<'_>,
    owned_through_parent: Option<Uuid>,
) {
    let meta = aggregate.meta();
    if let (Some(owner), Some(_)) = (owned_through_parent, meta.snapshot.as_ref()) {
        plan.push(through::ownership_check::<A>(
            meta.id(),
            owner,
            plan.dialect(),
        ));
    }
}

pub(crate) fn committed<A: Aggregate>(aggregate: &mut A) {
    for children in aggregate.children_mut() {
        children.committed();
    }
}

pub(crate) fn refuse_unlisted_moves<A: Aggregate>(aggregate: &A) -> Result<()> {
    aggregate
        .children()
        .iter()
        .try_for_each(|children| children.refuse_unlisted_moves())
}

pub(crate) fn children_changed<A: Aggregate>(aggregate: &A) -> bool {
    aggregate
        .children()
        .iter()
        .any(|children| children.changed())
}

struct ChildStatements<'a> {
    table: &'a ChildTable,
    parent: &'a str,
    parent_id: Uuid,
    dialect: &'a dyn Dialect,
}

impl ChildStatements<'_> {
    fn qualified(&self) -> String {
        self.dialect.qualified(self.table.module, self.table.name)
    }

    fn insert(&self, id: Uuid, values: &[SqlValue]) -> Step {
        let mut columns = vec!["id".to_owned(), self.parent.to_owned()];
        columns.extend(field_columns(self.table.fields));
        let mut params = vec![SqlValue::Uuid(id), SqlValue::Uuid(self.parent_id)];
        params.extend_from_slice(values);
        let sql = crate::sql::insert(self.dialect, &self.qualified(), &columns);
        Step::statement(sql, params, None)
    }

    /// Every column, so each table's update is one prepared statement.
    fn update(&self, id: Uuid, values: &[SqlValue]) -> Step {
        let mut params = Vec::new();
        let columns = field_columns(self.table.fields);
        let assignments: Vec<String> = columns
            .iter()
            .zip(values)
            .map(|(column, value)| equals(self.dialect, column, value.clone(), &mut params))
            .collect();
        let condition = self.this_row(id, &mut params);
        let sql = format!(
            "UPDATE {} SET {} WHERE {condition}",
            self.qualified(),
            assignments.join(", ")
        );
        Step::statement(sql, params, None)
    }

    fn delete(&self, id: Uuid) -> Step {
        let mut params = Vec::new();
        let condition = self.this_row(id, &mut params);
        let sql = format!("DELETE FROM {} WHERE {condition}", self.qualified());
        Step::statement(sql, params, None)
    }

    /// Within its parent, so a forged id cannot reach another parent's child.
    fn this_row(&self, id: Uuid, params: &mut Vec<SqlValue>) -> String {
        let id = equals(self.dialect, "id", SqlValue::Uuid(id), params);
        let parent = equals(
            self.dialect,
            self.parent,
            SqlValue::Uuid(self.parent_id),
            params,
        );
        format!("{id} AND {parent}")
    }
}

impl ChildTable {
    fn specs(&self) -> impl Iterator<Item = &'static ColumnSpec> {
        self.fields.iter().flat_map(|field| field.columns)
    }

    pub(crate) fn selected(&'static self) -> Selected {
        Selected(Selection::Children(self))
    }

    pub(crate) fn render(&self, base: &Table, dialect: &dyn Dialect) -> String {
        let fields = field_columns(self.fields).into_iter().zip(self.specs());
        let columns: Vec<String> = std::iter::once(dialect.quote("id"))
            .chain(fields.map(|(column, spec)| {
                let quoted = dialect.quote(&column);
                match spec.ty {
                    SqlType::Bytes => dialect.bytes_in_json(&quoted),
                    _ => quoted,
                }
            }))
            .collect();
        let from = format!(
            "{} WHERE {} = {}.{}",
            dialect.qualified(self.module, self.name),
            dialect.quote(base.name),
            base.qualified(dialect),
            dialect.quote("id")
        );
        dialect.nested_rows(&columns.join(", "), &from)
    }
}

fn child_columns<T: Child>(row: Value) -> Result<(Uuid, Vec<SqlValue>)> {
    let Value::Array(values) = row else {
        return Err(Error::internal_message(format!(
            "expected a row of `{}` as a JSON array",
            T::TABLE.name
        )));
    };
    let mut values = values.into_iter();
    let id = match values.next() {
        Some(Value::String(id)) => id
            .parse()
            .map_err(|error| Error::internal("reading a child row's id", error))?,
        _ => return Err(Error::internal_message("a child row without its id")),
    };
    let columns = T::TABLE
        .specs()
        .zip(values)
        .map(|(spec, value)| sql_value(spec.ty, value))
        .collect::<Result<_>>()?;
    Ok((id, columns))
}

fn sql_value(ty: SqlType, value: Value) -> Result<SqlValue> {
    let unreadable = |error: &dyn std::fmt::Display| {
        Error::internal_message(format!("reading a {ty:?} child column: {error}"))
    };
    Ok(match (ty, value) {
        (_, Value::Null) => SqlValue::Null,
        (SqlType::Bool, Value::Bool(value)) => SqlValue::Bool(value),
        // A database without booleans or a timestamp type writes them as numbers.
        (SqlType::Bool, Value::Number(number)) => SqlValue::Bool(number.as_i64() != Some(0)),
        (SqlType::Timestamp, Value::Number(number)) => SqlValue::Timestamp(
            number
                .as_i64()
                .and_then(|micros| sofie_core::Timestamp::from_microsecond(micros).ok())
                .ok_or_else(|| unreadable(&number))?,
        ),
        (SqlType::Int, Value::Number(number)) => {
            SqlValue::Int(number.as_i64().ok_or_else(|| unreadable(&number))?)
        }
        (SqlType::Float, Value::Number(number)) => {
            SqlValue::Float(number.as_f64().ok_or_else(|| unreadable(&number))?)
        }
        // Postgres's JSON has no number for these, so it writes them as text.
        (SqlType::Float, Value::String(text)) => SqlValue::Float(match text.as_str() {
            "NaN" => f64::NAN,
            "Infinity" => f64::INFINITY,
            "-Infinity" => f64::NEG_INFINITY,
            _ => return Err(unreadable(&text)),
        }),
        (SqlType::Text, Value::String(text)) => SqlValue::Text(text),
        (SqlType::Uuid, Value::String(text)) => {
            SqlValue::Uuid(text.parse().map_err(|error| unreadable(&error))?)
        }
        (SqlType::Timestamp, Value::String(text)) => {
            SqlValue::Timestamp(text.parse().map_err(|error| unreadable(&error))?)
        }
        (SqlType::Date, Value::String(text)) => {
            SqlValue::Date(text.parse().map_err(|error| unreadable(&error))?)
        }
        (SqlType::DateTime, Value::String(text)) => {
            SqlValue::DateTime(text.parse().map_err(|error| unreadable(&error))?)
        }
        (SqlType::Bytes, Value::String(text)) => {
            SqlValue::Bytes(hex_bytes(&text).ok_or_else(|| unreadable(&"bad hex"))?)
        }
        (_, other) => return Err(unreadable(&other)),
    })
}

/// `\x0a1b`, the text form of a byte column.
fn hex_bytes(text: &str) -> Option<Vec<u8>> {
    hex::decode(text.strip_prefix("\\x")?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_columns_read_back_as_their_types() {
        let read = |ty, value: Value| sql_value(ty, value).unwrap();

        assert_eq!(read(SqlType::Int, 7.into()), SqlValue::Int(7));
        assert_eq!(
            read(SqlType::Text, "a".into()),
            SqlValue::Text("a".to_owned())
        );
        assert_eq!(
            read(SqlType::Bytes, "\\x0aff".into()),
            SqlValue::Bytes(vec![10, 255])
        );
        assert_eq!(read(SqlType::Int, Value::Null), SqlValue::Null);
        assert!(matches!(
            read(SqlType::Timestamp, "2026-10-11T09:30:00.5+00:00".into()),
            SqlValue::Timestamp(_)
        ));
        assert!(sql_value(SqlType::Int, "7".into()).is_err());
    }

    #[test]
    fn a_float_postgres_writes_as_text_reads_back() {
        let read = |text: &str| sql_value(SqlType::Float, text.into());

        assert!(matches!(read("NaN"), Ok(SqlValue::Float(value)) if value.is_nan()));
        assert_eq!(read("Infinity").unwrap(), SqlValue::Float(f64::INFINITY));
        assert_eq!(
            read("-Infinity").unwrap(),
            SqlValue::Float(f64::NEG_INFINITY)
        );
        assert!(read("1.5").is_err());
    }

    struct Weight {
        id: Uuid,
        kilograms: f64,
    }

    impl Child for Weight {
        const TABLE: ChildTable = ChildTable {
            module: "stock",
            name: "weight",
            fields: &[Field::new(
                "kilograms",
                <f64 as sofie_core::Cast>::COLUMNS,
                false,
            )],
        };

        fn id(&self) -> Uuid {
            self.id
        }

        fn encode(&self, out: &mut Vec<SqlValue>) {
            out.push(SqlValue::Float(self.kilograms));
        }

        fn decode(id: Uuid, row: &mut dyn ColumnReader) -> Result<Self> {
            Ok(Self {
                id,
                kilograms: row.float()?,
            })
        }
    }

    #[test]
    fn a_child_float_changes_by_its_bits() {
        let id = Uuid::now_v7().unwrap();
        let dialect = crate::aggregate::tests::NUMBERED;
        let mut children = Children::default();
        children.push(Weight {
            id,
            kilograms: f64::NAN,
        });
        children.committed();

        assert!(!children.changed());
        assert!(children.steps("parcel", id, &dialect).is_empty());

        children[0].kilograms = -0.0;
        children.committed();
        children[0].kilograms = 0.0;

        assert!(children.changed());
        assert_eq!(children.steps("parcel", id, &dialect).len(), 1);
    }

    struct Label {
        id: Uuid,
        text: String,
        color: String,
    }

    impl Child for Label {
        const TABLE: ChildTable = ChildTable {
            module: "stock",
            name: "label",
            fields: &[
                Field::new("text", <String as sofie_core::Cast>::COLUMNS, false),
                Field::new("color", <String as sofie_core::Cast>::COLUMNS, false),
            ],
        };

        fn id(&self) -> Uuid {
            self.id
        }

        fn encode(&self, out: &mut Vec<SqlValue>) {
            out.push(SqlValue::Text(self.text.clone()));
            out.push(SqlValue::Text(self.color.clone()));
        }

        fn decode(id: Uuid, row: &mut dyn ColumnReader) -> Result<Self> {
            Ok(Self {
                id,
                text: row.text()?.to_owned(),
                color: row.text()?.to_owned(),
            })
        }
    }

    #[test]
    fn a_child_row_is_updated_by_one_statement_whatever_changed() {
        let dialect = crate::aggregate::tests::NUMBERED;
        let mut labels = Children::default();
        for _ in 0..2 {
            labels.push(Label {
                id: Uuid::now_v7().unwrap(),
                text: "fragile".to_owned(),
                color: "red".to_owned(),
            });
        }
        labels.committed();

        labels[0].text = "heavy".to_owned();
        labels[1].color = "blue".to_owned();
        let statements: Vec<String> = labels
            .steps("parcel", Uuid::now_v7().unwrap(), &dialect)
            .into_iter()
            .map(|step| match step {
                Step::Statement { sql, .. } | Step::Script(sql) => sql,
            })
            .collect();

        assert_eq!(statements.len(), 2);
        assert_eq!(statements[0], statements[1]);
    }
}
