//! `COMMIT` joins the pipeline unless a step's row count must be checked first.

use std::sync::atomic::{AtomicBool, Ordering};

use deadpool_postgres::Object;
use sofie_core::{Error, Result};
use sofie_db::{BoxFuture, Step, join_all, traced_statement};
use tokio_postgres::{Client, Statement};

use crate::Postgres;
use crate::error::{classified, statement_error};
use crate::value::Encoded;

type Request<'a> = BoxFuture<'a, Result<()>>;

impl Postgres {
    pub(crate) async fn run_in_transaction(&self, steps: Vec<Step>) -> Result<()> {
        let connection = self.connection().await?;
        // Preparing first keeps the pipeline in step order.
        let statements = prepare(&connection, &steps).await?;
        let open = OpenTransaction {
            connection: Some(connection),
        };
        if steps.iter().any(counts_rows) {
            let outcome = run_steps(open.client(), &steps, &statements).await;
            return open.end(outcome).await;
        }
        open.run_and_end(&steps, &statements).await
    }
}

fn counts_rows(step: &Step) -> bool {
    matches!(
        step,
        Step::Statement {
            expect_rows: Some(_),
            ..
        }
    )
}

pub(crate) async fn prepare(connection: &Object, steps: &[Step]) -> Result<Vec<Option<Statement>>> {
    let preparing = steps
        .iter()
        .map(|step| -> BoxFuture<'_, Result<Option<Statement>>> {
            Box::pin(async move {
                let Step::Statement { sql, .. } = step else {
                    return Ok(None);
                };
                crate::prepared(connection, sql).await.map(Some)
            })
        })
        .collect();
    join_all(preparing).await.into_iter().collect()
}

/// Fails with the first failed step; later ones fail only because the transaction aborted.
async fn run_steps(
    client: &Client,
    steps: &[Step],
    statements: &[Option<Statement>],
) -> Result<()> {
    let refused = AtomicBool::new(false);
    let requests = requests(client, steps, statements, &refused).collect();
    join_all(requests).await.into_iter().collect()
}

/// A step refused before its request left has recorded it by the time the next is first polled.
fn requests<'a>(
    client: &'a Client,
    steps: &'a [Step],
    statements: &'a [Option<Statement>],
    refused: &'a AtomicBool,
) -> impl Iterator<Item = Request<'a>> {
    let begin = simple(client, "BEGIN", "starting a transaction");
    let steps = steps.iter().zip(statements).map(move |(step, statement)| {
        Box::pin(async move {
            let outcome = run_step(client, step, statement.as_ref()).await;
            if outcome.is_err() {
                refused.store(true, Ordering::Relaxed);
            }
            outcome
        }) as Request<'a>
    });
    std::iter::once(begin).chain(steps)
}

pub(crate) async fn run_step(
    client: &Client,
    step: &Step,
    statement: Option<&Statement>,
) -> Result<()> {
    match (step, statement) {
        (Step::Script(sql), _) => {
            let script = async {
                client
                    .batch_execute(sql)
                    .await
                    .map_err(|error| statement_error(sql, error))
            };
            traced_statement(sql, script, |()| None).await
        }
        (
            Step::Statement {
                sql,
                params,
                expect_rows,
            },
            Some(statement),
        ) => {
            let execution = async {
                client
                    .execute_raw(statement, params.iter().map(Encoded))
                    .await
                    .map_err(|error| statement_error(sql, error))
            };
            let affected = traced_statement(sql, execution, |affected| Some(*affected)).await?;
            expect_rows
                .as_ref()
                .map_or(Ok(()), |expected| expected.check(affected))
        }
        (Step::Statement { sql, .. }, None) => {
            Err(Error::internal_message(format!("`{sql}` was not prepared")))
        }
    }
}

/// Dropped mid-transaction (a cancelled request), it is closed rather than
/// pooled, so the server rolls back and nobody reuses it inside the transaction.
pub(crate) struct OpenTransaction {
    pub(crate) connection: Option<Object>,
}

impl OpenTransaction {
    pub(crate) fn client(&self) -> &Client {
        self.connection
            .as_deref()
            .map(|wrapper| &**wrapper)
            .expect("an open transaction holds its connection until it ends")
    }

    async fn end(mut self, outcome: Result<()>) -> Result<()> {
        let ended = end(self.client(), outcome.is_ok()).await;
        self.closed_unless(&ended);
        first_failure(outcome, ended)
    }

    /// A connection whose transaction did not end cleanly is closed rather than pooled.
    pub(crate) fn closed_unless(&mut self, ended: &Result<()>) {
        if ended.is_ok() {
            self.connection = None;
        }
    }

    /// A step the server refuses makes `COMMIT` roll back; one refused before
    /// it was sent ends with `ROLLBACK`.
    async fn run_and_end(mut self, steps: &[Step], statements: &[Option<Statement>]) -> Result<()> {
        let client = self.client();
        let refused = AtomicBool::new(false);
        // Read once the steps were first polled, not now.
        let commit_or_rollback: Request<'_> =
            Box::pin(async { end(client, !refused.load(Ordering::Relaxed)).await });
        let pipeline = requests(client, steps, statements, &refused)
            .chain(std::iter::once(commit_or_rollback))
            .collect();
        let mut outcomes = join_all(pipeline).await;
        let ended = outcomes.pop().unwrap_or(Ok(()));
        let outcome = outcomes.into_iter().collect::<Result<()>>();
        self.closed_unless(&ended);
        first_failure(outcome, ended)
    }
}

/// The step's failure explains the rollback's, so it is the one returned.
pub(crate) fn first_failure(outcome: Result<()>, ended: Result<()>) -> Result<()> {
    match (outcome, ended) {
        (Err(failed), Err(ending)) => {
            tracing::warn!(
                error.kind = "rollback",
                error = %ending,
                "ending a failed transaction failed too; its connection is closed"
            );
            Err(failed)
        }
        (outcome, ended) => outcome.and(ended),
    }
}

async fn end(client: &Client, commit: bool) -> Result<()> {
    if commit {
        return simple(client, "COMMIT", "committing a transaction").await;
    }
    simple(client, "ROLLBACK", "rolling back a transaction").await
}

pub(crate) fn simple<'a>(client: &'a Client, sql: &'a str, context: &'static str) -> Request<'a> {
    Box::pin(async move {
        client
            .batch_execute(sql)
            .await
            .map_err(|error| classified(context, error))
    })
}

impl Drop for OpenTransaction {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            drop(Object::take(connection));
        }
    }
}
