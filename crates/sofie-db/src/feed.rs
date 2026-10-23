use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sofie_core::{Cast, ColumnReader, Error, Result, SqlValue, Uuid};
use tokio::sync::watch;

use crate::heard::Listeners;
use crate::messages::{Fanned, current_traceparent, fan_out_events};
use crate::worker::JobContext;
use crate::{AnyOutcome, BoxFuture, Db, Dialect, Outcome, Plan, Run, Step, query_decoded};

/// Entries older than this are pruned; a consumer further behind misses them.
const FEED_RETENTION: Duration = Duration::from_hours(7 * 24);
pub const FEED_PAGE: i64 = 100;
const PULL_INTERVAL: Duration = Duration::from_secs(1);
const PULL_INTERVAL_AFTER_FAILURE: Duration = Duration::from_secs(5);

/// `append` is serialized by a lock, so a reader past a position never misses an earlier one;
/// `receive` skips an id already queued; `advance` moves a cursor only from where it was.
pub struct FeedSql {
    pub append: &'static str,
    pub read: &'static str,
    pub prune: &'static str,
    pub receive: &'static str,
    pub cursor: &'static str,
    pub advance: &'static str,
}

/// An event as it crosses apps: its id stays the producer's, so it is delivered once.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FeedEntry {
    pub position: i64,
    pub id: Uuid,
    pub name: String,
    pub payload: Value,
    pub traceparent: Option<String>,
}

impl FeedEntry {
    fn decode(row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            position: row.int()?,
            id: row.uuid()?,
            name: row.text()?.to_owned(),
            payload: row.json()?,
            traceparent: Option::<String>::decode(row)?,
        })
    }
}

pub trait FeedSource: Send + Sync {
    fn producer(&self) -> &str;

    fn after(&self, position: i64, limit: i64) -> BoxFuture<'_, Result<Vec<FeedEntry>>>;
}

fn feed_sql(dialect: &dyn Dialect) -> Result<&'static FeedSql> {
    dialect
        .feed()
        .ok_or_else(|| Error::internal_message("this database keeps no event feed"))
}

pub async fn read_feed(db: &Db, after: i64, limit: i64) -> Result<Vec<FeedEntry>> {
    let params = [
        SqlValue::Int(after),
        SqlValue::Int(limit.clamp(1, FEED_PAGE)),
    ];
    query_decoded(
        db.driver(),
        feed_sql(db.driver().dialect())?.read,
        &params,
        Vec::new(),
        FeedEntry::decode,
    )
    .await
}

pub async fn feed_cursor(db: &Db, producer: &str) -> Result<i64> {
    let params = [SqlValue::Text(producer.to_owned())];
    let sql = feed_sql(db.driver().dialect())?.cursor;
    let found = query_decoded(db.driver(), sql, &params, Vec::new(), |row| row.int());
    Ok(found.await?.into_iter().next().unwrap_or(0))
}

/// A consumer racing from the same cursor fails as `Conflict`. Before any worker is published,
/// entries wait as events; one no listener hears is dropped with a warning after an hour.
pub async fn receive_feed(db: &Db, producer: &str, from: i64, entries: &[FeedEntry]) -> Result<()> {
    let Some(last) = entries.last() else {
        return Ok(());
    };
    let mut received = Received {
        producer,
        from,
        to: last.position,
        entries,
    };
    db.commit(&mut received).await
}

struct Received<'a> {
    producer: &'a str,
    from: i64,
    to: i64,
    entries: &'a [FeedEntry],
}

impl Received<'_> {
    /// Keyed by the producer's id, so an entry received again adds no job.
    fn fan_out(&self, plan: &mut Plan<'_>, listeners: &Listeners) -> Result<()> {
        let entries = self.entries.iter().map(|entry| Fanned {
            id: entry.id,
            name: &entry.name,
            payload: entry.payload.clone(),
            traceparent: entry.traceparent.clone(),
        });
        let now = plan.stamp().now.timestamp();
        if let Some(step) = fan_out_events(plan.dialect(), entries, now, None, listeners)? {
            plan.push_message(step);
        }
        Ok(())
    }

    fn queue(&self, plan: &mut Plan<'_>) -> Result<()> {
        let feed = feed_sql(plan.dialect())?;
        let now = SqlValue::Timestamp(plan.stamp().now.timestamp());
        for entry in self.entries {
            let params = vec![
                SqlValue::Uuid(entry.id),
                SqlValue::Text(entry.name.clone()),
                SqlValue::Json(entry.payload.clone()),
                now.clone(),
                entry
                    .traceparent
                    .clone()
                    .map_or(SqlValue::Null, SqlValue::Text),
            ];
            plan.push_message(Step::statement(feed.receive, params, None));
        }
        Ok(())
    }
}

impl Outcome for Received<'_> {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        let feed = feed_sql(plan.dialect())?;
        match plan.heard.clone() {
            Some(heard) => self.fan_out(plan, &heard.listeners)?,
            None => self.queue(plan)?,
        }
        let cursor = vec![
            SqlValue::Text(self.producer.to_owned()),
            SqlValue::Int(self.from),
            SqlValue::Int(self.to),
        ];
        plan.push(Step::statement(feed.advance, cursor, Some(1)));
        Ok(())
    }
}

/// Whether a full page came, so another may follow at once.
pub(crate) async fn pull_once(db: &Db, source: &dyn FeedSource) -> Result<bool> {
    let from = feed_cursor(db, source.producer()).await?;
    let entries = source.after(from, FEED_PAGE).await?;
    receive_feed(db, source.producer(), from, &entries).await?;
    Ok(i64::try_from(entries.len()).is_ok_and(|count| count >= FEED_PAGE))
}

pub(crate) async fn pull(db: Db, source: Arc<dyn FeedSource>, mut stopped: watch::Receiver<bool>) {
    loop {
        let wait = match pull_once(&db, source.as_ref()).await {
            Ok(true) => Duration::ZERO,
            Ok(false) => PULL_INTERVAL,
            Err(error) => {
                tracing::warn!(
                    error.kind = "feed",
                    feed.producer = source.producer(),
                    error = %error,
                    "pulling another app's events failed; trying again"
                );
                PULL_INTERVAL_AFTER_FAILURE
            }
        };
        if *stopped.borrow() {
            return;
        }
        if tokio::time::timeout(wait, stopped.changed()).await.is_ok() {
            return;
        }
    }
}

struct Appended {
    id: Uuid,
    name: &'static str,
    payload: Value,
}

impl Outcome for Appended {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        let params = vec![
            SqlValue::Uuid(self.id),
            SqlValue::Text(self.name.to_owned()),
            SqlValue::Json(self.payload.clone()),
            current_traceparent(),
        ];
        plan.push(Step::statement(
            feed_sql(plan.dialect())?.append,
            params,
            None,
        ));
        Ok(())
    }
}

/// Appended with the deletion of the message that carried it, so it is appended once.
pub(crate) fn append(event: &'static str) -> Run<JobContext> {
    Arc::new(move |context: &mut JobContext| {
        let appended = Appended {
            id: context.message,
            name: event,
            payload: context.payload.clone(),
        };
        Box::pin(std::future::ready(Ok(
            Box::new(appended) as Box<dyn AnyOutcome>
        )))
    })
}

struct Pruned;

impl Outcome for Pruned {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        let sql = feed_sql(plan.dialect())?;
        let before = plan
            .stamp()
            .now
            .timestamp()
            .checked_sub(FEED_RETENTION)
            .map_err(|error| Error::internal("computing the feed's retention", error))?;
        plan.push(Step::statement(
            sql.prune,
            vec![SqlValue::Timestamp(before)],
            None,
        ));
        Ok(())
    }
}

pub(crate) fn prune() -> Run<JobContext> {
    crate::erase(|| async { Ok(Pruned) })
}
