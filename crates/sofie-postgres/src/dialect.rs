use std::fmt::Write as _;

use sofie_db::{Dialect, MessageSql, Migrations};

use crate::Postgres;
use crate::messages::{MESSAGES, MIGRATIONS};

const MIGRATIONS_SETUP: &str = "CREATE SCHEMA IF NOT EXISTS sofie;
CREATE TABLE IF NOT EXISTS sofie.migrations (
    module text NOT NULL,
    name text NOT NULL,
    checksum text NOT NULL,
    applied_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (module, name)
)";

// Any fixed key works, but it must never change: processes running different
// versions of the app have to contend for the same lock.
pub(crate) const MIGRATIONS_LOCK: &str = "SELECT pg_advisory_xact_lock(7046573208157101413)";

impl Dialect for Postgres {
    fn push_placeholder(&self, sql: &mut String, index: usize) {
        let _ = write!(sql, "${index}");
    }

    fn binds_arrays(&self) -> bool {
        true
    }

    /// Hashed by the server, so every instance of the app maps a key to the same lock.
    fn key_lock(&self) -> Option<&'static str> {
        Some("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
    }

    fn shared_key_lock(&self) -> Option<&'static str> {
        Some("SELECT pg_advisory_xact_lock_shared(hashtextextended($1, 0))")
    }

    #[cfg(feature = "module-roles")]
    /// Entered only where the role migration `sofie gen` writes gave the role its schema in
    /// this database: roles belong to the whole server, so another database may have made it.
    fn enter_role(&self, module: Option<&str>) -> Option<String> {
        Some(match module {
            Some(module) => {
                let name = module.replace('\'', "''");
                format!(
                    "SELECT set_config('role', '{name}', true) FROM pg_namespace \
                     WHERE nspname = '{name}' AND pg_get_userbyid(nspowner) = '{name}'"
                )
            }
            None => "RESET ROLE".to_owned(),
        })
    }

    fn nested_rows(&self, columns: &str, from: &str) -> String {
        format!(
            "(SELECT COALESCE(json_agg(json_build_array({columns}) ORDER BY \"id\"), '[]') FROM {from})"
        )
    }

    fn migrations_setup(&self) -> &'static str {
        MIGRATIONS_SETUP
    }

    fn migrations_lock(&self) -> &'static str {
        MIGRATIONS_LOCK
    }

    fn enter_schema(&self, schema: &str) -> String {
        let schema = self.quote(schema);
        format!("CREATE SCHEMA IF NOT EXISTS {schema}; SET LOCAL search_path TO {schema}")
    }

    fn framework_migrations(&self) -> Migrations {
        MIGRATIONS
    }

    fn messages(&self) -> &'static MessageSql {
        &MESSAGES
    }

    #[cfg(feature = "shield")]
    fn shield(&self) -> Option<&'static sofie_db::ShieldSql> {
        Some(&crate::shield::SHIELD)
    }

    #[cfg(feature = "remote-contracts")]
    fn feed(&self) -> Option<&'static sofie_db::FeedSql> {
        Some(&crate::feed::FEED)
    }

    /// Matched by the same expression the search migration indexes.
    #[cfg(feature = "search")]
    fn text_search(&self, column: &str, words: &str) -> String {
        format!("(to_tsvector('simple', {column}) @@ websearch_to_tsquery('simple', {words}))")
    }

    #[cfg(feature = "product-reports")]
    fn product_events(&self) -> Option<&'static sofie_db::ProductSql> {
        Some(&crate::product_events::PRODUCT_EVENTS)
    }

    #[cfg(feature = "live")]
    fn live_notify(&self) -> Option<&'static str> {
        Some("SELECT 1 FROM pg_notify('sofie_live', $1)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn postgres() -> Postgres {
        Postgres::connect("postgres://nobody@localhost/nothing").unwrap()
    }

    #[test]
    fn identifiers_are_quoted_with_embedded_quotes_doubled() {
        assert_eq!(postgres().quote("order"), "\"order\"");
        assert_eq!(postgres().quote("a\"b"), "\"a\"\"b\"");
    }

    #[test]
    fn names_and_placeholders_are_written_into_the_statement_s_own_text() {
        let mut sql = "SELECT ".to_owned();

        postgres().push_quoted(&mut sql, "a\"b");
        sql.push_str(" = ");
        postgres().push_placeholder(&mut sql, 3);

        assert_eq!(sql, "SELECT \"a\"\"b\" = $3");
    }

    #[test]
    fn placeholders_are_numbered_from_one() {
        assert_eq!(postgres().placeholder(1), "$1");
        assert_eq!(postgres().placeholder(12), "$12");
    }
}
