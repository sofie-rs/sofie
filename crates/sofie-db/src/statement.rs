use std::any::{Any, TypeId};
use std::sync::Arc;

use sofie_core::{Cast, Error, Result, SqlType, SqlValue, Uuid};

use crate::aggregate::table;
use crate::condition::Kind;
use crate::selection::Selection;
use crate::type_cache::TypeCache;
use crate::{Aggregate, Dialect, Read, Selected, Source};

const COUNT: &[Selected] = &[Selected(Selection::Expression("COUNT(*)"))];
pub(crate) const ONE: &[Selected] = &[Selected(Selection::Expression("1"))];

#[derive(Clone, Default)]
pub(crate) struct Criteria {
    pub(crate) conditions: Vec<Kind>,
    pub(crate) sorts: Vec<SortKey>,
    pub(crate) limit: Option<u64>,
    pub(crate) offset: Option<u64>,
    pub(crate) with_hidden: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct SortKey {
    pub(crate) column: String,
    pub(crate) descending: bool,
    pub(crate) nullable: bool,
}

impl SortKey {
    pub(crate) fn id(descending: bool) -> Self {
        Self {
            column: "id".to_owned(),
            descending,
            nullable: false,
        }
    }
}

type SelectLists = TypeCache<(TypeId, TypeId), Option<Arc<str>>>;

static SELECT_LISTS: SelectLists = TypeCache::new();

/// `None` when a joined column binds the reader's owner, so it renders each time.
fn cached_select_list<T: Source>(lists: &SelectLists, dialect: &dyn Dialect) -> Option<Arc<str>> {
    let key = (TypeId::of::<T>(), (dialect as &dyn Any).type_id());
    lists.get_or_insert_with(key, || {
        let columns = T::columns();
        let fixed = columns.iter().all(Selected::is_fixed);
        fixed.then(|| select_list::<T::Base>(dialect, None, &columns, &mut Vec::new()).into())
    })
}

impl Criteria {
    pub(crate) async fn all<T: Source>(self, read: &Read) -> Result<Vec<T>> {
        let criteria = self.matched(read).await?;
        let (dialect, owner) = (read.dialect(), read.owner());
        let (sql, params) = match cached_select_list::<T>(&SELECT_LISTS, dialect) {
            Some(list) => {
                criteria.filtered::<T::Base>(String::from(&*list), Vec::new(), dialect, owner)
            }
            None => criteria.select_sql::<T::Base>(dialect, owner, &T::columns()),
        };
        let expected_rows = criteria
            .limit
            .map_or(0, |limit| usize::try_from(limit).unwrap_or(usize::MAX));
        read.decoded(&sql, &params, expected_rows, T::column_types(), T::decode)
            .await
    }

    #[cfg_attr(
        not(feature = "search-engine"),
        expect(unused_variables, reason = "only an engine reads it")
    )]
    pub(crate) fn matched(self, read: &Read) -> impl Future<Output = Result<Self>> + Send + '_ {
        #[cfg(feature = "search-engine")]
        return crate::search::matched(self, read);
        #[cfg(not(feature = "search-engine"))]
        std::future::ready(Ok(self))
    }

    /// A count or an existence check covers every match, whatever page the query asked for.
    fn unpaged(mut self) -> Self {
        self.sorts.clear();
        self.limit = None;
        self.offset = None;
        self
    }

    pub(crate) async fn count<A: Aggregate>(self, read: &Read) -> Result<u64> {
        let criteria = self.unpaged().matched(read).await?;
        let (sql, params) = criteria.select_sql::<A>(read.dialect(), read.owner(), COUNT);
        let counts = read
            .decoded(&sql, &params, 1, vec![Some(SqlType::Int)], i64::decode)
            .await?;
        let count = counts
            .first()
            .copied()
            .ok_or_else(|| Error::internal_message("a count answered no row"))?;
        u64::try_from(count).map_err(|error| Error::internal("negative row count", error))
    }

    pub(crate) async fn exists<A: Aggregate>(self, read: &Read) -> Result<bool> {
        let criteria = Self {
            limit: Some(1),
            ..self.unpaged()
        }
        .matched(read)
        .await?;
        let (sql, params) = criteria.select_sql::<A>(read.dialect(), read.owner(), ONE);
        let rows = read
            .decoded(&sql, &params, 0, Vec::new(), |_| Ok(()))
            .await?;
        Ok(!rows.is_empty())
    }

    #[cfg(feature = "live")]
    fn heard(&self, module: &'static str, name: &'static str) {
        if !crate::live::tracking() {
            return;
        }
        let mut tables = vec![(module, name)];
        for condition in &self.conditions {
            condition.tables(&mut tables);
        }
        crate::live::read_tables(tables);
    }

    /// In parentheses, so the owner's condition cannot be escaped by an `or` among the others.
    pub(crate) fn select_sql<A: Aggregate>(
        &self,
        dialect: &dyn Dialect,
        owner: Option<Uuid>,
        selection: &[Selected],
    ) -> (String, Vec<SqlValue>) {
        #[cfg(feature = "live")]
        crate::live::read_tables(selection.iter().filter_map(Selected::joined_table));
        let mut params = Vec::new();
        let sql = select_list::<A>(dialect, owner, selection, &mut params);
        self.filtered::<A>(sql, params, dialect, owner)
    }

    fn filtered<A: Aggregate>(
        &self,
        mut sql: String,
        mut params: Vec<SqlValue>,
        dialect: &dyn Dialect,
        owner: Option<Uuid>,
    ) -> (String, Vec<SqlValue>) {
        #[cfg(feature = "live")]
        self.heard(A::MODULE, A::NAME);
        let implicit = self.implicit::<A>(owner);
        for (index, condition) in implicit.iter().chain(&self.conditions).enumerate() {
            sql.push_str(if index == 0 { " WHERE " } else { " AND " });
            condition.render_into(dialect, &mut params, &mut sql);
        }
        for (index, sort) in self.sorts.iter().enumerate() {
            sql.push_str(if index == 0 { " ORDER BY " } else { ", " });
            dialect.push_quoted(&mut sql, &sort.column);
            sql.push_str(if sort.descending { " DESC" } else { " ASC" });
            if sort.nullable {
                sql.push_str(dialect.empty_order(sort.descending));
            }
        }
        // Bound, not written: each distinct SQL text stays prepared on every pooled connection.
        for (keyword, count) in [(" LIMIT ", self.limit), (" OFFSET ", self.offset)] {
            if let Some(count) = count {
                params.push(SqlValue::Int(i64::try_from(count).unwrap_or(i64::MAX)));
                sql.push_str(keyword);
                dialect.push_placeholder(&mut sql, params.len());
            }
        }
        (sql, params)
    }

    /// Skipped when the query already states the same condition.
    fn implicit<A: Aggregate>(&self, owner: Option<Uuid>) -> Vec<Kind> {
        let hidden = table::<A>()
            .hiding()
            .filter(|_| !self.with_hidden)
            .map(|column| Kind::Null(vec![column.to_owned()]));
        owner
            .and_then(A::owned_by)
            .map(|condition| condition.kind)
            .into_iter()
            .chain(hidden)
            .filter(|condition| !self.conditions.contains(condition))
            .collect()
    }
}

fn select_list<A: Aggregate>(
    dialect: &dyn Dialect,
    owner: Option<Uuid>,
    selection: &[Selected],
    params: &mut Vec<SqlValue>,
) -> String {
    let base = table::<A>();
    let selection: Vec<String> = selection
        .iter()
        .map(|selected| selected.render(base, dialect, owner, params))
        .collect();
    format!(
        "SELECT {} FROM {}",
        selection.join(", "),
        base.qualified(dialect)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::tests::{NUMBERED, Note};
    use crate::{Column, Id, Select, Source};

    const TITLE: Column<Note, String> = Column::new("title");

    fn sql(query: &crate::Query<Note>, owner: Option<Uuid>) -> (String, Vec<SqlValue>) {
        query.criteria.select_sql::<Note>(&NUMBERED, owner, ONE)
    }

    #[test]
    fn finding_its_own_record_in_its_owners_area_filters_by_the_id_once() {
        let owner = Uuid::from_u128(7);

        let (text, params) = sql(
            &Note::query().where_(Note::ID.eq(Id::from_uuid(owner))),
            Some(owner),
        );

        assert_eq!(
            text,
            "SELECT 1 FROM \"notes\".\"note\" WHERE (\"deleted_at\" IS NULL) AND (\"id\" = $1)"
        );
        assert_eq!(params, [SqlValue::Uuid(owner)]);
    }

    #[test]
    fn another_record_is_still_kept_to_the_owner() {
        let (text, _) = sql(
            &Note::query().where_(Note::ID.eq(Id::from_uuid(Uuid::from_u128(8)))),
            Some(Uuid::from_u128(7)),
        );

        assert!(
            text.ends_with("WHERE (\"id\" = $1) AND (\"deleted_at\" IS NULL) AND (\"id\" = $2)"),
            "{text}"
        );
    }

    #[test]
    fn renders_between_in_and_like() {
        let query = Note::query()
            .where_(TITLE.between("a".to_owned(), "m".to_owned()))
            .where_(TITLE.in_(["x".to_owned(), "y".to_owned()]))
            .where_(TITLE.in_([]))
            .where_(TITLE.like("Ada%"))
            .with_trashed();

        let (text, params) = sql(&query, None);

        assert!(
            text.ends_with(
                "WHERE (\"title\" BETWEEN $1 AND $2) AND (\"title\" IN ($3, $4)) \
                 AND (1 = 0) AND (\"title\" LIKE $5 ESCAPE '\\')"
            ),
            "{text}"
        );
        assert_eq!(params.len(), 5);
    }

    #[test]
    fn a_source_s_select_list_is_rendered_once_and_matches_a_fresh_one() {
        let lists = SelectLists::new();

        let first = cached_select_list::<Note>(&lists, &NUMBERED).unwrap();
        let again = cached_select_list::<Note>(&lists, &NUMBERED).unwrap();

        assert!(Arc::ptr_eq(&first, &again));
        let fresh = select_list::<Note>(&NUMBERED, None, &Note::columns(), &mut Vec::new());
        assert_eq!(*first, fresh);
        assert!(first.starts_with("SELECT \"id\", "), "{first}");
    }

    #[test]
    fn every_page_shares_one_statement_text() {
        let page = |offset| {
            let mut query = Note::query().limit(20).with_trashed();
            query.criteria.offset = Some(offset);
            sql(&query, None)
        };

        let ((first, first_params), (later, later_params)) = (page(0), page(4_000));

        assert_eq!(first, later);
        assert!(first.ends_with(" LIMIT $1 OFFSET $2"), "{first}");
        assert_eq!(first_params, [SqlValue::Int(20), SqlValue::Int(0)]);
        assert_eq!(later_params, [SqlValue::Int(20), SqlValue::Int(4_000)]);
    }

    #[test]
    fn a_count_ignores_the_page_the_query_asked_for() {
        let mut query = Note::query().limit(5).latest().with_trashed();
        query.criteria.offset = Some(20);

        let (text, params) = query
            .criteria
            .unpaged()
            .select_sql::<Note>(&NUMBERED, None, COUNT);

        assert_eq!(text, "SELECT COUNT(*) FROM \"notes\".\"note\"");
        assert_eq!(params, []);
    }

    #[test]
    fn a_multi_column_value_is_matched_as_a_row() {
        let (mut sql, mut params) = (String::new(), Vec::new());
        let kind = Kind::In {
            columns: vec!["total".to_owned(), "total_currency".to_owned()],
            types: vec![SqlType::Int, SqlType::Text],
            rows: vec![vec![SqlValue::Int(1), SqlValue::Text("EUR".to_owned())]],
        };

        kind.render_into(&NUMBERED, &mut params, &mut sql);

        assert_eq!(sql, "((\"total\", \"total_currency\") IN (($1, $2)))");
    }
}
