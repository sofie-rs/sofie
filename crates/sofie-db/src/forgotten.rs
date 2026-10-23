use sofie_core::{Result, SqlValue, Uuid};

use crate::{Plan, Read, Step, query_values};

const SCHEMA: &str = "sofie";
const TABLE: &str = "forgotten_principals";

/// What the framework itself keeps of a principal, such as product events, erased in the
/// plan's commit. The principal stays recorded for [`forgotten`] and [`unless_forgotten`]; a
/// module erasing rows of its own plans this first, so no write racing it outlives them.
pub fn forget_principal(plan: &mut Plan<'_>, principal: Uuid) {
    remember(plan, principal);
    #[cfg(feature = "product-reports")]
    if let Some(sql) = plan.dialect().product_events() {
        let params = vec![SqlValue::Uuid(principal)];
        plan.push(Step::statement(sql.forget, params, None));
    }
}

pub async fn forgotten(read: &Read, principal: Uuid) -> Result<bool> {
    let sql = select(read.dialect());
    let rows = query_values(read.db.driver(), &sql, &[SqlValue::Uuid(principal)]).await?;
    Ok(!rows.is_empty())
}

/// A commit writing for a principal read as not forgotten fails as a conflict if an erasure
/// committed since; the retry reads it forgotten.
pub fn unless_forgotten(plan: &mut Plan<'_>, principal: Uuid) {
    act_as_framework(plan);
    lock(plan, plan.dialect().shared_key_lock(), principal);
    let sql = select(plan.dialect());
    plan.push(Step::statement(
        sql,
        vec![SqlValue::Uuid(principal)],
        Some(0),
    ));
}

/// Locked before the erasure's deletes, so a write holding the shared lock is committed and
/// seen by them, and a later one waits and reads the record.
fn remember(plan: &mut Plan<'_>, principal: Uuid) {
    act_as_framework(plan);
    lock(plan, plan.dialect().key_lock(), principal);
    let dialect = plan.dialect();
    let sql = format!(
        "INSERT INTO {} (principal) VALUES ({}) ON CONFLICT DO NOTHING",
        dialect.qualified(SCHEMA, TABLE),
        dialect.placeholder(1),
    );
    plan.push(Step::statement(sql, vec![SqlValue::Uuid(principal)], None));
}

fn select(dialect: &dyn crate::Dialect) -> String {
    format!(
        "SELECT principal FROM {} WHERE principal = {}",
        dialect.qualified(SCHEMA, TABLE),
        dialect.placeholder(1),
    )
}

fn lock(plan: &mut Plan<'_>, lock: Option<&'static str>, principal: Uuid) {
    if let Some(lock) = lock {
        let key = SqlValue::Text(format!("{SCHEMA}.{TABLE}:{principal}"));
        plan.push(Step::statement(lock, vec![key], None));
    }
}

#[cfg_attr(not(feature = "module-roles"), expect(unused_variables))]
fn act_as_framework(plan: &mut Plan<'_>) {
    #[cfg(feature = "module-roles")]
    plan.act_as(crate::outcome::FRAMEWORK_MODULE);
}
