use sofie_core::{Error, Result, SqlType, SqlValue, Uuid};

use crate::aggregate::{table, uuid_equals};
use crate::condition::Kind;
use crate::{
    Aggregate, Column, Condition, Context, Dialect, ExpectedRows, Id, Query, Read, Source, Step,
};

#[must_use]
pub fn owned_through<A: Aggregate, P: Aggregate>(
    parent: Column<A, Id<P>>,
    owner: Uuid,
) -> Option<Condition<A>> {
    let owned_parents = P::owned_by(owner)?;
    Some(crate::in_select(parent, P::ID, owned_parents))
}

pub(crate) fn ownership_check<A: Aggregate>(id: Uuid, owner: Uuid, dialect: &dyn Dialect) -> Step {
    let table = table::<A>();
    let mut params = Vec::new();
    let mut sql = format!("SELECT 1 FROM {} WHERE ", table.qualified(dialect));
    match A::owned_by(owner) {
        Some(condition) => condition.kind.render_into(dialect, &mut params, &mut sql),
        None => sql.push_str("(1 = 0)"),
    }
    sql.push_str(" AND ");
    sql.push_str(&crate::sql::equals(
        dialect,
        "id",
        SqlValue::Uuid(id),
        &mut params,
    ));
    Step::Statement {
        sql,
        params,
        expect_rows: Some(ExpectedRows::or_not_found(1)),
    }
}

/// A child reached under another parent, as in `/orders/{order}/lines/{line}`, is not found.
pub(crate) fn within_parents<S: Source, C: Context>(mut query: Query<S>, context: &C) -> Query<S> {
    for field in S::Base::FIELDS {
        let [column] = field.columns else { continue };
        if column.ty != SqlType::Uuid || field.name == S::Base::NAME {
            continue;
        }
        if let Ok(parent) = context.bound(field.name) {
            let column = crate::sql::column_name(field.name, column);
            query = query.where_(Condition::new(uuid_equals(&column, parent)));
        }
    }
    query
}

/// A list under a parent the read cannot reach, as in `/wish-lists/{wish_list}/notes` with
/// another principal's list, is not found: an empty list would say the parent exists.
pub async fn refuse_parent_beyond_reach<A: Aggregate>(
    read: &Read,
    field: &str,
    parent: Uuid,
) -> Result<()> {
    let Some(owner) = read.owner() else {
        return Ok(());
    };
    if A::OWNER != Some(field) {
        return Ok(());
    }
    let Some(Kind::InSelect(owned_parents)) = A::owned_by(owner).map(|owned| owned.kind) else {
        return Ok(());
    };
    let mut params = Vec::new();
    let mut sql = "SELECT 1 WHERE ".to_owned();
    owned_parents.render_holding(
        &SqlValue::Uuid(parent),
        read.dialect(),
        &mut params,
        &mut sql,
    );
    let found = read
        .decoded(&sql, &params, 1, vec![None], |_| Ok(()))
        .await?;
    if found.is_empty() {
        return Err(Error::NotFound);
    }
    Ok(())
}
