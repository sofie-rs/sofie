use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use sofie_core::{Cast, ColumnReader, Error, Instant, Result, SqlValue, Uuid};
use sofie_db::worker::Worker;
use sofie_db::{
    Aggregate, AggregateTable, Behavior, BoxFuture, Column, Context, Db, Document, Driver, Field,
    File, Meta, Migrations, Prune, Reach, Resolve, SearchEngine, SearchIndex, SearchIndexes,
    Searched, Step, Writes, index_searches, install_search, query_values, reindex,
};
use sofie_postgres::Postgres;
use sofie_testing::TestDatabase;
use sofie_values::Search;

const MIGRATIONS: &[File] = &[File::new(
    "0001_note.sql",
    b"CREATE TABLE note (
        id uuid PRIMARY KEY,
        version bigint NOT NULL,
        search_text text NOT NULL DEFAULT '',
        title text NOT NULL,
        pin text NOT NULL,
        written timestamptz NOT NULL
    );",
)];
const TABLES: &[AggregateTable] = &[AggregateTable::of::<Note>()];

/// `pin` stands for a field marked `#[sensitive]` after records were written with it searched.
struct Note {
    meta: Meta,
    title: String,
    pin: String,
    written: Instant,
}

impl Note {
    const WRITTEN: Column<Self, Instant> = Column::new("written");

    fn new(title: &str) -> Self {
        Self {
            meta: Meta::new().unwrap(),
            title: title.to_owned(),
            pin: "4711".to_owned(),
            written: Instant::parse("2026-01-01T00:00:00Z").unwrap(),
        }
    }
}

impl Aggregate for Note {
    const NAME: &'static str = "note";
    const MODULE: &'static str = "search_test";
    const FIELDS: &'static [Field] = &[
        Field::new("title", String::COLUMNS, false),
        Field::new("pin", String::COLUMNS, false),
        Field::new("written", Instant::COLUMNS, false),
    ];
    const BEHAVIORS: &'static [&'static dyn Behavior] = &[&Searched {
        fields: &[("title", String::COLUMNS, Search::Text)],
    }];

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.title.encode(out);
        self.pin.encode(out);
        self.written.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            title: String::decode(row)?,
            pin: String::decode(row)?,
            written: Instant::decode(row)?,
        })
    }
}

#[derive(Default)]
struct Kept {
    documents: Mutex<BTreeMap<Uuid, Document>>,
    failing_removes: AtomicUsize,
    crashing_removes: AtomicUsize,
    /// Rewrites the record and indexes it before the first put lands, as a later job would.
    overtaken_by: Mutex<Option<Postgres>>,
}

impl SearchEngine for Kept {
    fn matching<'a>(
        &'a self,
        _: &'a SearchIndex,
        _: &'a str,
        _: Option<Uuid>,
    ) -> BoxFuture<'a, Result<Vec<Uuid>>> {
        Box::pin(std::future::ready(Ok(Vec::new())))
    }

    fn put<'a>(
        &'a self,
        _: &'a SearchIndex,
        documents: &'a [Document],
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let overtaking = self.overtaken_by.lock().unwrap().take();
            if let Some(raw) = overtaking {
                raw.transaction(vec![Step::Script(
                    "UPDATE search_test.note SET title = 'Shopping', search_text = 'Shopping', \
                     version = version + 1"
                        .to_owned(),
                )])
                .await?;
                let mut later = documents[0].clone();
                "Shopping".clone_into(&mut later.text);
                self.documents.lock().unwrap().insert(later.id, later);
            }
            let mut kept = self.documents.lock().unwrap();
            for document in documents {
                kept.insert(document.id, document.clone());
            }
            Ok(())
        })
    }

    fn remove<'a>(&'a self, _: &'a SearchIndex, ids: &'a [Uuid]) -> BoxFuture<'a, Result<()>> {
        let crashing = self.crashing_removes.load(Ordering::Relaxed);
        if crashing > 0 {
            self.crashing_removes.store(crashing - 1, Ordering::Relaxed);
            panic!("the worker stopped");
        }
        let failing = self.failing_removes.load(Ordering::Relaxed);
        if failing > 0 {
            self.failing_removes.store(failing - 1, Ordering::Relaxed);
            let refused = Err(Error::Unavailable("the engine is down".to_owned()));
            return Box::pin(std::future::ready(refused));
        }
        let mut kept = self.documents.lock().unwrap();
        for id in ids {
            kept.remove(id);
        }
        Box::pin(std::future::ready(Ok(())))
    }
}

struct Task(Db);

impl Context for Task {
    fn db(&self) -> &Db {
        &self.0
    }

    fn reach(&self) -> Reach {
        Reach::All
    }
}

impl Writes for Task {}

async fn started(database: &TestDatabase) -> (Db, Arc<Kept>) {
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    let migrations = [Migrations::new("search_test", MIGRATIONS).aggregates(TABLES)];
    db.schema().migrate(&migrations).await.unwrap();
    let engine = Arc::new(Kept::default());
    install_search(
        &db,
        Arc::clone(&engine) as Arc<dyn SearchEngine>,
        &migrations,
    );
    (db, engine)
}

async fn retry_now(database: &TestDatabase) {
    Postgres::connect(database.url())
        .unwrap()
        .transaction(vec![Step::Script(
            "UPDATE sofie.messages SET available_at = now() - interval '1 second'".to_owned(),
        )])
        .await
        .unwrap();
}

fn kept_ids(engine: &Kept) -> Vec<Uuid> {
    engine.documents.lock().unwrap().keys().copied().collect()
}

async fn prune_all(prune: Prune) -> Result<()> {
    let later = Instant::parse("2027-01-01T00:00:00Z")?;
    prune.before(Note::WRITTEN, later).await.map(drop)
}

struct Expired;

impl serde::Serialize for Expired {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_unit_struct("Expired")
    }
}

impl sofie_db::Input for Expired {
    fn from_json(_: serde_json::Value) -> Result<Self> {
        Ok(Self)
    }
}

impl sofie_db::Event for Expired {
    const NAME: &'static str = "search_test.Expired";
}

#[tokio::test]
async fn records_a_prune_failed_to_remove_from_the_engine_are_removed_by_a_retry() {
    let database = TestDatabase::create().await;
    let (db, engine) = started(&database).await;
    let worker =
        || index_searches(Worker::new(db.clone())).on::<Expired, _>("prune_all", prune_all);
    db.commit(&mut Note::new("Groceries")).await.unwrap();
    worker().drain().await.unwrap();
    assert_eq!(kept_ids(&engine).len(), 1);
    engine.failing_removes.store(1, Ordering::Relaxed);

    let mut trigger = Note::new("Trigger");
    trigger.meta.raise(Expired).unwrap();
    db.commit(&mut trigger).await.unwrap();
    worker().drain().await.unwrap();
    retry_now(&database).await;
    worker().drain().await.unwrap();

    assert_eq!(kept_ids(&engine), Vec::<Uuid>::new(), "both pruned");
}

#[tokio::test]
async fn a_reindex_recomputes_each_record_s_text_from_the_fields_searched_now() {
    let database = TestDatabase::create().await;
    let (db, engine) = started(&database).await;
    let mut note = Note::new("Groceries");
    db.commit(&mut note).await.unwrap();
    let raw = Postgres::connect(database.url()).unwrap();
    raw.transaction(vec![Step::Script(
        "UPDATE search_test.note SET search_text = 'Groceries\n4711'".to_owned(),
    )])
    .await
    .unwrap();

    let indexes = SearchIndexes::resolve(&mut Task(db.clone())).await.unwrap();
    db.commit(&mut reindex(indexes).await.unwrap())
        .await
        .unwrap();
    index_searches(Worker::new(db.clone()))
        .drain()
        .await
        .unwrap();

    let texts: Vec<String> = engine
        .documents
        .lock()
        .unwrap()
        .values()
        .map(|document| document.text.clone())
        .collect();
    assert_eq!(texts, ["Groceries"]);
    let stored = query_values(&raw, "SELECT search_text FROM search_test.note", &[])
        .await
        .unwrap();
    assert_eq!(stored, [[SqlValue::Text("Groceries".to_owned())]]);
}

#[tokio::test]
async fn an_index_job_overtaken_by_a_later_one_leaves_the_later_text() {
    let database = TestDatabase::create().await;
    let (db, engine) = started(&database).await;
    *engine.overtaken_by.lock().unwrap() = Some(Postgres::connect(database.url()).unwrap());
    db.commit(&mut Note::new("Groceries")).await.unwrap();

    index_searches(Worker::new(db.clone()))
        .drain()
        .await
        .unwrap();

    let texts: Vec<String> = engine
        .documents
        .lock()
        .unwrap()
        .values()
        .map(|document| document.text.clone())
        .collect();
    assert_eq!(texts, ["Shopping"]);
}

#[tokio::test]
async fn a_reindex_page_overtaken_by_a_later_change_leaves_the_later_text() {
    let database = TestDatabase::create().await;
    let (db, engine) = started(&database).await;
    db.commit(&mut Note::new("Groceries")).await.unwrap();
    let worker = || index_searches(Worker::new(db.clone()));
    worker().drain().await.unwrap();
    *engine.overtaken_by.lock().unwrap() = Some(Postgres::connect(database.url()).unwrap());

    let indexes = SearchIndexes::resolve(&mut Task(db.clone())).await.unwrap();
    db.commit(&mut reindex(indexes).await.unwrap())
        .await
        .unwrap();
    worker().drain().await.unwrap();

    let texts: Vec<String> = engine
        .documents
        .lock()
        .unwrap()
        .values()
        .map(|document| document.text.clone())
        .collect();
    assert_eq!(texts, ["Shopping"]);
}

#[tokio::test]
async fn records_pruned_before_a_crash_are_removed_from_the_engine() {
    let database = TestDatabase::create().await;
    let (db, engine) = started(&database).await;
    let worker =
        || index_searches(Worker::new(db.clone())).on::<Expired, _>("prune_all", prune_all);
    db.commit(&mut Note::new("Groceries")).await.unwrap();
    worker().drain().await.unwrap();
    engine.crashing_removes.store(1, Ordering::Relaxed);

    let mut trigger = Note::new("Trigger");
    trigger.meta.raise(Expired).unwrap();
    db.commit(&mut trigger).await.unwrap();
    worker().drain().await.unwrap();
    retry_now(&database).await;
    worker().drain().await.unwrap();

    assert_eq!(kept_ids(&engine), Vec::<Uuid>::new());
}
