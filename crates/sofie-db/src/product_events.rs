use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use sofie_core::{Date, Error, Result, SqlValue, Timestamp};
use sofie_telemetry::{ProductRecord, keep_product_events};
use tokio::sync::{mpsc, oneshot};

use crate::{Db, Dialect, query_decoded, query_values};

/// Past it, events are dropped rather than slowing the code that raises them.
const QUEUED: usize = 8192;
const BATCH: usize = 512;

/// `cohorts` answers `start, offset, count` rows, an offset of -1 counting the cohort.
pub struct ProductSql {
    pub insert: &'static str,
    pub cohorts: &'static str,
    /// Deletes at most a batch, answering how many it deleted.
    pub prune: &'static str,
    pub forget: &'static str,
    /// Deletes what a write racing `forget` kept.
    pub sweep: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Period {
    Day,
    Week,
    Month,
}

impl Period {
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "day" => Some(Self::Day),
            "week" => Some(Self::Week),
            "month" => Some(Self::Month),
            _ => None,
        }
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
        }
    }
}

/// Principals by the period of their first `start` since `since`, then who raised `returning`.
pub struct Retention<'a> {
    pub start: &'a str,
    pub returning: &'a str,
    pub period: Period,
    pub since: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cohort {
    pub start: Date,
    pub size: u64,
    pub returned: Vec<u64>,
}

/// Only events raised by a signed-in principal join a cohort.
pub async fn cohorts(db: &Db, retention: &Retention<'_>) -> Result<Vec<Cohort>> {
    let sql = product_sql(db.driver().dialect())?;
    let params = [
        SqlValue::Text(retention.period.name().to_owned()),
        SqlValue::Text(retention.start.to_owned()),
        SqlValue::Text(retention.returning.to_owned()),
        SqlValue::Timestamp(retention.since),
    ];
    let rows = query_values(db.driver(), sql.cohorts, &params).await?;
    let mut cohorts: Vec<Cohort> = Vec::new();
    for row in rows {
        let [
            SqlValue::DateTime(start),
            SqlValue::Int(offset),
            SqlValue::Int(count),
        ] = row.as_slice()
        else {
            return Err(Error::internal_message(format!(
                "a cohort row has unexpected columns: {:?}",
                row.iter().map(SqlValue::kind).collect::<Vec<_>>()
            )));
        };
        let count = u64::try_from(*count).unwrap_or_default();
        let start = start.date();
        if cohorts.last().is_none_or(|cohort| cohort.start != start) {
            cohorts.push(Cohort {
                start,
                size: 0,
                returned: Vec::new(),
            });
        }
        let cohort = cohorts.last_mut().expect("a cohort was just pushed");
        match usize::try_from(*offset) {
            Err(_) => cohort.size = count,
            Ok(offset) => {
                if cohort.returned.len() <= offset {
                    cohort.returned.resize(offset + 1, 0);
                }
                cohort.returned[offset] = count;
            }
        }
    }
    Ok(cohorts)
}

impl crate::Prune {
    pub async fn product_events(&self, before: sofie_core::Instant) -> Result<u64> {
        let sql = product_sql(self.db().driver().dialect())?;
        query_values(self.db().driver(), sql.sweep, &[]).await?;
        let params = [SqlValue::Timestamp(before.timestamp())];
        let mut deleted = 0;
        loop {
            let counted =
                query_decoded(self.db().driver(), sql.prune, &params, Vec::new(), |row| {
                    row.int()
                })
                .await?;
            let batch = match counted[..] {
                [count] => u64::try_from(count)
                    .map_err(|error| Error::internal("counting deleted events", error))?,
                _ => {
                    return Err(Error::internal_message(
                        "pruning answered no count of deleted events",
                    ));
                }
            };
            if batch == 0 {
                return Ok(deleted);
            }
            deleted += batch;
        }
    }
}

pub(crate) fn product_sql(dialect: &dyn Dialect) -> Result<&'static ProductSql> {
    dialect
        .product_events()
        .ok_or_else(|| Error::internal_message("this database keeps no product events"))
}

enum Kept {
    Event(ProductRecord),
    Flush(oneshot::Sender<()>),
}

pub struct ProductEvents {
    queue: mpsc::Sender<Kept>,
}

impl ProductEvents {
    /// Once per process; it must run inside the async runtime.
    pub fn keep(db: &Db) -> Result<Self> {
        let sql = product_sql(db.driver().dialect())?;
        let (queue, mut received) = mpsc::channel(QUEUED);
        let raised = queue.clone();
        let dropped = Arc::new(AtomicU64::new(0));
        let installed = keep_product_events(move |record| {
            if raised.try_send(Kept::Event(record)).is_ok() {
                return;
            }
            let lost = dropped.fetch_add(1, Ordering::Relaxed) + 1;
            if lost.is_power_of_two() {
                tracing::warn!(
                    product.dropped = lost,
                    "product events dropped: the database keeps up with fewer"
                );
            }
        });
        if !installed {
            return Err(Error::internal_message(
                "product events are kept already in this process",
            ));
        }
        let db = db.clone();
        tokio::spawn(async move {
            let mut batch = Vec::with_capacity(BATCH);
            while let Some(first) = received.recv().await {
                let mut flushed = Vec::new();
                let mut next = Some(first);
                while let Some(kept) = next {
                    match kept {
                        Kept::Event(record) => batch.push(record),
                        Kept::Flush(done) => flushed.push(done),
                    }
                    next = (batch.len() < BATCH)
                        .then(|| received.try_recv().ok())
                        .flatten();
                }
                write(&db, sql, &mut batch).await;
                for done in flushed {
                    let _ = done.send(());
                }
            }
        });
        Ok(Self { queue })
    }

    pub async fn flush(&self) {
        let (done, written) = oneshot::channel();
        if self.queue.send(Kept::Flush(done)).await.is_ok() {
            let _ = written.await;
        }
    }
}

async fn write(db: &Db, sql: &ProductSql, batch: &mut Vec<ProductRecord>) {
    if batch.is_empty() {
        return;
    }
    let mut at = Vec::with_capacity(batch.len());
    let mut names = Vec::with_capacity(batch.len());
    let mut principals = Vec::with_capacity(batch.len());
    for record in batch.drain(..) {
        at.push(SqlValue::Timestamp(record.at));
        names.push(SqlValue::Text(record.name));
        principals.push(record.principal.map_or(SqlValue::Null, SqlValue::Uuid));
    }
    let count = at.len();
    let params = [
        SqlValue::Array(at),
        SqlValue::Array(names),
        SqlValue::Array(principals),
    ];
    if let Err(error) = query_values(db.driver(), sql.insert, &params).await {
        tracing::warn!(
            error = %error,
            product.lost = count,
            "writing product events failed"
        );
    }
}
