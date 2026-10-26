use std::marker::PhantomData;

mod render;

use sofie_core::{Cast, ColumnSpec, Ordered, SqlType, SqlValue};

use crate::Aggregate;
use crate::statement::SortKey;

pub struct Column<A, T> {
    field: &'static str,
    marker: PhantomData<fn() -> (A, T)>,
}

impl<A, T> Clone for Column<A, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<A, T> Copy for Column<A, T> {}

impl<A, T> Column<A, T> {
    #[must_use]
    pub const fn new(field: &'static str) -> Self {
        Self {
            field,
            marker: PhantomData,
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        self.field
    }
}

impl<A: Aggregate, T: Cast> Column<A, T> {
    /// `None` matches the rows where the value is null.
    #[must_use]
    pub fn eq(self, value: T) -> Condition<A> {
        Condition::new(compare(self.field, "=", value))
    }

    /// Also matches the rows holding a null where `value` does not, as a nullable value's.
    #[must_use]
    pub fn ne(self, value: T) -> Condition<A> {
        let operator = if T::NULLABLE || T::COLUMNS.len() > 1 {
            "IS NOT DISTINCT FROM"
        } else {
            "="
        };
        Condition::new(Kind::Not(Box::new(compare(self.field, operator, value))))
    }

    /// An empty list matches nothing; a value holding a null matches as in `eq`.
    #[must_use]
    pub fn in_(self, values: impl IntoIterator<Item = T>) -> Condition<A> {
        let columns = names::<T>(self.field);
        let (holding_null, rows): (Vec<_>, Vec<_>) = values
            .into_iter()
            .map(encoded)
            .partition(|row| row.contains(&SqlValue::Null));
        let listed = Kind::In {
            columns: columns.clone(),
            types: T::COLUMNS.iter().map(|column| column.ty).collect(),
            rows,
        };
        let kind = holding_null.into_iter().fold(listed, |kind, values| {
            let equal = Kind::Compare {
                columns: columns.clone(),
                operator: "=",
                values,
            };
            Kind::Or(Box::new(kind), Box::new(equal))
        });
        Condition::new(kind)
    }

    #[must_use]
    pub fn like(self, pattern: &str) -> Condition<A> {
        const { assert!(is_text(T::COLUMNS), "`like` applies to text columns only") };
        Condition::new(Kind::Compare {
            columns: vec![self.field.to_owned()],
            operator: "LIKE",
            values: vec![SqlValue::Text(pattern.to_owned())],
        })
    }

    #[must_use]
    pub fn asc(self) -> Sort<A> {
        Sort::new(names::<T>(self.field).swap_remove(0), false, T::NULLABLE)
    }

    #[must_use]
    pub fn desc(self) -> Sort<A> {
        Sort::new(names::<T>(self.field).swap_remove(0), true, T::NULLABLE)
    }
}

/// Held where `column` equals `selected` of a row of `B` matching `condition`, in the same query.
#[must_use]
pub fn in_select<A: Aggregate, B: Aggregate, T: Cast>(
    column: Column<A, T>,
    selected: Column<B, T>,
    condition: Condition<B>,
) -> Condition<A> {
    const {
        assert!(
            T::COLUMNS.len() == 1,
            "`in_select` applies to one-column values"
        );
    };
    Condition::new(Kind::InSelect(Box::new(InSelect {
        column: column.field.to_owned(),
        selected: selected.field.to_owned(),
        table: (B::MODULE, B::NAME),
        condition: condition.kind,
        hidden: crate::aggregate::table::<B>().hiding().collect(),
    })))
}

fn names<T: Cast>(field: &str) -> Vec<String> {
    T::COLUMNS
        .iter()
        .map(|column| crate::sql::column_name(field, column))
        .collect()
}

fn compare<T: Cast>(field: &str, operator: &'static str, value: T) -> Kind {
    Kind::Compare {
        columns: names::<T>(field),
        operator,
        values: encoded(value),
    }
}

const fn is_text(columns: &[ColumnSpec]) -> bool {
    matches!(
        columns,
        [ColumnSpec {
            ty: SqlType::Text,
            ..
        }]
    )
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "call sites read `eq(Status::Paid)`"
)]
fn encoded<T: Cast>(value: T) -> Vec<SqlValue> {
    let mut values = Vec::with_capacity(T::COLUMNS.len());
    value.encode(&mut values);
    values
}

/// Of a nullable value, a null row never matches: give `is_null` its own condition to keep them.
impl<A: Aggregate, T: Ordered> Column<A, T> {
    #[must_use]
    pub fn gt(self, value: impl Into<T>) -> Condition<A> {
        Condition::new(compare(self.field, ">", value.into()))
    }

    #[must_use]
    pub fn gte(self, value: impl Into<T>) -> Condition<A> {
        Condition::new(compare(self.field, ">=", value.into()))
    }

    #[must_use]
    pub fn lt(self, value: impl Into<T>) -> Condition<A> {
        Condition::new(compare(self.field, "<", value.into()))
    }

    #[must_use]
    pub fn lte(self, value: impl Into<T>) -> Condition<A> {
        Condition::new(compare(self.field, "<=", value.into()))
    }

    /// Both ends included.
    #[must_use]
    pub fn between(self, low: impl Into<T>, high: impl Into<T>) -> Condition<A> {
        Condition::new(Kind::Between {
            column: self.field.to_owned(),
            low: encoded(low.into()).swap_remove(0),
            high: encoded(high.into()).swap_remove(0),
        })
    }
}

impl<A: Aggregate, T: Cast> Column<A, Option<T>> {
    #[must_use]
    pub fn is_null(self) -> Condition<A> {
        Condition::new(Kind::Null(names::<T>(self.field)))
    }

    #[must_use]
    pub fn is_not_null(self) -> Condition<A> {
        Condition::new(Kind::Not(Box::new(Kind::Null(names::<T>(self.field)))))
    }
}

pub struct Condition<A> {
    pub(crate) kind: Kind,
    marker: PhantomData<fn() -> A>,
}

impl<A> Condition<A> {
    pub(crate) const fn new(kind: Kind) -> Self {
        Self {
            kind,
            marker: PhantomData,
        }
    }

    #[must_use]
    pub fn or(self, other: Self) -> Self {
        Self::new(Kind::Or(Box::new(self.kind), Box::new(other.kind)))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Kind {
    Compare {
        columns: Vec<String>,
        operator: &'static str,
        values: Vec<SqlValue>,
    },
    Between {
        column: String,
        low: SqlValue,
        high: SqlValue,
    },
    In {
        columns: Vec<String>,
        types: Vec<SqlType>,
        rows: Vec<Vec<SqlValue>>,
    },
    Null(Vec<String>),
    #[cfg(feature = "listing")]
    Contains {
        column: String,
        pattern: String,
    },
    InSelect(Box<InSelect>),
    Not(Box<Kind>),
    Or(Box<Kind>, Box<Kind>),
    #[cfg(feature = "search")]
    TextSearch {
        column: String,
        words: String,
        #[cfg(feature = "search-engine")]
        index: (&'static str, &'static str),
    },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct InSelect {
    column: String,
    selected: String,
    table: (&'static str, &'static str),
    condition: Kind,
    hidden: Vec<&'static str>,
}

#[cfg(feature = "live")]
impl Kind {
    pub(crate) fn tables(&self, found: &mut Vec<(&'static str, &'static str)>) {
        match self {
            Self::InSelect(select) => {
                found.push(select.table);
                select.condition.tables(found);
            }
            Self::Not(kind) => kind.tables(found),
            Self::Or(left, right) => {
                left.tables(found);
                right.tables(found);
            }
            Self::Compare { .. } | Self::Between { .. } | Self::In { .. } | Self::Null(_) => {}
            #[cfg(feature = "listing")]
            Self::Contains { .. } => {}
            #[cfg(feature = "search")]
            Self::TextSearch { .. } => {}
        }
    }
}

#[cfg(feature = "search-engine")]
impl Kind {
    /// `own_records` is false inside a subquery, which the read's owner does not narrow.
    pub(crate) fn searches<'k>(
        &'k mut self,
        own_records: bool,
        found: &mut Vec<(&'k mut Self, bool)>,
    ) {
        if matches!(self, Self::TextSearch { .. }) {
            found.push((self, own_records));
            return;
        }
        match self {
            Self::InSelect(select) => select.condition.searches(false, found),
            Self::Not(kind) => kind.searches(own_records, found),
            Self::Or(left, right) => {
                left.searches(own_records, found);
                right.searches(own_records, found);
            }
            Self::Compare { .. }
            | Self::Between { .. }
            | Self::In { .. }
            | Self::Null(_)
            | Self::TextSearch { .. } => {}
            #[cfg(feature = "listing")]
            Self::Contains { .. } => {}
        }
    }
}

pub struct Sort<A> {
    pub(crate) key: SortKey,
    marker: PhantomData<fn() -> A>,
}

impl<A> Sort<A> {
    pub(crate) fn new(column: String, descending: bool, nullable: bool) -> Self {
        Self {
            key: SortKey {
                column,
                descending,
                nullable,
            },
            marker: PhantomData,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::tests::{NUMBERED, Note};

    const SUBTITLE: Column<Note, Option<String>> = Column::new("subtitle");

    fn rendered(condition: &Condition<Note>) -> (String, Vec<SqlValue>) {
        let (mut sql, mut params) = (String::new(), Vec::new());
        condition.kind.render_into(&NUMBERED, &mut params, &mut sql);
        (sql, params)
    }

    #[cfg(feature = "search")]
    #[test]
    fn no_words_match_nothing_on_every_database() {
        let blank = Condition::<Note>::new(Kind::TextSearch {
            column: "search".to_owned(),
            words: " ".to_owned(),
            #[cfg(feature = "search-engine")]
            index: ("notes", "note"),
        });

        assert_eq!(rendered(&blank), ("(1 = 0)".to_owned(), Vec::new()));
    }

    #[test]
    fn a_subquery_leaves_its_aggregate_s_hidden_rows_out() {
        let (sql, _) = rendered(&in_select(Note::ID, Note::ID, SUBTITLE.eq(None)));

        assert!(sql.ends_with(" AND (\"deleted_at\" IS NULL)))"), "{sql}");
    }

    #[test]
    fn a_nullable_value_equal_to_none_is_null() {
        assert_eq!(
            rendered(&SUBTITLE.eq(None)),
            ("(\"subtitle\" IS NULL)".to_owned(), Vec::new())
        );
    }

    #[test]
    fn a_nullable_value_unequal_to_a_value_keeps_the_null_rows() {
        assert_eq!(
            rendered(&SUBTITLE.ne(Some("draft".to_owned()))),
            (
                "NOT (\"subtitle\" IS NOT DISTINCT FROM $1)".to_owned(),
                vec![SqlValue::Text("draft".to_owned())]
            )
        );
        assert_eq!(rendered(&SUBTITLE.ne(None)).0, "NOT (\"subtitle\" IS NULL)");
    }

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

        fn decode(row: &mut dyn sofie_core::ColumnReader) -> sofie_core::Result<Self> {
            Ok(Self {
                from: Cast::decode(row)?,
                until: Cast::decode(row)?,
            })
        }
    }

    #[test]
    fn a_value_of_several_columns_holding_a_null_matches_that_column_by_is_null() {
        let period: Column<Note, Option<Period>> = Column::new("period");
        let open = || {
            Some(Period {
                from: None,
                until: 9,
            })
        };

        assert_eq!(
            rendered(&period.eq(open())),
            (
                "(\"period_from\" IS NULL AND \"period_until\" = $1)".to_owned(),
                vec![SqlValue::Int(9)]
            )
        );
        assert_eq!(
            rendered(&period.ne(open())).0,
            "NOT (\"period_from\" IS NULL AND \"period_until\" IS NOT DISTINCT FROM $1)"
        );
        assert_eq!(
            rendered(&period.eq(None)).0,
            "(\"period_from\" IS NULL AND \"period_until\" IS NULL)"
        );
    }

    #[test]
    fn a_listed_value_holding_a_null_matches_as_eq_does() {
        let period: Column<Note, Option<Period>> = Column::new("period");
        let closed = Some(Period {
            from: Some(1),
            until: 2,
        });
        let open = Some(Period {
            from: None,
            until: 9,
        });

        assert_eq!(
            rendered(&period.in_([closed, open, None])),
            (
                "((((\"period_from\", \"period_until\") IN (($1, $2))) \
                 OR (\"period_from\" IS NULL AND \"period_until\" = $3)) \
                 OR (\"period_from\" IS NULL AND \"period_until\" IS NULL))"
                    .to_owned(),
                vec![SqlValue::Int(1), SqlValue::Int(2), SqlValue::Int(9)]
            )
        );
    }

    struct Arrays;

    impl crate::Dialect for Arrays {
        fn push_placeholder(&self, sql: &mut String, index: usize) {
            NUMBERED.push_placeholder(sql, index);
        }

        fn binds_arrays(&self) -> bool {
            true
        }

        fn migrations_setup(&self) -> &'static str {
            unimplemented!()
        }

        fn migrations_lock(&self) -> &'static str {
            unimplemented!()
        }

        fn enter_schema(&self, _schema: &str) -> String {
            unimplemented!()
        }

        fn framework_migrations(&self) -> crate::Migrations {
            unimplemented!()
        }

        fn messages(&self) -> &'static crate::MessageSql {
            NUMBERED.messages()
        }
    }

    #[test]
    fn listed_values_of_several_columns_bind_one_array_per_column_where_arrays_bind() {
        let period: Column<Note, Option<Period>> = Column::new("period");
        let closed = |from| {
            Some(Period {
                from: Some(from),
                until: 9,
            })
        };
        let open = Some(Period {
            from: None,
            until: 9,
        });
        let render = |condition: Condition<Note>| {
            let (mut sql, mut params) = (String::new(), Vec::new());
            condition.kind.render_into(&Arrays, &mut params, &mut sql);
            (sql, params)
        };

        let (one, _) = render(period.in_([closed(1)]));
        let (three, params) = render(period.in_([closed(1), open, closed(2), closed(3)]));

        assert_eq!(
            one,
            "((\"period_from\", \"period_until\") IN \
             (SELECT * FROM unnest($1::bigint[], $2::bigint[])))"
        );
        assert_eq!(
            three,
            format!("({one} OR (\"period_from\" IS NULL AND \"period_until\" = $3))")
        );
        let ints = |values: [i64; 3]| SqlValue::Array(values.map(SqlValue::Int).to_vec());
        assert_eq!(params, [ints([1, 2, 3]), ints([9, 9, 9]), SqlValue::Int(9)]);
    }

    #[test]
    fn a_value_that_cannot_be_null_compares_with_equals() {
        let title: Column<Note, String> = Column::new("title");

        assert_eq!(
            rendered(&title.ne("draft".to_owned())).0,
            "NOT (\"title\" = $1)"
        );
    }

    #[test]
    fn a_nullable_value_compares_by_order_and_leaves_its_null_rows_out() {
        assert_eq!(
            rendered(&SUBTITLE.lt("m".to_owned())),
            (
                "(\"subtitle\" < $1)".to_owned(),
                vec![SqlValue::Text("m".to_owned())]
            )
        );
        assert_eq!(
            rendered(&SUBTITLE.between("a".to_owned(), "m".to_owned())).1,
            [
                SqlValue::Text("a".to_owned()),
                SqlValue::Text("m".to_owned())
            ]
        );
        assert_eq!(rendered(&SUBTITLE.gte(None)).1, [SqlValue::Null]);
    }

    #[test]
    fn like_names_its_escape_so_every_database_reads_a_backslash_alike() {
        let title: Column<Note, String> = Column::new("title");

        assert_eq!(
            rendered(&title.like("50\\%")).0,
            "(\"title\" LIKE $1 ESCAPE '\\')"
        );
    }
}
