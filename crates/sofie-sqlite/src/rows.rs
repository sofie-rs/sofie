use rusqlite::types::ValueRef;
use rusqlite::{CachedStatement, Connection, Row, Statement};
use sofie_core::{ColumnReader, Date, DateTime, Error, Result, SqlType, SqlValue, Timestamp, Uuid};
use sofie_db::RowSink;

use crate::statement_error;
use crate::value::{date, date_time, stored, timestamp};

/// What a column holds, read from its declared type, since SQLite stores values by kind
/// rather than by type; an expression has none, and is read as it is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Bool,
    Int,
    Float,
    Text,
    Bytes,
    Uuid,
    Timestamp,
    Date,
    DateTime,
    Json,
    AsStored,
}

impl Kind {
    pub(crate) fn declared(declared: Option<&str>) -> Self {
        let Some(declared) = declared else {
            return Self::AsStored;
        };
        match declared.to_ascii_lowercase().as_str() {
            "bool" | "boolean" => Self::Bool,
            "uuid" => Self::Uuid,
            "timestamptz" | "timestamp with time zone" => Self::Timestamp,
            "timestamp" | "timestamp without time zone" | "datetime" => Self::DateTime,
            "date" => Self::Date,
            "json" | "jsonb" => Self::Json,
            "blob" | "bytea" => Self::Bytes,
            "real" | "float" | "double" | "double precision" | "numeric" | "decimal" => Self::Float,
            other if other.contains("int") => Self::Int,
            other if other.contains("char") || other.contains("text") || other.contains("clob") => {
                Self::Text
            }
            _ => Self::AsStored,
        }
    }

    pub(crate) fn sql_type(self) -> Option<SqlType> {
        Some(match self {
            Self::Bool => SqlType::Bool,
            Self::Int => SqlType::Int,
            Self::Float => SqlType::Float,
            Self::Text => SqlType::Text,
            Self::Bytes => SqlType::Bytes,
            Self::Uuid => SqlType::Uuid,
            Self::Timestamp => SqlType::Timestamp,
            Self::Date => SqlType::Date,
            Self::DateTime => SqlType::DateTime,
            Self::Json | Self::AsStored => return None,
        })
    }
}

pub(crate) fn declared_type(declared: Option<&str>) -> Option<SqlType> {
    Kind::declared(declared).sql_type()
}

/// Statements separated by `;` at a line's end run in order, the last one's rows read:
/// SQLite runs one statement at a time and has no statement that writes inside a query.
/// Several run in one transaction, so a failure part way leaves none of their writes.
pub(crate) fn read(
    connection: &Connection,
    sql: &str,
    params: &[SqlValue],
    rows: &mut dyn RowSink,
) -> Result<u64> {
    if !sql.contains(";\n") {
        return read_statements(connection, sql, params, rows);
    }
    crate::transaction::immediately(connection, |transaction| {
        read_statements(transaction, sql, params, rows)
    })
}

fn read_statements(
    connection: &Connection,
    sql: &str,
    params: &[SqlValue],
    rows: &mut dyn RowSink,
) -> Result<u64> {
    let mut statements: Vec<&str> = sql.split(";\n").collect();
    let last = statements.pop().unwrap_or_default();
    for earlier in statements {
        let mut statement = prepared(connection, earlier)?;
        bind(&mut statement, params)?;
        statement
            .raw_execute()
            .map_err(|error| statement_error(connection, earlier, error))?;
    }
    let mut statement = prepared(connection, last)?;
    bind(&mut statement, params)?;
    let kinds = kinds(&statement, last, rows.expects())?;
    let mut answered = statement.raw_query();
    let mut count = 0;
    while let Some(row) = answered
        .next()
        .map_err(|error| statement_error(connection, last, error))?
    {
        rows.row(&mut Columns {
            row,
            kinds: &kinds,
            sql: last,
            next: 0,
        })?;
        count += 1;
    }
    Ok(count)
}

/// Writes are refused while it runs, as nothing else keeps raw SQL from writing; one
/// statement only, so an earlier one cannot switch writes back on.
#[cfg(feature = "raw-sql")]
pub(crate) fn read_only(
    connection: &Connection,
    sql: &str,
    params: &[SqlValue],
    rows: &mut dyn RowSink,
) -> Result<u64> {
    if sql.contains(";\n") {
        return Err(Error::internal_message(format!(
            "raw SQL runs one statement, so `{sql}` was not run"
        )));
    }
    let set = |pragma: &str| {
        connection
            .execute_batch(pragma)
            .map_err(|error| Error::internal("switching writes off and on", error))
    };
    set("PRAGMA query_only = ON")?;
    // A panicking decoder must not leave the shared connection refusing every later write.
    let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        read(connection, sql, params, rows)
    }));
    set("PRAGMA query_only = OFF")?;
    read.unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

pub(crate) fn prepared<'c>(connection: &'c Connection, sql: &str) -> Result<CachedStatement<'c>> {
    connection
        .prepare_cached(sql)
        .map_err(|error| statement_error(connection, sql, error))
}

/// `?NNN` names its parameter, so each statement binds those it numbers.
pub(crate) fn bind(statement: &mut Statement<'_>, params: &[SqlValue]) -> Result<()> {
    for index in 1..=statement.parameter_count() {
        let value = params.get(index - 1).ok_or_else(|| {
            Error::internal_message(format!(
                "the statement reads parameter {index}, but {} were given",
                params.len()
            ))
        })?;
        statement
            .raw_bind_parameter(index, stored(value)?)
            .map_err(|error| Error::internal(format!("binding parameter {index}"), error))?;
    }
    Ok(())
}

fn kinds(statement: &Statement<'_>, sql: &str, expects: &[Option<SqlType>]) -> Result<Vec<Kind>> {
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
            let kind = Kind::declared(column.decl_type());
            match (expects.get(index).copied().flatten(), kind.sql_type()) {
                (Some(expected), Some(declared)) if expected != declared => {
                    Err(Error::internal_message(format!(
                        "column {} of `{sql}` is a {declared:?}, but its reader expects {expected:?}",
                        column.name()
                    )))
                }
                _ => Ok(kind),
            }
        })
        .collect()
}

struct Columns<'r> {
    row: &'r Row<'r>,
    kinds: &'r [Kind],
    sql: &'r str,
    next: usize,
}

impl<'r> Columns<'r> {
    fn stored(&self, index: usize) -> Result<ValueRef<'r>> {
        if index >= self.kinds.len() {
            return Err(Error::internal_message(format!(
                "a row of `{}` has fewer columns than its reader expects",
                self.sql
            )));
        }
        self.row.get_ref(index).map_err(|error| {
            Error::internal(format!("reading column {index} of `{}`", self.sql), error)
        })
    }

    fn read<T>(
        &mut self,
        expected: &str,
        decode: impl FnOnce(ValueRef<'r>) -> Option<Result<T>>,
    ) -> Result<T> {
        let index = self.next;
        let stored = self.stored(index)?;
        if stored == ValueRef::Null {
            return Err(self.refused(
                index,
                &format!("is null, but its reader expects {expected}"),
            ));
        }
        self.next += 1;
        decode(stored).unwrap_or_else(|| {
            Err(self.refused(
                index,
                &format!(
                    "holds {:?}, but its reader expects {expected}",
                    stored.data_type()
                ),
            ))
        })
    }

    fn refused(&self, index: usize, why: &str) -> Error {
        Error::internal_message(format!("column {index} of `{}` {why}", self.sql))
    }

    fn text_of(stored: ValueRef<'r>) -> Option<Result<&'r str>> {
        match stored {
            ValueRef::Text(text) => Some(
                std::str::from_utf8(text)
                    .map_err(|error| Error::internal("reading stored text", error)),
            ),
            _ => None,
        }
    }
}

impl ColumnReader for Columns<'_> {
    fn remaining(&self) -> usize {
        self.kinds.len().saturating_sub(self.next)
    }

    fn is_null_at(&self, ahead: usize) -> Result<bool> {
        Ok(self.stored(self.next + ahead)? == ValueRef::Null)
    }

    fn skip(&mut self) -> Result<()> {
        self.stored(self.next)?;
        self.next += 1;
        Ok(())
    }

    fn bool(&mut self) -> Result<bool> {
        self.read("a bool", |stored| match stored {
            ValueRef::Integer(number) => Some(Ok(number != 0)),
            _ => None,
        })
    }

    fn int(&mut self) -> Result<i64> {
        self.read("an integer", |stored| match stored {
            ValueRef::Integer(number) => Some(Ok(number)),
            _ => None,
        })
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "SQLite keeps a whole real as an integer; it is read back as the real it was"
    )]
    fn float(&mut self) -> Result<f64> {
        self.read("a float", |stored| match stored {
            ValueRef::Real(number) => Some(Ok(number)),
            ValueRef::Integer(number) => Some(Ok(number as f64)),
            _ => None,
        })
    }

    fn text(&mut self) -> Result<&str> {
        self.read("text", Self::text_of)
    }

    fn bytes(&mut self) -> Result<&[u8]> {
        self.read("bytes", |stored| match stored {
            ValueRef::Blob(bytes) => Some(Ok(bytes)),
            _ => None,
        })
    }

    fn uuid(&mut self) -> Result<Uuid> {
        self.read("a uuid", |stored| {
            Some(Self::text_of(stored)?.and_then(|text| {
                text.parse()
                    .map_err(|error| Error::internal("reading stored text as a uuid", error))
            }))
        })
    }

    fn timestamp(&mut self) -> Result<Timestamp> {
        self.read("a timestamp", |stored| match stored {
            ValueRef::Integer(micros) => Some(timestamp(micros)),
            _ => None,
        })
    }

    fn date(&mut self) -> Result<Date> {
        self.read("a date", |stored| {
            Some(Self::text_of(stored)?.and_then(date))
        })
    }

    fn date_time(&mut self) -> Result<DateTime> {
        self.read("a date-time", |stored| {
            Some(Self::text_of(stored)?.and_then(date_time))
        })
    }

    /// JSON declared on a column of numeric affinity may come back as the number it held.
    fn json(&mut self) -> Result<serde_json::Value> {
        self.read("json", |stored| match stored {
            ValueRef::Integer(number) => Some(Ok(number.into())),
            ValueRef::Real(number) => Some(Ok(number.into())),
            _ => Some(Self::text_of(stored)?.and_then(|text| {
                serde_json::from_str(text)
                    .map_err(|error| Error::internal("reading stored JSON", error))
            })),
        })
    }

    fn value(&mut self) -> Result<SqlValue> {
        let stored = self.stored(self.next)?;
        if stored == ValueRef::Null {
            self.next += 1;
            return Ok(SqlValue::Null);
        }
        Ok(match self.kinds[self.next] {
            Kind::Bool => SqlValue::Bool(self.bool()?),
            Kind::Int => SqlValue::Int(self.int()?),
            Kind::Float => SqlValue::Float(self.float()?),
            Kind::Text => SqlValue::Text(self.text()?.to_owned()),
            Kind::Bytes => SqlValue::Bytes(self.bytes()?.to_vec()),
            Kind::Uuid => SqlValue::Uuid(self.uuid()?),
            Kind::Timestamp => SqlValue::Timestamp(self.timestamp()?),
            Kind::Date => SqlValue::Date(self.date()?),
            Kind::DateTime => SqlValue::DateTime(self.date_time()?),
            Kind::Json => SqlValue::Json(self.json()?),
            Kind::AsStored => {
                self.next += 1;
                as_stored(stored)?
            }
        })
    }
}

fn as_stored(stored: ValueRef<'_>) -> Result<SqlValue> {
    Ok(match stored {
        ValueRef::Null => SqlValue::Null,
        ValueRef::Integer(number) => SqlValue::Int(number),
        ValueRef::Real(number) => SqlValue::Float(number),
        ValueRef::Text(text) => SqlValue::Text(
            std::str::from_utf8(text)
                .map_err(|error| Error::internal("reading stored text", error))?
                .to_owned(),
        ),
        ValueRef::Blob(bytes) => SqlValue::Bytes(bytes.to_vec()),
    })
}

#[cfg(test)]
mod tests {
    use sofie_db::{Decoding, row_values};

    use super::*;

    #[test]
    fn a_batch_that_fails_part_way_writes_nothing() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE note (title text)")
            .unwrap();
        let mut rows = Decoding::new(Vec::new(), row_values);

        let failed = read(
            &connection,
            "INSERT INTO note VALUES ('x');\nINSERT INTO missing VALUES (1);\nSELECT 1",
            &[],
            &mut rows,
        );

        assert!(failed.is_err());
        let count: i64 = connection
            .query_row("SELECT count(*) FROM note", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[cfg(feature = "raw-sql")]
    struct Panicking;

    #[cfg(feature = "raw-sql")]
    impl RowSink for Panicking {
        fn expects(&self) -> &[Option<SqlType>] {
            &[]
        }

        fn row(&mut self, _row: &mut dyn ColumnReader) -> Result<()> {
            panic!("the decoder broke")
        }
    }

    #[cfg(feature = "raw-sql")]
    #[test]
    fn a_panicking_read_only_query_leaves_writes_switched_back_on() {
        let connection = Connection::open_in_memory().unwrap();

        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            read_only(&connection, "SELECT 1", &[], &mut Panicking)
        }));

        assert!(panicked.is_err());
        let query_only: i64 = connection
            .query_row("PRAGMA query_only", [], |row| row.get(0))
            .unwrap();
        assert_eq!(query_only, 0);
    }

    #[cfg(feature = "raw-sql")]
    #[test]
    fn raw_sql_cannot_switch_writes_back_on_with_an_earlier_statement() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE note (title text)")
            .unwrap();
        let mut rows = Decoding::new(Vec::new(), row_values);

        let refused = read_only(
            &connection,
            "PRAGMA query_only = OFF;\nINSERT INTO note VALUES ('x') RETURNING title",
            &[],
            &mut rows,
        );

        assert!(refused.is_err());
        let count: i64 = connection
            .query_row("SELECT count(*) FROM note", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
}
