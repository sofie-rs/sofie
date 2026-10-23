//! For `CONCURRENTLY` index migrations, which cannot run inside a transaction block.

use sofie_core::Result;
use sofie_core::sql_lexer::{Token, lex};
use sofie_db::{BoxFuture, Dialect, Step};

use crate::Postgres;
use crate::transaction::{OpenTransaction, prepare, run_step};

/// A build that fails leaves its index invalid, and `IF NOT EXISTS` in a rerun keeps it so.
const REFUSE_INVALID_INDEXES: &str = "DO $refuse$
DECLARE
    invalid text;
BEGIN
    SELECT string_agg(format('%I.%I', current_schema(), class.relname), ', ') INTO invalid
    FROM pg_index JOIN pg_class class ON class.oid = pg_index.indexrelid
    WHERE NOT pg_index.indisvalid
        AND class.relnamespace = (SELECT oid FROM pg_namespace WHERE nspname = current_schema());
    IF invalid IS NOT NULL THEN
        RAISE EXCEPTION 'index % is invalid, left by a failed concurrent build', invalid
            USING HINT = 'DROP INDEX CONCURRENTLY it, then migrate again to build it anew';
    END IF;
END
$refuse$";

pub(crate) fn migration_alone<'a>(
    postgres: &'a Postgres,
    schema: &str,
    sql: &str,
    record: Step,
) -> std::result::Result<BoxFuture<'a, Result<()>>, Step> {
    if !builds_concurrently(sql) {
        return Err(record);
    }
    let schema = postgres.quote(schema);
    let steps = vec![
        Step::Script(format!(
            "CREATE SCHEMA IF NOT EXISTS {schema}; SET search_path TO {schema}"
        )),
        Step::Script(sql.to_owned()),
        Step::Script(REFUSE_INVALID_INDEXES.to_owned()),
        record,
        Step::Script("RESET search_path".to_owned()),
    ];
    Ok(Box::pin(run_steps_alone(postgres, steps)))
}

async fn run_steps_alone(postgres: &Postgres, steps: Vec<Step>) -> Result<()> {
    let connection = postgres.connection().await?;
    let statements = prepare(&connection, &steps).await?;
    // The steps set session state (a schema); a connection left
    // midway, by a failure or a cancelled caller, is closed with it.
    let mut open = OpenTransaction {
        connection: Some(connection),
    };
    for (step, statement) in steps.iter().zip(&statements) {
        run_step(open.client(), step, statement.as_ref()).await?;
    }
    open.connection = None;
    Ok(())
}

/// `CREATE [UNIQUE] INDEX CONCURRENTLY`, `DROP INDEX CONCURRENTLY` and `REINDEX … CONCURRENTLY`
/// refuse a transaction block; the word anywhere else, as in `REFRESH … CONCURRENTLY`, does not.
fn builds_concurrently(sql: &str) -> bool {
    let lexed = lex(sql);
    lexed
        .tokens
        .split(|token| *token == Token::Symbol(';'))
        .any(|statement| {
            let words: Vec<&str> = statement.iter().map(Token::keyword).collect();
            match words.as_slice() {
                ["create" | "drop", "index", "concurrently", ..]
                | ["create", "unique", "index", "concurrently", ..] => true,
                ["reindex", rest @ ..] => rest.contains(&"concurrently"),
                _ => false,
            }
        })
}

#[cfg(test)]
mod tests {
    use super::builds_concurrently;

    #[test]
    fn only_a_concurrent_index_build_runs_alone() {
        assert!(builds_concurrently("CREATE INDEX CONCURRENTLY a ON t (b);"));
        assert!(builds_concurrently("drop index concurrently a"));
        assert!(!builds_concurrently(
            "-- not concurrently\nCREATE INDEX a ON t (b);"
        ));
    }

    #[test]
    fn a_concurrent_build_among_other_words_is_found_by_its_statement() {
        assert!(builds_concurrently(
            "/* built online */ CREATE UNIQUE INDEX CONCURRENTLY a ON t (b)"
        ));
        assert!(builds_concurrently("DROP INDEX CONCURRENTLY IF EXISTS a;"));
        assert!(builds_concurrently("REINDEX INDEX CONCURRENTLY a"));
        assert!(builds_concurrently(
            "COMMENT ON TABLE t IS 'x;'; CREATE INDEX CONCURRENTLY a ON t (b)"
        ));
    }

    #[test]
    fn concurrently_outside_an_index_build_keeps_the_transaction() {
        for sql in [
            "/* concurrently */ CREATE INDEX a ON t (b);",
            "/* outer /* concurrently */ still */ CREATE INDEX a ON t (b);",
            "COMMENT ON TABLE t IS 'rebuilt concurrently';",
            "COMMENT ON TABLE t IS E'it\\'s concurrently';",
            "REFRESH MATERIALIZED VIEW CONCURRENTLY totals;",
            "ALTER TABLE t ADD COLUMN \"concurrently\" text;",
            "DO $body$ BEGIN EXECUTE 'CREATE INDEX CONCURRENTLY a ON t (b)'; END $body$;",
            "CREATE INDEX a ON t (concurrently);",
        ] {
            assert!(!builds_concurrently(sql), "{sql}");
        }
    }
}
