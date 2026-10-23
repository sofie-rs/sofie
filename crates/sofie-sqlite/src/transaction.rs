use rusqlite::{Connection, Statement, Transaction, TransactionBehavior};
use sofie_core::{Error, Result};
use sofie_db::Step;

use crate::rows::{bind, prepared};
use crate::{statement_error, traced};

pub(crate) fn run(connection: &Connection, steps: &[Step]) -> Result<()> {
    immediately(connection, |transaction| {
        steps
            .iter()
            .try_for_each(|step| run_step(transaction, step))
    })
}

/// `IMMEDIATE` takes the write lock at once, so two writers never deadlock upgrading a read.
pub(crate) fn immediately<T>(
    connection: &Connection,
    work: impl FnOnce(&Connection) -> Result<T>,
) -> Result<T> {
    let transaction = Transaction::new_unchecked(connection, TransactionBehavior::Immediate)
        .map_err(|error| Error::internal("starting a transaction", error))?;
    let done = work(&transaction)?;
    transaction
        .commit()
        .map_err(|error| Error::internal("committing a transaction", error))?;
    Ok(done)
}

fn run_step(connection: &Connection, step: &Step) -> Result<()> {
    match step {
        Step::Script(sql) => traced(sql, || {
            connection
                .execute_batch(sql)
                .map_err(|error| statement_error(connection, sql, error))?;
            Ok(0)
        })
        .map(drop),
        Step::Statement {
            sql,
            params,
            expect_rows,
        } => {
            let affected = traced(sql, || {
                let mut statement = prepared(connection, sql)?;
                bind(&mut statement, params)?;
                counted(connection, sql, &mut statement)
            })?;
            expect_rows
                .as_ref()
                .map_or(Ok(()), |expected| expected.check(affected))
        }
    }
}

/// A statement that returns rows counts them: `changes()` would still hold the previous write's.
fn counted(connection: &Connection, sql: &str, statement: &mut Statement<'_>) -> Result<u64> {
    let failed = |error| statement_error(connection, sql, error);
    if statement.column_count() == 0 {
        let affected = statement.raw_execute().map_err(failed)?;
        return Ok(u64::try_from(affected).unwrap_or(u64::MAX));
    }
    let mut rows = statement.raw_query();
    let mut count = 0;
    while rows.next().map_err(failed)?.is_some() {
        count += 1;
    }
    Ok(count)
}
