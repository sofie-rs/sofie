use std::any::type_name;
use std::fmt::{self, Display};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{Cast, ColumnReader, ColumnSpec, Error, Result, SqlType, SqlValue};

pub trait Value: Sized {
    type Inner: Cast;
    const SENSITIVE: bool = false;

    fn inner(&self) -> &Self::Inner;

    fn try_from_inner(inner: Self::Inner) -> Result<Self, impl Display>;

    /// Overridden when an input rule tightens, so rows written under the looser one still read.
    fn decode_stored(inner: Self::Inner) -> Result<Self> {
        stored(Self::try_from_inner(inner))
    }
}

impl<V: Value> Cast for V {
    const COLUMNS: &'static [ColumnSpec] = V::Inner::COLUMNS;
    const NULLABLE: bool = V::Inner::NULLABLE;
    const SENSITIVE: bool = V::SENSITIVE || V::Inner::SENSITIVE;

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.inner().encode(out);
    }

    fn decode(row: &mut dyn ColumnReader) -> Result<Self> {
        V::decode_stored(V::Inner::decode(row)?)
    }
}

/// A stored value its own check refuses is the database's fault, not the caller's.
pub fn stored<V>(built: Result<V, impl Display>) -> Result<V> {
    built.map_err(|error| {
        Error::internal_message(format!(
            "a stored {} is not valid: {error}",
            type_name::<V>()
        ))
    })
}

/// Evaluated in a constant, so a field spanning more than one column fails the build.
#[must_use]
pub const fn single_column(columns: &[ColumnSpec]) -> SqlType {
    assert!(
        columns.len() == 1,
        "every field of a value with several fields must span one column"
    );
    columns[0].ty
}

pub fn write_masked(formatter: &mut fmt::Formatter<'_>, type_name: &str) -> fmt::Result {
    write!(formatter, "{type_name}(***)")
}

pub fn serialize<V: Value, S: Serializer>(value: &V, serializer: S) -> Result<S::Ok, S::Error>
where
    V::Inner: Serialize,
{
    value.inner().serialize(serializer)
}

pub fn deserialize<'de, V: Value, D: Deserializer<'de>>(deserializer: D) -> Result<V, D::Error>
where
    V::Inner: Deserialize<'de>,
{
    let inner = V::Inner::deserialize(deserializer)?;
    V::try_from_inner(inner).map_err(|error| serde::de::Error::custom(format_args!("{error:#}")))
}

#[doc(hidden)]
pub use serde;

/// `from_stored` reads only stored rows, never input: it may take the column's text as it is,
/// and may accept what an earlier, looser rule wrote.
#[doc(hidden)]
#[macro_export]
macro_rules! text_value {
    ($value:ty, $field:ident) => {
        $crate::text_value!($value, $field, |text: String| <$value>::parse(&text));
    };
    ($value:ty, $field:ident, $from_stored:expr) => {
        impl $crate::value::Value for $value {
            type Inner = String;

            fn inner(&self) -> &String {
                &self.$field
            }

            fn try_from_inner(text: String) -> $crate::Result<Self, impl ::std::fmt::Display> {
                <$value>::parse(&text)
            }

            fn decode_stored(text: String) -> $crate::Result<Self> {
                $crate::value::stored($from_stored(text))
            }
        }

        impl $crate::value::serde::Serialize for $value {
            fn serialize<S: $crate::value::serde::Serializer>(
                &self,
                serializer: S,
            ) -> Result<S::Ok, S::Error> {
                $crate::value::serialize(self, serializer)
            }
        }

        impl<'de> $crate::value::serde::Deserialize<'de> for $value {
            fn deserialize<D: $crate::value::serde::Deserializer<'de>>(
                deserializer: D,
            ) -> Result<Self, D::Error> {
                $crate::value::deserialize(deserializer)
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[derive(Debug, PartialEq)]
    struct Even(i64);

    impl Value for Even {
        type Inner = i64;

        fn inner(&self) -> &i64 {
            &self.0
        }

        fn try_from_inner(inner: i64) -> Result<Self, impl Display> {
            if inner % 2 == 0 {
                return Ok(Self(inner));
            }
            Err(Error::invalid("even", "must be even"))
        }
    }

    struct Pin(String);

    impl Value for Pin {
        type Inner = String;
        const SENSITIVE: bool = true;

        fn inner(&self) -> &String {
            &self.0
        }

        fn try_from_inner(inner: String) -> Result<Self, impl Display> {
            Ok::<_, Error>(Self(inner))
        }
    }

    #[test]
    fn a_sensitive_value_is_a_sensitive_column_even_when_optional() {
        const { assert!(<Pin as Cast>::SENSITIVE) };
        const { assert!(<Option<Pin> as Cast>::SENSITIVE) };
        const { assert!(!<Even as Cast>::SENSITIVE) };
    }

    #[test]
    fn is_stored_as_its_inner_value_and_decoded_through_its_check() {
        let mut values = Vec::new();
        Even(4).encode(&mut values);
        values.push(SqlValue::Int(3));
        let mut row = crate::RowReader::new(&values);

        assert_eq!(Even::decode(&mut row).unwrap(), Even(4));
        assert!(matches!(
            Even::decode(&mut row),
            Err(Error::Internal { context, .. }) if context.contains("Even")
        ));
    }

    #[derive(Debug, PartialEq)]
    struct Small(i64);

    impl Value for Small {
        type Inner = i64;

        fn inner(&self) -> &i64 {
            &self.0
        }

        fn try_from_inner(inner: i64) -> Result<Self, impl Display> {
            if inner < 10 {
                return Ok(Self(inner));
            }
            Err(Error::invalid("small", "must be below 10"))
        }

        fn decode_stored(inner: i64) -> Result<Self> {
            Ok(Self(inner))
        }
    }

    #[test]
    fn a_row_written_under_a_looser_rule_reads_while_input_stays_strict() {
        let values = [SqlValue::Int(12)];

        assert_eq!(
            Small::decode(&mut crate::RowReader::new(&values)).unwrap(),
            Small(12)
        );
        assert!(deserialize::<Small, _>(json!(12)).is_err());
    }

    #[test]
    fn deserializing_reports_only_the_refusal_message() {
        let accepted: Even = deserialize(json!(2)).unwrap();
        let refused = deserialize::<Even, _>(json!(3)).unwrap_err();

        assert_eq!(accepted, Even(2));
        assert_eq!(refused.to_string(), "must be even");
    }

    #[test]
    fn masks_a_sensitive_value() {
        struct Pin;
        impl fmt::Debug for Pin {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write_masked(formatter, "Pin")
            }
        }

        assert_eq!(format!("{Pin:?}"), "Pin(***)");
    }

    #[test]
    fn a_field_of_a_value_with_several_fields_spans_one_column() {
        assert_eq!(single_column(i64::COLUMNS), SqlType::Int);
    }
}
