//! A request cannot reach past the aggregate's fields.

#[cfg(feature = "resources")]
mod resources;

use sofie_core::{Error, FieldError, Instant, Result, SqlType, SqlValue};
use sofie_values::{EXACT, ORDERED, Operator, TEXT};

#[cfg(feature = "resources")]
pub use self::resources::delete_softly;
use crate::aggregate::field_columns;
use crate::condition::Kind;
use crate::{Aggregate, Condition, Field, Sort, Source};

#[must_use]
pub fn filterable<A: Aggregate>(name: &str) -> Vec<Operator> {
    let Some(column) = column_of::<A>(name) else {
        return Vec::new();
    };
    let mut offered = operators(column.ty).to_vec();
    if column.nullable && !offered.is_empty() {
        offered.push(Operator::IsEmpty);
    }
    offered
}

/// A list reaches only what its read model shows, so a hidden column cannot be probed by filters.
#[must_use]
pub fn selects<S: Source>(name: &str) -> bool {
    let wanted = if name == "id" {
        vec!["id".to_owned()]
    } else {
        match listed_field::<S::Base>(name) {
            Some(field) => field_columns(std::slice::from_ref(field)),
            None => return false,
        }
    };
    let selected = S::columns();
    wanted.iter().all(|column| {
        selected
            .iter()
            .any(|selection| selection.column_name() == Some(column))
    })
}

fn listed_field<A: Aggregate>(name: &str) -> Option<&'static Field> {
    if A::SENSITIVE.contains(&name) {
        return None;
    }
    A::FIELDS.iter().find(|field| field.name == name)
}

struct FieldColumn {
    name: String,
    ty: SqlType,
    nullable: bool,
    values: &'static [&'static str],
    parse_text: fn(&str) -> Option<Result<SqlValue, FieldError>>,
}

fn operators(ty: SqlType) -> &'static [Operator] {
    match ty {
        SqlType::Text => TEXT,
        SqlType::Int | SqlType::Float | SqlType::Timestamp | SqlType::Date | SqlType::DateTime => {
            ORDERED
        }
        SqlType::Bool | SqlType::Uuid => EXACT,
        SqlType::Bytes => &[],
    }
}

fn column_of<A: Aggregate>(name: &str) -> Option<FieldColumn> {
    if name == "id" {
        return Some(FieldColumn {
            name: "id".to_owned(),
            ty: SqlType::Uuid,
            nullable: false,
            values: &[],
            parse_text: |_| None,
        });
    }
    let field = listed_field::<A>(name)?;
    match field.columns {
        [column] => Some(FieldColumn {
            name: crate::sql::column_name(name, column),
            ty: column.ty,
            nullable: field.nullable,
            values: column.values,
            parse_text: field.parse_text,
        }),
        _ => None,
    }
}

impl<A: Aggregate> Sort<A> {
    #[must_use]
    pub fn by_field(name: &str, descending: bool) -> Option<Self> {
        if let Some(column) = column_of::<A>(name) {
            return Some(Self::new(column.name, descending, column.nullable));
        }
        let field = listed_field::<A>(name)?;
        let mut ordering = field.columns.iter().filter(|column| column.orders);
        match (ordering.next(), ordering.next()) {
            (Some(column), None) => Some(Self::new(
                crate::sql::column_name(name, column),
                descending,
                field.nullable,
            )),
            _ => None,
        }
    }
}

impl<A: Aggregate> Condition<A> {
    /// `Between` takes `low..high`; a value the field refuses is its own error, shown beside it.
    pub fn on_field(name: &str, operator: Operator, text: &str) -> Result<Self> {
        let refused = |why: String| {
            Error::BadRequest(format!("a list cannot be filtered by `{name}`: {why}"))
        };
        if !filterable::<A>(name).contains(&operator) {
            return Err(refused(format!("it offers no `{operator:?}`")));
        }
        let column = column_of::<A>(name).ok_or_else(|| refused("no such field".to_owned()))?;
        let exact = matches!(operator, Operator::Equals | Operator::NotEquals);
        if exact && !column.values.is_empty() && !column.values.contains(&text) {
            let accepted = column.values.join(", ");
            let refusal = FieldError::new(
                name,
                "validation.one_of",
                format!("must be one of: {accepted}"),
            );
            return Err(Error::Invalid(vec![refusal.with("values", accepted)]));
        }
        let value = |text: &str| column.value(name, text);
        let compare = |operator: &'static str, value: SqlValue| Kind::Compare {
            columns: vec![column.name.clone()],
            operator,
            values: vec![value],
        };
        let kind = match operator {
            Operator::Equals => compare("=", value(text)?),
            Operator::NotEquals if column.nullable => {
                Kind::Not(Box::new(compare("IS NOT DISTINCT FROM", value(text)?)))
            }
            Operator::NotEquals => Kind::Not(Box::new(compare("=", value(text)?))),
            Operator::GreaterThan => compare(">", value(text)?),
            Operator::LessThan => compare("<", value(text)?),
            Operator::AtLeast => compare(">=", value(text)?),
            Operator::AtMost => compare("<=", value(text)?),
            Operator::Between => {
                let (low, high) = text.split_once("..").ok_or_else(|| {
                    let refusal = FieldError::new(
                        name,
                        "validation.range",
                        format!("`{text}` is not a range `low..high`"),
                    );
                    Error::Invalid(vec![refusal.with("value", text)])
                })?;
                Kind::Between {
                    column: column.name.clone(),
                    low: value(low)?,
                    high: value(high)?,
                }
            }
            Operator::Contains => Kind::Contains {
                column: column.name.clone(),
                pattern: contains_pattern(text),
            },
            Operator::IsEmpty => Kind::Null(vec![column.name.clone()]),
        };
        Ok(Self::new(kind))
    }
}

impl FieldColumn {
    fn value(&self, name: &str, text: &str) -> Result<SqlValue> {
        match (self.parse_text)(text) {
            Some(read) => read.map_err(|refusal| refused(name, vec![refusal])),
            None if self.ty == SqlType::Timestamp => instant_value(name, text),
            None => parse(self.ty, text)
                .ok_or_else(|| refused(name, vec![crate::aggregate::expected(self.ty)])),
        }
    }
}

fn refused(name: &str, refusals: Vec<FieldError>) -> Error {
    let named = refusals.into_iter().map(|refusal| FieldError {
        field: name.to_owned(),
        ..refusal
    });
    Error::Invalid(named.collect())
}

fn parse(ty: SqlType, text: &str) -> Option<SqlValue> {
    Some(match ty {
        SqlType::Text => SqlValue::Text(text.to_owned()),
        SqlType::Int => SqlValue::Int(text.parse().ok()?),
        SqlType::Float => SqlValue::Float(text.parse().ok()?),
        SqlType::Bool => SqlValue::Bool(text.parse().ok()?),
        SqlType::Uuid => SqlValue::Uuid(text.parse().ok()?),
        SqlType::Date => SqlValue::Date(text.parse().ok()?),
        SqlType::DateTime => SqlValue::DateTime(text.parse().ok()?),
        SqlType::Timestamp | SqlType::Bytes => return None,
    })
}

/// A wall clock a backward shift repeats is refused naming both offsets, which a text box can hold.
fn instant_value(name: &str, text: &str) -> Result<SqlValue> {
    let instant = Instant::parse(text).map_err(|error| match error {
        Error::Invalid(refusals) => refused(name, refusals),
        other => other,
    })?;
    Ok(SqlValue::Timestamp(instant.timestamp()))
}

/// The text matched literally anywhere: `%`, `_` and the escape itself are escaped.
fn contains_pattern(text: &str) -> String {
    let mut pattern = String::with_capacity(text.len() + 4);
    pattern.push('%');
    for character in text.chars() {
        if matches!(character, '\\' | '%' | '_') {
            pattern.push('\\');
        }
        pattern.push(character);
    }
    pattern.push('%');
    pattern
}

#[cfg(test)]
mod tests {
    use std::marker::PhantomData;

    use sofie_core::{Cast as _, ColumnSpec};

    use super::*;
    use crate::{ReadsNoText as _, ReadsText as _};

    #[test]
    fn a_filter_reads_its_value_as_the_column_s_type() {
        assert_eq!(parse(SqlType::Int, "12"), Some(SqlValue::Int(12)));
        assert_eq!(parse(SqlType::Int, "twelve"), None);
        assert_eq!(parse(SqlType::Bool, "true"), Some(SqlValue::Bool(true)));
        assert_eq!(contains_pattern("50%_\\"), "%50\\%\\_\\\\%");
    }

    #[test]
    fn an_instant_filter_reads_a_datetime_local_value_as_an_instant_does() {
        let instant = sofie_core::Instant::parse("2026-10-16T10:00").unwrap();
        assert_eq!(
            instant_value("due", "2026-10-16T10:00").unwrap(),
            SqlValue::Timestamp(instant.timestamp())
        );
    }

    struct Priced(crate::Meta);

    const AMOUNT: &[ColumnSpec] = &[
        ColumnSpec::new("", SqlType::Int),
        ColumnSpec::new("_currency", SqlType::Text).unordered(),
    ];

    impl Aggregate for Priced {
        const NAME: &'static str = "priced";
        const MODULE: &'static str = "shop";
        const FIELDS: &'static [crate::Field] = &[
            crate::Field::new("total", AMOUNT, false),
            crate::Field::new("token", &[ColumnSpec::new("", SqlType::Text)], false),
            crate::Field::new("note", &[ColumnSpec::new("", SqlType::Text)], true),
            crate::Field::new("due", &[ColumnSpec::new("", SqlType::Timestamp)], false),
        ];
        const SENSITIVE: &'static [&'static str] = &["token"];

        fn meta(&self) -> &crate::Meta {
            &self.0
        }

        fn meta_mut(&mut self) -> &mut crate::Meta {
            &mut self.0
        }

        fn encode(&self, _out: &mut Vec<SqlValue>) {}

        fn decode(meta: crate::Meta, _row: &mut dyn sofie_core::ColumnReader) -> Result<Self> {
            Ok(Self(meta))
        }
    }

    #[test]
    fn a_value_of_several_columns_sorts_by_the_one_that_orders_it_and_filters_by_none() {
        assert!(Sort::<Priced>::by_field("total", true).is_some());
        assert_eq!(filterable::<Priced>("total"), []);
    }

    #[test]
    fn a_sensitive_field_is_neither_filtered_nor_sorted_by() {
        assert_eq!(filterable::<Priced>("token"), []);
        assert!(Sort::<Priced>::by_field("token", false).is_none());
        assert!(!selects::<Priced>("token"));
        assert!(Condition::<Priced>::on_field("token", Operator::Equals, "a").is_err());
    }

    #[test]
    fn a_nullable_field_unequal_to_a_value_keeps_its_empty_rows() {
        let condition = Condition::<Priced>::on_field("note", Operator::NotEquals, "gift").unwrap();
        let (mut sql, mut params) = (String::new(), Vec::new());
        condition
            .kind
            .render_into(&crate::aggregate::tests::NUMBERED, &mut params, &mut sql);

        assert_eq!(sql, "NOT (\"note\" IS NOT DISTINCT FROM $1)");
    }

    #[test]
    fn an_instant_filter_refused_is_an_error_of_its_field_saying_why() {
        let refused = Condition::<Priced>::on_field("due", Operator::AtLeast, "yesterday").err();

        let Some(Error::Invalid(errors)) = &refused else {
            panic!("{refused:?}");
        };
        assert_eq!(errors[0].field, "due");
        assert!(errors[0].message.contains("with an offset"), "{refused:?}");
    }

    struct Tenths(i64);

    struct Count(i64);

    macro_rules! int_column {
        ($ty:ident) => {
            impl sofie_core::Cast for $ty {
                const COLUMNS: &'static [ColumnSpec] = &[ColumnSpec::new("", SqlType::Int)];

                fn encode(&self, out: &mut Vec<SqlValue>) {
                    out.push(SqlValue::Int(self.0));
                }

                fn decode(_row: &mut dyn sofie_core::ColumnReader) -> Result<Self> {
                    Ok(Self(0))
                }
            }
        };
    }

    int_column!(Tenths);
    int_column!(Count);
    int_column!(Hundredths);

    struct Hundredths(i64);

    impl<'de> serde::Deserialize<'de> for Hundredths {
        fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let whole: i64 = String::deserialize(deserializer)?
                .parse()
                .map_err(serde::de::Error::custom)?;
            if !(0..=100).contains(&whole) {
                return Err(serde::de::Error::custom("must be at most 100"));
            }
            Ok(Self(whole * 100))
        }
    }

    impl<'de> serde::Deserialize<'de> for Tenths {
        fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let text = String::deserialize(deserializer)?;
            let tenths = text
                .replace('.', "")
                .parse()
                .map_err(serde::de::Error::custom)?;
            Ok(Self(tenths))
        }
    }

    struct Rated(crate::Meta);

    impl Aggregate for Rated {
        const NAME: &'static str = "rated";
        const MODULE: &'static str = "shop";
        #[expect(
            clippy::needless_borrow,
            reason = "as the derive writes it, for every type"
        )]
        const FIELDS: &'static [crate::Field] = &[
            crate::Field::new("rate", Tenths::COLUMNS, false)
                .parsed_by(|text| (&&crate::TextOf::<Tenths>(PhantomData)).read(text)),
            crate::Field::new("count", Count::COLUMNS, false)
                .parsed_by(|text| (&&crate::TextOf::<Count>(PhantomData)).read(text)),
            crate::Field::new("total", i64::COLUMNS, false)
                .parsed_by(|text| (&&crate::TextOf::<i64>(PhantomData)).read(text)),
            crate::Field::new("share", Hundredths::COLUMNS, false)
                .parsed_by(|text| (&&crate::TextOf::<Hundredths>(PhantomData)).read(text)),
            crate::Field::new("open", bool::COLUMNS, false)
                .parsed_by(|text| (&&crate::TextOf::<bool>(PhantomData)).read(text)),
            crate::Field::new("due", Instant::COLUMNS, false)
                .parsed_by(|text| (&&crate::TextOf::<Instant>(PhantomData)).read(text)),
            crate::Field::new("owner", sofie_core::Uuid::COLUMNS, false)
                .parsed_by(|text| (&&crate::TextOf::<sofie_core::Uuid>(PhantomData)).read(text)),
        ];

        fn meta(&self) -> &crate::Meta {
            &self.0
        }

        fn meta_mut(&mut self) -> &mut crate::Meta {
            &mut self.0
        }

        fn encode(&self, _out: &mut Vec<SqlValue>) {}

        fn decode(meta: crate::Meta, _row: &mut dyn sofie_core::ColumnReader) -> Result<Self> {
            Ok(Self(meta))
        }
    }

    fn filtered_values(name: &str, text: &str) -> Vec<SqlValue> {
        let condition = Condition::<Rated>::on_field(name, Operator::AtLeast, text).unwrap();
        let (mut sql, mut params) = (String::new(), Vec::new());
        condition
            .kind
            .render_into(&crate::aggregate::tests::NUMBERED, &mut params, &mut sql);
        params
    }

    #[test]
    fn a_filter_reads_its_value_as_the_field_s_type_reads_text() {
        assert_eq!(filtered_values("rate", "1.9"), [SqlValue::Int(19)]);
        assert_eq!(filtered_values("count", "12"), [SqlValue::Int(12)]);
        assert_eq!(filtered_values("total", "12"), [SqlValue::Int(12)]);
        assert_eq!(filtered_values("share", "19"), [SqlValue::Int(1900)]);
    }

    fn refusal_of<A: Aggregate>(name: &str, operator: Operator, text: &str) -> String {
        refused_by::<A>(name, operator, text).message
    }

    fn refused_by<A: Aggregate>(name: &str, operator: Operator, text: &str) -> FieldError {
        match Condition::<A>::on_field(name, operator, text).err() {
            Some(Error::Invalid(mut errors)) if errors.len() == 1 && errors[0].field == name => {
                errors.remove(0)
            }
            other => panic!("`{name}` refusing `{text}` gave {other:?}"),
        }
    }

    #[test]
    fn a_value_the_field_s_type_refuses_is_not_read_in_the_column_s_units() {
        let refused = refusal_of::<Rated>("share", Operator::AtLeast, "150");
        assert!(refused.contains("at most 100"), "{refused}");
        refusal_of::<Rated>("total", Operator::AtLeast, "twelve");
        let open = Condition::<Rated>::on_field("open", Operator::Equals, "true").unwrap();
        let (mut sql, mut params) = (String::new(), Vec::new());
        open.kind
            .render_into(&crate::aggregate::tests::NUMBERED, &mut params, &mut sql);
        assert_eq!(params, [SqlValue::Bool(true)]);
    }

    #[test]
    fn every_filter_value_refused_is_an_error_of_its_field() {
        refusal_of::<Rated>("due", Operator::Between, "2026-10-16T10:00");
        refusal_of::<Rated>("open", Operator::Equals, "maybe");
        refusal_of::<Priced>("due", Operator::Between, "a..b");
        refusal_of::<Priced>("id", Operator::Equals, "not-a-uuid");
    }

    #[test]
    fn a_value_not_of_its_column_s_kind_is_refused_in_neutral_words() {
        let rated = |name, text| refusal_of::<Rated>(name, Operator::Equals, text);

        assert_eq!(rated("total", "twelve"), "must be a whole number");
        assert_eq!(rated("total", "1.5"), "must be a whole number");
        assert_eq!(rated("open", "maybe"), "must be true or false");
        assert_eq!(rated("owner", "not-a-uuid"), "must be an id");
        assert_eq!(rated("owner", "7"), "must be an id");
        assert_eq!(
            refusal_of::<Priced>("id", Operator::Equals, "7"),
            "must be an id"
        );
    }

    #[test]
    fn a_refusal_carries_the_key_and_arguments_a_page_translates_it_by() {
        let rated = |name, operator, text| refused_by::<Rated>(name, operator, text);

        assert_eq!(
            rated("total", Operator::Equals, "twelve").key,
            "validation.kind.whole_number"
        );
        assert_eq!(
            rated("open", Operator::Equals, "maybe").key,
            "validation.kind.true_or_false"
        );
        assert_eq!(
            rated("due", Operator::AtLeast, "yesterday").key,
            "validation.instant.invalid"
        );
        let range = rated("total", Operator::Between, "12");
        assert_eq!(range.key, "validation.range");
        assert_eq!(range.arguments, [("value".into(), "12".to_owned())]);
        let undated = refused_by::<Priced>("due", Operator::AtLeast, "yesterday");
        assert_eq!(undated.key, "validation.instant.invalid");
    }

    #[test]
    fn a_derived_instant_filter_refused_says_what_reading_the_instant_says() {
        let refused = refusal_of::<Rated>("due", Operator::AtLeast, "yesterday");
        let read = Instant::parse("yesterday").unwrap_err();
        assert_eq!(refused, format!("{read:#}"));
    }
}
