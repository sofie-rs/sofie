use std::fmt::Write as _;

use rusqlite::functions::FunctionFlags;

use sofie_db::{Dialect, Engine, MessageSql, Migrations};

use crate::Sqlite;
use crate::messages::{MESSAGES, MIGRATIONS};

const MIGRATIONS_SETUP: &str = "CREATE TABLE IF NOT EXISTS sofie_migrations (
    module text NOT NULL,
    name text NOT NULL,
    checksum text NOT NULL,
    applied_at timestamptz NOT NULL DEFAULT (CAST(unixepoch('subsec') * 1000000 AS INTEGER)),
    PRIMARY KEY (module, name)
)";

const DROP_EVERY_TABLE: &str = r#"SELECT statement FROM (
    SELECT 0 AS step, 'PRAGMA defer_foreign_keys = ON' AS statement
    UNION ALL
    SELECT 1, 'DROP TABLE "' || replace(name, '"', '""') || '"' FROM sqlite_master
    WHERE type = 'table' AND name NOT LIKE 'sqlite\_%' ESCAPE '\'
) ORDER BY step"#;

/// `lower` of every letter, where SQLite's own lowers only ASCII ones.
const LOWER: &str = "sofie_lower";

pub(crate) fn add_lower(connection: &rusqlite::Connection) -> rusqlite::Result<()> {
    let flags = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
    connection.create_scalar_function(LOWER, 1, flags, |context| {
        let text: Option<String> = context.get(0)?;
        Ok(text.map(|text| text.to_lowercase()))
    })
}

impl Dialect for Sqlite {
    fn push_placeholder(&self, sql: &mut String, index: usize) {
        let _ = write!(sql, "?{index}");
    }

    fn engine(&self) -> Engine {
        Engine::Sqlite
    }

    fn lowercase(&self, expression: &str) -> String {
        format!("{LOWER}({expression})")
    }

    /// SQLite sorts empty values first ascending.
    fn empty_order(&self, descending: bool) -> &'static str {
        if descending {
            " NULLS FIRST"
        } else {
            " NULLS LAST"
        }
    }

    fn qualified(&self, schema: &str, name: &str) -> String {
        self.quote(&format!("{schema}_{name}"))
    }

    fn nested_rows(&self, columns: &str, from: &str) -> String {
        format!(
            "(SELECT COALESCE(json_group_array(json_array({columns})), '[]') \
             FROM (SELECT * FROM {from} ORDER BY \"id\"))"
        )
    }

    /// SQLite's JSON cannot hold a blob.
    fn bytes_in_json(&self, column: &str) -> String {
        format!("'\\x' || lower(hex({column}))")
    }

    fn migrations_setup(&self) -> &'static str {
        MIGRATIONS_SETUP
    }

    /// The transaction it runs in already holds the database's one write lock.
    fn migrations_lock(&self) -> &'static str {
        "SELECT 1"
    }

    fn enter_schema(&self, _schema: &str) -> String {
        String::new()
    }

    /// Deferred, so a table another references drops in any order.
    fn drop_every_table(&self) -> Option<&'static str> {
        Some(DROP_EVERY_TABLE)
    }

    fn framework_migrations(&self) -> Migrations {
        MIGRATIONS
    }

    fn messages(&self) -> &'static MessageSql {
        &MESSAGES
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_module_s_table_is_named_after_the_module() {
        let sqlite = Sqlite::memory().unwrap();

        assert_eq!(sqlite.qualified("order", "order"), "\"order_order\"");
        assert_eq!(sqlite.quote("a\"b"), "\"a\"\"b\"");
        assert_eq!(sqlite.placeholder(12), "?12");
    }
}
