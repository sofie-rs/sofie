//! A subquery in the select list, so conditions and orderings on the base stay unambiguous.

use std::fmt::Write as _;

use sofie_core::{Cast, SqlValue, Uuid};

use crate::aggregate::Table;
use crate::condition::Kind;
use crate::selection::Selection;
use crate::{Aggregate, Column, Dialect, Id, Selected};

/// So a join to the base's own table still reads the base's row outside the subquery.
const ALIAS: &str = "joined";

#[derive(Clone, Debug)]
pub(crate) struct Join {
    via: &'static str,
    table: (&'static str, &'static str),
    column: String,
    hidden: Vec<&'static str>,
    owned_by: fn(Uuid) -> Option<Kind>,
}

/// Null where there is none, where it is hidden, or, in an owned area, another principal's.
#[must_use]
pub fn joined<B: Aggregate, J: Aggregate, T: Cast>(
    via: Column<B, Id<J>>,
    field: Column<J, T>,
) -> Vec<Selected> {
    T::COLUMNS
        .iter()
        .map(|column| {
            Selected(Selection::Joined(Join {
                via: via.name(),
                table: (J::MODULE, J::NAME),
                column: crate::sql::column_name(field.name(), column),
                hidden: crate::aggregate::table::<J>().hiding().collect(),
                owned_by: |owner| J::owned_by(owner).map(|condition| condition.kind),
            }))
        })
        .collect()
}

/// The join reads null for a missing, hidden or other principal's record, so a joined field is
/// an `Option`: of the column's type, or the column's own `Option`.
#[diagnostic::on_unimplemented(
    message = "a joined field must be an `Option` of the joined column's type",
    note = "the joined record may be missing, hidden or another principal's"
)]
pub trait JoinedField<T> {}

impl<T: Cast> JoinedField<T> for Option<T> {}

impl<T: Cast> JoinedField<Option<T>> for Option<T> {}

pub const fn joined_field<J, T, F: JoinedField<T>>(_field: Column<J, T>) {}

impl Join {
    #[cfg(feature = "live")]
    pub(crate) fn table(&self) -> (&'static str, &'static str) {
        self.table
    }

    pub(crate) fn render(
        &self,
        base: &Table,
        dialect: &dyn Dialect,
        owner: Option<Uuid>,
        params: &mut Vec<SqlValue>,
    ) -> String {
        let alias = dialect.quote(ALIAS);
        let (schema, name) = self.table;
        let mut sql = format!(
            "(SELECT {alias}.{} FROM {} AS {alias} WHERE {alias}.{} = {}.{}",
            dialect.quote(&self.column),
            dialect.qualified(schema, name),
            dialect.quote("id"),
            base.qualified(dialect),
            dialect.quote(self.via),
        );
        for column in &self.hidden {
            let _ = write!(sql, " AND {alias}.{} IS NULL", dialect.quote(column));
        }
        if let Some(condition) = owner.and_then(self.owned_by) {
            sql.push_str(" AND ");
            condition.render_into(dialect, params, &mut sql);
        }
        sql.push(')');
        sql
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::table;
    use crate::aggregate::tests::{NUMBERED, Note};

    #[test]
    fn a_joined_field_is_an_option_of_the_column_or_the_column_s_own_option() {
        let title: Column<Note, String> = Column::new("title");
        let subtitle: Column<Note, Option<String>> = Column::new("subtitle");

        joined_field::<_, _, Option<String>>(title);
        joined_field::<_, _, Option<String>>(subtitle);
    }

    #[test]
    fn a_joined_record_its_aggregate_hides_reads_as_none() {
        let via: Column<Note, Id<Note>> = Column::new("parent");
        let title: Column<Note, String> = Column::new("title");
        let Some(Selected(Selection::Joined(join))) = joined(via, title).pop() else {
            panic!("a joined column is a join");
        };

        let sql = join.render(table::<Note>(), &NUMBERED, None, &mut Vec::new());

        assert!(
            sql.ends_with(" AND \"joined\".\"deleted_at\" IS NULL)"),
            "{sql}"
        );
    }
}
