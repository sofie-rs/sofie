use std::sync::{Arc, Mutex, MutexGuard};

use sofie_core::{Error, Result, RowReader, SqlValue, unpoisoned};
use sofie_db::{BoxFuture, Dialect, Driver, MessageSql, Migrations, RowSink, Step};

/// A test build chooses no gateway adapter, so a test that reaches a gateway provides its fake here.
#[cfg(feature = "contracts")]
pub fn fake<G: ?Sized + Send + Sync + 'static>(app: &crate::TestApp, fake: Arc<G>) {
    sofie_db::provide::<G>(app.db(), fake);
}

#[derive(Clone, Default)]
pub struct FakeDriver {
    record: Arc<Mutex<Record>>,
    rows: Arc<Vec<Vec<SqlValue>>>,
    is_down: bool,
}

#[derive(Default)]
struct Record {
    queries: Vec<(String, Vec<SqlValue>)>,
    transactions: Vec<Vec<Step>>,
}

impl FakeDriver {
    #[must_use]
    pub fn returning(rows: Vec<Vec<SqlValue>>) -> Self {
        Self {
            rows: Arc::new(rows),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn down() -> Self {
        Self {
            is_down: true,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn queries(&self) -> Vec<(String, Vec<SqlValue>)> {
        self.record().queries.clone()
    }

    #[must_use]
    pub fn transaction_count(&self) -> usize {
        self.record().transactions.len()
    }

    #[must_use]
    pub fn take_transactions(&self) -> Vec<Vec<Step>> {
        std::mem::take(&mut self.record().transactions)
    }

    fn record(&self) -> MutexGuard<'_, Record> {
        unpoisoned::lock(&self.record)
    }

    fn outcome(&self) -> Result<()> {
        if self.is_down {
            return Err(Error::internal_message("connection refused"));
        }
        Ok(())
    }
}

impl Driver for FakeDriver {
    fn dialect(&self) -> &dyn Dialect {
        &FakeDialect
    }

    fn query<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [SqlValue],
        rows: &'a mut dyn RowSink,
    ) -> BoxFuture<'a, Result<()>> {
        self.record()
            .queries
            .push((sql.to_owned(), params.to_vec()));
        let answer = self.outcome().and_then(|()| {
            self.rows
                .iter()
                .try_for_each(|values| rows.row(&mut RowReader::new(values)))
        });
        Box::pin(std::future::ready(answer))
    }

    fn transaction(&self, steps: Vec<Step>) -> BoxFuture<'_, Result<()>> {
        self.record().transactions.push(steps);
        Box::pin(std::future::ready(self.outcome()))
    }
}

struct FakeDialect;

const NO_MESSAGES: MessageSql = MessageSql {
    insert: "",
    fan_out: "",
    fan_out_events: "",
    claim: "",
    renew: "",
    complete: "",
    retry: "",
    bury: "",
    to_job: "",
    add_schedule: "",
    fire_schedule: "",
    notify: None,
};

impl Dialect for FakeDialect {
    fn push_placeholder(&self, sql: &mut String, index: usize) {
        sql.push('$');
        sql.push_str(&index.to_string());
    }

    fn migrations_setup(&self) -> &'static str {
        ""
    }

    fn migrations_lock(&self) -> &'static str {
        ""
    }

    fn enter_schema(&self, _schema: &str) -> String {
        String::new()
    }

    fn framework_migrations(&self) -> Migrations {
        Migrations::new("sofie", &[])
    }

    fn messages(&self) -> &'static MessageSql {
        &NO_MESSAGES
    }
}
