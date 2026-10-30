use sofie_core::{Error, Result, SqlValue};
use sofie_db::{BoxFuture, RowSink, join_all, traced_statement};

use crate::rows::read_rows;
use crate::transaction::{OpenTransaction, first_failure, simple};
use crate::{Postgres, prepared};

impl Postgres {
    pub(crate) async fn rows_read_only(
        &self,
        role: Option<&str>,
        sql: &str,
        params: &[SqlValue],
        rows: &mut dyn RowSink,
    ) -> Result<()> {
        let enter_role = role.map(crate::dialect::entering_role);
        let connection = self.connection().await?;
        let statement = prepared(&connection, sql).await?;
        let mut open = OpenTransaction {
            connection: Some(connection),
        };
        let client = open.client();
        let query: BoxFuture<'_, Result<()>> = Box::pin(async {
            traced_statement(
                sql,
                read_rows(client, &statement, sql, params, rows),
                |count| Some(*count),
            )
            .await
            .map(drop)
        });
        let mut pipeline = vec![simple(
            client,
            "BEGIN READ ONLY",
            "starting a read-only transaction",
        )];
        pipeline.extend(
            enter_role
                .as_deref()
                .map(|statement| simple(client, statement, "entering the module's role")),
        );
        pipeline.push(query);
        pipeline.push(simple(client, "COMMIT", "ending a read-only transaction"));
        let mut outcomes = join_all(pipeline).await;
        let ended = outcomes.pop();
        let read = outcomes.pop();
        // A query the server refused aborted the transaction, which `COMMIT`
        // then rolls back without an error of its own.
        let ended = ended.unwrap_or(Ok(()));
        open.closed_unless(&ended);
        let read = outcomes.into_iter().collect::<Result<()>>().and_then(|()| {
            read.unwrap_or_else(|| Err(Error::internal_message("the read-only query never ran")))
        });
        first_failure(read, ended)
    }
}
