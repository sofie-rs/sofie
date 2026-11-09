//! Jobs keeping a search engine in step after each commit. A job reads the record as it is
//! then, so a retried or late job writes the latest state.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Value, json};
use sofie_core::{Cast, Error, Result, SqlType, SqlValue, Uuid};

use super::engine::{Installed, SearchEngine};
use super::texts::{PAGE, Record, Records, read_records, rewrite_stale};
use super::{Document, SEARCH_COLUMN, SearchIndex};
use crate::worker::{JobContext, Worker};
use crate::{Db, MessageKind, Outcome, Plan, Resolve, Write, query_decoded};

const INDEX_JOB: &str = "sofie.search.index";
const REINDEX_JOB: &str = "sofie.search.reindex_page";
const FORGET_JOB: &str = "sofie.search.forget";
const INDEX_TRIES: usize = 3;

/// On the worker of a database a search engine is installed on.
#[must_use]
pub fn index_searches(worker: Worker) -> Worker {
    worker
        .job(INDEX_JOB, index_record)
        .job(REINDEX_JOB, reindex_page)
        .job(FORGET_JOB, forget_documents)
}

pub(crate) fn changes_document(write: &Write<'_>) -> bool {
    let table = write.table;
    std::iter::once(SEARCH_COLUMN)
        .chain(table.hiding())
        .chain(table.owner)
        .any(|column| write.value(column) != write.loaded(column))
}

pub(crate) fn queue(write: &Write<'_>, plan: &mut Plan<'_>) -> Result<()> {
    let job = index_job(write.table.schema, write.table.name, write.id);
    plan.queue(MessageKind::Job, [job])
}

fn index_job(module: &str, aggregate: &str, id: Uuid) -> (&'static str, Value) {
    let payload = json!({ "module": module, "aggregate": aggregate, "id": id });
    (INDEX_JOB, payload)
}

/// In the prune's own commit: the rows are gone, so no later prune or reindex would find them.
#[cfg(feature = "retention")]
pub(crate) fn queue_forget(index: &SearchIndex, ids: &[Uuid], plan: &mut Plan<'_>) -> Result<()> {
    let payload = json!({ "module": index.module, "aggregate": index.name, "ids": ids });
    plan.queue(MessageKind::Job, [(FORGET_JOB, payload)])
}

#[cfg(feature = "retention")]
pub(crate) fn index_of(db: &Db, module: &str, name: &str) -> Option<SearchIndex> {
    db.search.get()?.find(module, name).cloned()
}

struct Forgetting {
    indexing: Indexing,
    ids: Vec<Uuid>,
}

impl Resolve<JobContext> for Forgetting {
    fn resolve(context: &mut JobContext) -> impl Future<Output = Result<Self>> + Send {
        let ids = context
            .payload
            .get("ids")
            .cloned()
            .map(serde_json::from_value::<Vec<Uuid>>)
            .transpose()
            .map_err(|error| Error::internal("reading a forgetting job's ids", error))
            .and_then(|ids| {
                ids.ok_or_else(|| Error::internal_message("a forgetting job names no ids"))
            });
        let forgetting = ids
            .and_then(|ids| Indexing::read(context, "id").map(|indexing| Self { indexing, ids }));
        std::future::ready(forgetting)
    }
}

async fn forget_documents(job: Forgetting) -> Result<()> {
    let Forgetting { indexing, ids } = job;
    indexing.engine.remove(&indexing.index, &ids).await
}

struct Indexing {
    db: Db,
    engine: Arc<dyn SearchEngine>,
    index: SearchIndex,
    id: Option<Uuid>,
}

impl Indexing {
    fn read(context: &JobContext, id_key: &str) -> Result<Self> {
        let installed = Installed::of(&context.db)?;
        let text = |key: &str| {
            context
                .payload
                .get(key)
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    Error::internal_message(format!("an indexing job's payload has no `{key}`"))
                })
        };
        let index = installed
            .index(text("module")?, text("aggregate")?)?
            .clone();
        let id = match context.payload.get(id_key) {
            None | Some(Value::Null) => None,
            Some(_) => Some(text(id_key)?.parse().map_err(|_| {
                Error::internal_message(format!("an indexing job's `{id_key}` is not an id"))
            })?),
        };
        Ok(Self {
            db: context.db.clone(),
            engine: Arc::clone(&installed.engine),
            index,
            id,
        })
    }
}

struct OneRecord(Indexing);

impl Resolve<JobContext> for OneRecord {
    fn resolve(context: &mut JobContext) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(Indexing::read(context, "id").map(Self))
    }
}

struct NextPage(Indexing);

impl Resolve<JobContext> for NextPage {
    fn resolve(context: &mut JobContext) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(Indexing::read(context, "after").map(Self))
    }
}

/// Read again after each write: a job for a later version may have landed first, and this one's
/// write then undid it. A record rewritten under every try is a conflict, retried later.
async fn index_record(job: OneRecord) -> Result<()> {
    let OneRecord(job) = job;
    let id = job
        .id
        .ok_or_else(|| Error::internal_message("an indexing job names no record"))?;
    let mut records = read_records(&job.db, &job.index, Records::One(id)).await?;
    for _ in 0..INDEX_TRIES {
        rewrite_stale(&job.db, &job.index, &records).await?;
        let indexed = records.first().map(|record| record.version.clone());
        match records.into_iter().next() {
            Some(record) if !record.hidden => job.engine.put(&job.index, &[record.document]).await,
            _ => job.engine.remove(&job.index, &[id]).await,
        }?;
        records = read_records(&job.db, &job.index, Records::One(id)).await?;
        if records.first().map(|record| record.version.clone()) == indexed {
            return Ok(());
        }
    }
    Err(Error::Conflict)
}

async fn reindex_page(job: NextPage) -> Result<Reindexing> {
    let NextPage(job) = job;
    let records = read_records(&job.db, &job.index, Records::After(job.id)).await?;
    rewrite_stale(&job.db, &job.index, &records).await?;
    let last = records.last().map(|record| record.document.id);
    let full = records.len() == PAGE;
    let written: Vec<(Uuid, SqlValue)> = records
        .iter()
        .map(|record| (record.document.id, record.version.clone()))
        .collect();
    let (hidden, shown): (Vec<Record>, Vec<Record>) =
        records.into_iter().partition(|record| record.hidden);
    let documents: Vec<Document> = shown.into_iter().map(|record| record.document).collect();
    if !documents.is_empty() {
        job.engine.put(&job.index, &documents).await?;
    }
    let removed: Vec<Uuid> = hidden.iter().map(|record| record.document.id).collect();
    if !removed.is_empty() {
        job.engine.remove(&job.index, &removed).await?;
    }
    let moved = moved_since(&job.db, &job.index, &written).await?;
    let pages = match last {
        Some(last) if full => vec![(job.index.clone(), Some(last))],
        _ => Vec::new(),
    };
    Ok(Reindexing {
        pages,
        moved: moved
            .into_iter()
            .map(|id| (job.index.clone(), id))
            .collect(),
    })
}

/// A commit between the page's read and its write was undone by it: index those records again.
async fn moved_since(
    db: &Db,
    index: &SearchIndex,
    written: &[(Uuid, SqlValue)],
) -> Result<Vec<Uuid>> {
    let (Some((first, _)), Some((last, _))) = (written.first(), written.last()) else {
        return Ok(Vec::new());
    };
    let dialect = db.driver().dialect();
    let id = dialect.quote("id");
    let sql = format!(
        "SELECT {id}, {} FROM {} WHERE {id} >= {} AND {id} <= {}",
        dialect.quote("version"),
        dialect.qualified(index.module, index.name),
        dialect.placeholder(1),
        dialect.placeholder(2),
    );
    let params = [SqlValue::Uuid(*first), SqlValue::Uuid(*last)];
    let types = vec![Some(SqlType::Uuid), Some(SqlType::Int)];
    let now: HashMap<Uuid, SqlValue> = query_decoded(db.driver(), &sql, &params, types, |row| {
        Ok((Uuid::decode(row)?, SqlValue::Int(i64::decode(row)?)))
    })
    .await?
    .into_iter()
    .collect();
    Ok(written
        .iter()
        .filter(|(id, version)| now.get(id) != Some(version))
        .map(|(id, _)| *id)
        .collect())
}

pub struct Reindexing {
    pages: Vec<(SearchIndex, Option<Uuid>)>,
    moved: Vec<(SearchIndex, Uuid)>,
}

impl Reindexing {
    /// Each page queues the next, as a mirror's backfill does, so a stopped worker resumes.
    pub(crate) fn every_page(indexes: &[SearchIndex]) -> Self {
        Self {
            pages: indexes.iter().map(|index| (index.clone(), None)).collect(),
            moved: Vec::new(),
        }
    }
}

impl Outcome for Reindexing {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        let pages = self.pages.iter().map(|(index, after)| {
            let payload = json!({
                "module": index.module,
                "aggregate": index.name,
                "after": after,
            });
            (REINDEX_JOB, payload)
        });
        let records = self
            .moved
            .iter()
            .map(|(index, id)| index_job(index.module, index.name, *id));
        plan.queue(MessageKind::Job, pages.chain(records))
    }
}
