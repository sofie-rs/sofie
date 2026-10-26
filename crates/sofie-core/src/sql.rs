use crate::moves::{Moves, targets};
use crate::{Error, Result, Uuid};
use jiff::Timestamp;
use jiff::civil::{Date, DateTime};

#[derive(Clone, Debug, PartialEq)]
pub enum SqlValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    Bytes(Vec<u8>),
    Uuid(Uuid),
    Timestamp(Timestamp),
    Date(Date),
    DateTime(DateTime),
    Json(serde_json::Value),
    /// Bound as one array parameter; never stored.
    Array(Vec<SqlValue>),
}

impl SqlValue {
    /// What kind of value this is, for errors that must not quote the value.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "bool",
            Self::Int(_) => "integer",
            Self::Float(_) => "float",
            Self::Text(_) => "text",
            Self::Bytes(_) => "bytes",
            Self::Uuid(_) => "uuid",
            Self::Timestamp(_) => "timestamp",
            Self::Date(_) => "date",
            Self::DateTime(_) => "date-time",
            Self::Json(_) => "json",
            Self::Array(_) => "array",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SqlType {
    Bool,
    Int,
    Float,
    Text,
    Bytes,
    Uuid,
    Timestamp,
    Date,
    DateTime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColumnSpec {
    pub suffix: &'static str,
    pub ty: SqlType,
    /// A list of records sorts them by the one column of a value that orders it, such as
    /// an amount's minor units rather than its currency.
    pub orders: bool,
    /// The only texts the column holds, as an enum's variant names; empty for any text.
    pub values: &'static [&'static str],
}

impl ColumnSpec {
    #[must_use]
    pub const fn new(suffix: &'static str, ty: SqlType) -> Self {
        Self {
            suffix,
            ty,
            orders: true,
            values: &[],
        }
    }

    #[must_use]
    pub const fn with_values(self, values: &'static [&'static str]) -> Self {
        Self { values, ..self }
    }

    #[must_use]
    pub const fn unordered(self) -> Self {
        Self {
            orders: false,
            ..self
        }
    }
}

/// Whether field `index` stores a column an earlier field stores; a derive asserts it in a constant.
#[doc(hidden)]
#[must_use]
pub const fn stores_earlier_column(fields: &[(&str, &[ColumnSpec])], index: usize) -> bool {
    let (name, columns) = fields[index];
    let mut earlier = 0;
    while earlier < index {
        let (earlier_name, earlier_columns) = fields[earlier];
        let mut column = 0;
        while column < columns.len() {
            if names_column(earlier_name, earlier_columns, name, columns[column].suffix) {
                return true;
            }
            column += 1;
        }
        earlier += 1;
    }
    false
}

const fn names_column(name: &str, columns: &[ColumnSpec], other: &str, suffix: &str) -> bool {
    let mut column = 0;
    while column < columns.len() {
        if same_column(name, columns[column].suffix, other, suffix) {
            return true;
        }
        column += 1;
    }
    false
}

/// Whether `left` and `left_suffix` name the same column as `right` and `right_suffix`.
#[doc(hidden)]
#[must_use]
pub const fn same_column(left: &str, left_suffix: &str, right: &str, right_suffix: &str) -> bool {
    let length = left.len() + left_suffix.len();
    if length != right.len() + right_suffix.len() {
        return false;
    }
    let mut index = 0;
    while index < length {
        if byte_at(left, left_suffix, index) != byte_at(right, right_suffix, index) {
            return false;
        }
        index += 1;
    }
    true
}

const fn byte_at(name: &str, suffix: &str, index: usize) -> u8 {
    if index < name.len() {
        return name.as_bytes()[index];
    }
    suffix.as_bytes()[index - name.len()]
}

pub trait Cast: Sized {
    const COLUMNS: &'static [ColumnSpec];
    const NULLABLE: bool = false;
    /// A field holding it is never filtered, sorted, listed or searched, as `#[sensitive]` ones.
    const SENSITIVE: bool = false;
    /// Set for a value moving between listed states; a save of any other value skips the check.
    const MOVES: Option<Moves<Self>> = None;
    /// Set when a state anywhere in the value is checked on save, as a field's of several.
    const HOLDS_STATES: bool = Self::MOVES.is_some();

    fn encode(&self, out: &mut Vec<SqlValue>);

    fn decode(row: &mut dyn ColumnReader) -> Result<Self>;

    fn refuse_unlisted_move(field: &str, stored: &Self, current: &Self) -> Result<()> {
        crate::moves::refuse_unlisted(field, stored, current)
    }
}

/// One row's columns in order, each read once straight into its target, text borrowed meanwhile.
pub trait ColumnReader {
    fn remaining(&self) -> usize;

    /// Of the column `ahead` places past the next one, without reading it.
    fn is_null_at(&self, ahead: usize) -> Result<bool>;

    fn is_null(&self) -> Result<bool> {
        self.is_null_at(0)
    }

    fn skip(&mut self) -> Result<()>;

    fn bool(&mut self) -> Result<bool>;

    fn int(&mut self) -> Result<i64>;

    fn float(&mut self) -> Result<f64>;

    fn text(&mut self) -> Result<&str>;

    fn bytes(&mut self) -> Result<&[u8]>;

    fn uuid(&mut self) -> Result<Uuid>;

    fn timestamp(&mut self) -> Result<Timestamp>;

    fn date(&mut self) -> Result<Date>;

    fn date_time(&mut self) -> Result<DateTime>;

    fn json(&mut self) -> Result<serde_json::Value>;

    fn value(&mut self) -> Result<SqlValue>;
}

pub struct RowReader<'a> {
    values: std::slice::Iter<'a, SqlValue>,
}

impl<'a> RowReader<'a> {
    #[must_use]
    pub fn new(values: &'a [SqlValue]) -> Self {
        Self {
            values: values.iter(),
        }
    }

    fn take(&mut self) -> Result<&'a SqlValue> {
        self.values.next().ok_or_else(short_row)
    }

    fn read<T>(
        &mut self,
        expected: &str,
        pick: impl FnOnce(&'a SqlValue) -> Option<T>,
    ) -> Result<T> {
        let value = self.take()?;
        pick(value).ok_or_else(|| mismatch(expected, value))
    }
}

macro_rules! pick {
    ($pattern:pat => $value:expr) => {
        |value| match value {
            $pattern => Some($value),
            _ => None,
        }
    };
}

impl ColumnReader for RowReader<'_> {
    fn remaining(&self) -> usize {
        self.values.len()
    }

    fn is_null_at(&self, ahead: usize) -> Result<bool> {
        let column = self.values.as_slice().get(ahead).ok_or_else(short_row)?;
        Ok(*column == SqlValue::Null)
    }

    fn skip(&mut self) -> Result<()> {
        self.take().map(drop)
    }

    fn bool(&mut self) -> Result<bool> {
        self.read("bool", pick!(SqlValue::Bool(value) => *value))
    }

    fn int(&mut self) -> Result<i64> {
        self.read("integer", pick!(SqlValue::Int(value) => *value))
    }

    fn float(&mut self) -> Result<f64> {
        self.read("float", pick!(SqlValue::Float(value) => *value))
    }

    fn text(&mut self) -> Result<&str> {
        self.read("text", pick!(SqlValue::Text(value) => value.as_str()))
    }

    fn bytes(&mut self) -> Result<&[u8]> {
        self.read("bytes", pick!(SqlValue::Bytes(value) => value.as_slice()))
    }

    fn uuid(&mut self) -> Result<Uuid> {
        self.read("uuid", pick!(SqlValue::Uuid(value) => *value))
    }

    fn timestamp(&mut self) -> Result<Timestamp> {
        self.read("timestamp", pick!(SqlValue::Timestamp(value) => *value))
    }

    fn date(&mut self) -> Result<Date> {
        self.read("date", pick!(SqlValue::Date(value) => *value))
    }

    fn date_time(&mut self) -> Result<DateTime> {
        self.read("date-time", pick!(SqlValue::DateTime(value) => *value))
    }

    fn json(&mut self) -> Result<serde_json::Value> {
        self.read("json", pick!(SqlValue::Json(value) => value.clone()))
    }

    fn value(&mut self) -> Result<SqlValue> {
        self.take().cloned()
    }
}

fn short_row() -> Error {
    Error::internal_message("row has fewer columns than expected")
}

fn mismatch(expected: &str, found: &SqlValue) -> Error {
    Error::internal_message(format!(
        "expected a {expected} column, found a {} value",
        found.kind()
    ))
}

macro_rules! scalar_cast {
    ($type:ty, $sql:ident, |$value:ident| $encode:expr, |$row:ident| $decode:expr) => {
        impl Cast for $type {
            const COLUMNS: &'static [ColumnSpec] = &[ColumnSpec::new("", SqlType::$sql)];

            fn encode(&self, out: &mut Vec<SqlValue>) {
                let $value = self;
                out.push($encode);
            }

            fn decode($row: &mut dyn ColumnReader) -> Result<Self> {
                $decode
            }
        }
    };
}

scalar_cast!(bool, Bool, |value| SqlValue::Bool(*value), |row| row.bool());
scalar_cast!(i64, Int, |value| SqlValue::Int(*value), |row| row.int());
scalar_cast!(i32, Int, |value| SqlValue::Int(i64::from(*value)), |row| {
    i32::try_from(row.int()?).map_err(|error| Error::internal("integer column out of range", error))
});
scalar_cast!(f64, Float, |value| SqlValue::Float(*value), |row| row
    .float());
scalar_cast!(String, Text, |value| SqlValue::Text(value.clone()), |row| {
    row.text().map(str::to_owned)
});
scalar_cast!(Uuid, Uuid, |value| SqlValue::Uuid(*value), |row| row.uuid());
scalar_cast!(
    Timestamp,
    Timestamp,
    |value| SqlValue::Timestamp(*value),
    |row| row.timestamp()
);
scalar_cast!(Date, Date, |value| SqlValue::Date(*value), |row| row.date());
scalar_cast!(
    DateTime,
    DateTime,
    |value| SqlValue::DateTime(*value),
    |row| row.date_time()
);

pub trait Ordered: Cast {}

impl Ordered for i64 {}
impl Ordered for i32 {}
impl Ordered for f64 {}
impl Ordered for String {}
impl Ordered for Timestamp {}
impl Ordered for Date {}
impl Ordered for DateTime {}
impl<T: Ordered> Ordered for Option<T> {}

/// Only all-null columns read back as `None`, so an `Option` of a value that can be all nulls
/// (an `Option`, or a value of several fields that are all optional) is refused when compiled:
///
/// ```compile_fail,E0080
/// use sofie_core::Cast;
///
/// let _ = <Option<Option<i64>>>::COLUMNS;
/// ```
impl<T: Cast> Cast for Option<T> {
    const COLUMNS: &'static [ColumnSpec] = {
        assert!(
            !T::NULLABLE,
            "an `Option` of a nullable value cannot be stored: `Some(None)` would read back as `None`"
        );
        T::COLUMNS
    };
    const NULLABLE: bool = true;
    const SENSITIVE: bool = T::SENSITIVE;
    const MOVES: Option<Moves<Self>> = if T::MOVES.is_some() {
        Some(|value| value.as_ref().and_then(targets))
    } else {
        None
    };
    const HOLDS_STATES: bool = T::HOLDS_STATES;

    fn encode(&self, out: &mut Vec<SqlValue>) {
        match self {
            Some(value) => value.encode(out),
            None => out.extend(Self::COLUMNS.iter().map(|_| SqlValue::Null)),
        }
    }

    fn decode(row: &mut dyn ColumnReader) -> Result<Self> {
        for ahead in 0..Self::COLUMNS.len() {
            if !row.is_null_at(ahead)? {
                return T::decode(row).map(Some);
            }
        }
        for _ in Self::COLUMNS {
            row.skip()?;
        }
        Ok(None)
    }

    fn refuse_unlisted_move(field: &str, stored: &Self, current: &Self) -> Result<()> {
        match (stored, current) {
            (Some(stored), Some(current)) => T::refuse_unlisted_move(field, stored, current),
            _ => crate::moves::refuse_unlisted(field, stored, current),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn option_reads_none_from_null_and_some_from_a_value() {
        let values = [SqlValue::Null, SqlValue::Int(7)];
        let mut row = RowReader::new(&values);

        assert_eq!(Option::<i64>::decode(&mut row).unwrap(), None);
        assert_eq!(Option::<i64>::decode(&mut row).unwrap(), Some(7));
    }

    #[derive(Debug, PartialEq)]
    struct Period {
        from: Option<i64>,
        until: i64,
    }

    impl Cast for Period {
        const COLUMNS: &'static [ColumnSpec] = &[
            ColumnSpec::new("_from", SqlType::Int),
            ColumnSpec::new("_until", SqlType::Int),
        ];

        fn encode(&self, out: &mut Vec<SqlValue>) {
            self.from.encode(out);
            self.until.encode(out);
        }

        fn decode(row: &mut dyn ColumnReader) -> Result<Self> {
            Ok(Self {
                from: Cast::decode(row)?,
                until: Cast::decode(row)?,
            })
        }
    }

    #[test]
    fn an_option_of_several_columns_is_none_only_when_every_column_is_null() {
        let period = Some(Period {
            from: None,
            until: 9,
        });
        let mut values = Vec::new();
        period.encode(&mut values);
        None::<Period>.encode(&mut values);
        values.push(SqlValue::Int(1));
        let mut row = RowReader::new(&values);

        assert_eq!(Option::<Period>::decode(&mut row).unwrap(), period);
        assert_eq!(Option::<Period>::decode(&mut row).unwrap(), None);
        assert_eq!(i64::decode(&mut row).unwrap(), 1);
    }

    #[test]
    fn a_value_of_the_wrong_type_is_an_internal_error_that_does_not_quote_it() {
        let values = [SqlValue::Text("seven".into())];

        let error = i64::decode(&mut RowReader::new(&values)).unwrap_err();

        assert!(matches!(error, Error::Internal { .. }));
        assert!(!error.to_string().contains("seven"), "{error}");
    }

    #[test]
    fn a_short_row_is_refused() {
        let values = [SqlValue::Int(1)];
        let mut row = RowReader::new(&values);

        i64::decode(&mut row).unwrap();

        assert!(matches!(i64::decode(&mut row), Err(Error::Internal { .. })));
        assert!(Option::<i64>::decode(&mut row).is_err());
    }

    #[test]
    fn a_field_storing_an_earlier_field_s_column_is_found() {
        const AMOUNT: &[ColumnSpec] = &[
            ColumnSpec::new("_minor", SqlType::Int),
            ColumnSpec::new("_currency", SqlType::Text),
        ];
        let fields = [
            ("total", AMOUNT),
            ("total_currency", String::COLUMNS),
            ("totals", String::COLUMNS),
            ("note", AMOUNT),
            ("totals", String::COLUMNS),
        ];

        assert!(stores_earlier_column(&fields, 1));
        assert!(!stores_earlier_column(&fields, 2));
        assert!(!stores_earlier_column(&fields, 3));
        assert!(stores_earlier_column(&fields, 4));
    }

    #[test]
    fn an_integer_too_large_for_i32_is_refused() {
        let values = [SqlValue::Int(i64::MAX)];

        assert!(i32::decode(&mut RowReader::new(&values)).is_err());
    }
}
