use std::error::Error as StdError;
use std::future::poll_fn;
use std::pin::pin;

use futures_core::Stream;
use sofie_core::{ColumnReader, Date, DateTime, Error, Result, SqlType, SqlValue, Timestamp, Uuid};
use sofie_db::RowSink;
use tokio_postgres::types::{FromSql, Type};
use tokio_postgres::{Client, Row, Statement};

use crate::error::statement_error;
use crate::value::{
    Encoded, WireResult, decode_date, decode_date_time, decode_jsonb, decode_timestamp,
};

/// How a column arrives, read from the statement once so a row is only bytes to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Wire {
    Bool,
    Int2,
    Int4,
    Int8,
    Float4,
    Float8,
    Text,
    Bytes,
    Uuid,
    Timestamp,
    Date,
    DateTime,
    Json,
    Jsonb,
    Unsupported,
}

impl Wire {
    pub(crate) fn of(ty: &Type) -> Self {
        match *ty {
            Type::BOOL => Self::Bool,
            Type::INT2 => Self::Int2,
            Type::INT4 => Self::Int4,
            Type::INT8 => Self::Int8,
            Type::FLOAT4 => Self::Float4,
            Type::FLOAT8 => Self::Float8,
            Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME => Self::Text,
            Type::BYTEA => Self::Bytes,
            Type::UUID => Self::Uuid,
            Type::TIMESTAMPTZ => Self::Timestamp,
            Type::DATE => Self::Date,
            Type::TIMESTAMP => Self::DateTime,
            Type::JSON => Self::Json,
            Type::JSONB => Self::Jsonb,
            _ => Self::named(ty.name()),
        }
    }

    /// An extension's type has no fixed id, so it is known by name.
    pub(crate) fn named(name: &str) -> Self {
        match name {
            "citext" => Self::Text,
            _ => Self::Unsupported,
        }
    }

    pub(crate) fn sql_type(self) -> Option<SqlType> {
        Some(match self {
            Self::Bool => SqlType::Bool,
            Self::Int2 | Self::Int4 | Self::Int8 => SqlType::Int,
            Self::Float4 | Self::Float8 => SqlType::Float,
            Self::Text => SqlType::Text,
            Self::Bytes => SqlType::Bytes,
            Self::Uuid => SqlType::Uuid,
            Self::Timestamp => SqlType::Timestamp,
            Self::Date => SqlType::Date,
            Self::DateTime => SqlType::DateTime,
            Self::Json | Self::Jsonb | Self::Unsupported => return None,
        })
    }
}

/// Hands each row to `rows` as it arrives, so no answer is held whole; returns the row count.
pub(crate) async fn read_rows(
    client: &Client,
    statement: &Statement,
    sql: &str,
    params: &[SqlValue],
    rows: &mut dyn RowSink,
) -> Result<u64> {
    let wires = wires(sql, statement, rows.expects())?;
    let stream = client
        .query_raw(statement, params.iter().map(Encoded))
        .await
        .map_err(|error| statement_error(sql, error))?;
    let mut stream = pin!(stream);
    let mut count = 0;
    while let Some(row) = poll_fn(|context| stream.as_mut().poll_next(context)).await {
        let row = row.map_err(|error| statement_error(sql, error))?;
        rows.row(&mut WireRow {
            row: &row,
            wires: &wires,
            sql,
            next: 0,
        })?;
        count += 1;
    }
    Ok(count)
}

/// Refuses a statement whose columns do not hold what its reader expects before any row is read.
fn wires(sql: &str, statement: &Statement, expects: &[Option<SqlType>]) -> Result<Vec<Wire>> {
    let columns = statement.columns();
    if columns.len() < expects.len() {
        return Err(Error::internal_message(format!(
            "`{sql}` answers {} columns, but its reader expects {}",
            columns.len(),
            expects.len()
        )));
    }
    columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let wire = Wire::of(column.type_());
            match expects.get(index).copied().flatten() {
                Some(expected) if wire.sql_type() != Some(expected) => {
                    Err(Error::internal_message(format!(
                        "column {} of `{sql}` is a {}, but its reader expects {expected:?}",
                        column.name(),
                        column.type_()
                    )))
                }
                _ => Ok(wire),
            }
        })
        .collect()
}

struct Raw<'r>(Option<&'r [u8]>);

impl<'r> FromSql<'r> for Raw<'r> {
    fn from_sql(_: &Type, raw: &'r [u8]) -> WireResult<Self> {
        Ok(Self(Some(raw)))
    }

    fn from_sql_null(_: &Type) -> WireResult<Self> {
        Ok(Self(None))
    }

    fn accepts(_: &Type) -> bool {
        true
    }
}

struct WireRow<'r> {
    row: &'r Row,
    wires: &'r [Wire],
    sql: &'r str,
    next: usize,
}

impl<'r> WireRow<'r> {
    fn raw(&self, index: usize) -> Result<Option<&'r [u8]>> {
        if index >= self.wires.len() {
            return Err(Error::internal_message(format!(
                "a row of `{}` has fewer columns than its reader expects",
                self.sql
            )));
        }
        let raw = self
            .row
            .try_get::<_, Raw>(index)
            .map_err(|error| self.unreadable(index, error))?;
        Ok(raw.0)
    }

    /// Decodes the next column, which must not be null; `None` from `decode` means its type
    /// does not hold what was asked.
    fn read<T>(
        &mut self,
        expected: &str,
        decode: impl FnOnce(Wire, &'r [u8]) -> Option<WireResult<T>>,
    ) -> Result<T> {
        let index = self.next;
        let raw = self.raw(index)?.ok_or_else(|| {
            self.refused(
                index,
                &format!("is null, but its reader expects {expected}"),
            )
        })?;
        self.next += 1;
        let wire = self.wires[index];
        match decode(wire, raw) {
            Some(decoded) => decoded.map_err(|error| self.unreadable(index, error)),
            None => Err(self.refused(
                index,
                &format!(
                    "is a {}, but its reader expects {expected}",
                    self.type_of(index)
                ),
            )),
        }
    }

    fn type_of(&self, index: usize) -> &Type {
        self.row.columns()[index].type_()
    }

    fn refused(&self, index: usize, why: &str) -> Error {
        let column = self.row.columns()[index].name();
        Error::internal_message(format!("column {column} of `{}` {why}", self.sql))
    }

    fn unreadable(&self, index: usize, error: impl Into<Box<dyn StdError + Send + Sync>>) -> Error {
        let column = self.row.columns()[index].name();
        Error::internal(format!("reading column {column} of `{}`", self.sql), error)
    }
}

impl ColumnReader for WireRow<'_> {
    fn remaining(&self) -> usize {
        self.wires.len().saturating_sub(self.next)
    }

    fn is_null_at(&self, ahead: usize) -> Result<bool> {
        Ok(self.raw(self.next + ahead)?.is_none())
    }

    fn skip(&mut self) -> Result<()> {
        self.raw(self.next)?;
        self.next += 1;
        Ok(())
    }

    fn bool(&mut self) -> Result<bool> {
        self.read("a bool", |wire, raw| {
            (wire == Wire::Bool).then(|| bool::from_sql(&Type::BOOL, raw))
        })
    }

    fn int(&mut self) -> Result<i64> {
        self.read("an integer", |wire, raw| match wire {
            Wire::Int2 => Some(i16::from_sql(&Type::INT2, raw).map(i64::from)),
            Wire::Int4 => Some(i32::from_sql(&Type::INT4, raw).map(i64::from)),
            Wire::Int8 => Some(i64::from_sql(&Type::INT8, raw)),
            _ => None,
        })
    }

    fn float(&mut self) -> Result<f64> {
        self.read("a float", |wire, raw| match wire {
            Wire::Float4 => Some(f32::from_sql(&Type::FLOAT4, raw).map(f64::from)),
            Wire::Float8 => Some(f64::from_sql(&Type::FLOAT8, raw)),
            _ => None,
        })
    }

    fn text(&mut self) -> Result<&str> {
        self.read("text", |wire, raw| {
            (wire == Wire::Text).then(|| <&str>::from_sql(&Type::TEXT, raw))
        })
    }

    fn bytes(&mut self) -> Result<&[u8]> {
        self.read("bytes", |wire, raw| {
            (wire == Wire::Bytes).then_some(Ok(raw))
        })
    }

    fn uuid(&mut self) -> Result<Uuid> {
        self.read("a uuid", |wire, raw| {
            (wire == Wire::Uuid).then(|| Ok(Uuid::from_bytes(raw.try_into()?)))
        })
    }

    fn timestamp(&mut self) -> Result<Timestamp> {
        self.read("a timestamp", |wire, raw| {
            (wire == Wire::Timestamp).then(|| decode_timestamp(raw))
        })
    }

    fn date(&mut self) -> Result<Date> {
        self.read("a date", |wire, raw| {
            (wire == Wire::Date).then(|| decode_date(raw))
        })
    }

    fn date_time(&mut self) -> Result<DateTime> {
        self.read("a date-time", |wire, raw| {
            (wire == Wire::DateTime).then(|| decode_date_time(raw))
        })
    }

    fn json(&mut self) -> Result<serde_json::Value> {
        self.read("json", |wire, raw| match wire {
            Wire::Json => Some(serde_json::from_slice(raw).map_err(Into::into)),
            Wire::Jsonb => Some(decode_jsonb(raw)),
            _ => None,
        })
    }

    fn value(&mut self) -> Result<SqlValue> {
        if self.is_null()? {
            self.next += 1;
            return Ok(SqlValue::Null);
        }
        Ok(match self.wires[self.next] {
            Wire::Bool => SqlValue::Bool(self.bool()?),
            Wire::Int2 | Wire::Int4 | Wire::Int8 => SqlValue::Int(self.int()?),
            Wire::Float4 | Wire::Float8 => SqlValue::Float(self.float()?),
            Wire::Text => SqlValue::Text(self.text()?.to_owned()),
            Wire::Bytes => SqlValue::Bytes(self.bytes()?.to_vec()),
            Wire::Uuid => SqlValue::Uuid(self.uuid()?),
            Wire::Timestamp => SqlValue::Timestamp(self.timestamp()?),
            Wire::Date => SqlValue::Date(self.date()?),
            Wire::DateTime => SqlValue::DateTime(self.date_time()?),
            Wire::Json | Wire::Jsonb => SqlValue::Json(self.json()?),
            Wire::Unsupported => {
                let why = format!("is a {}, which no value reads", self.type_of(self.next));
                return Err(self.refused(self.next, &why));
            }
        })
    }
}
