use sofie_model::Module;
use sofie_model::naming::QUERY_MODULE;

use super::{Finding, Rule};

/// Raw SQL runs read-only and outside the owner filter, so only queries may use it.
pub(super) fn check_raw_sql(modules: &[Module]) -> Vec<Finding> {
    modules
        .iter()
        .flat_map(|module| &module.code.raw_sql)
        .filter(|raw| raw.module_path.first().map(String::as_str) != Some(QUERY_MODULE))
        .map(|raw| Finding {
            rule: Rule::RawSql,
            file: raw.file.clone(),
            line: raw.line,
            message: format!(
                "raw SQL is for queries only; actions write through aggregates, and this call is \
                 in `crate::{}`",
                raw.module_path.join("::")
            ),
            fix: format!(
                "move the read into a query under `{QUERY_MODULE}`, or use the query builder"
            ),
        })
        .collect()
}
