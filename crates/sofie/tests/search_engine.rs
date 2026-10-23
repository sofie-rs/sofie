#![cfg(all(feature = "search-meilisearch", feature = "soft-delete"))]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use sofie::db::{
    AggregateTable, BoxFuture, Db, Document, File, Migrations, REINDEX_TASK, Reach, SearchEngine,
    SearchIndex, Uuid, index_searches, install_search, reindex,
};
use sofie::postgres::Postgres;
use sofie::prelude::*;
use sofie::worker::Worker;
use sofie_core::unpoisoned;
use sofie_testing::TestDatabase;

const MIGRATIONS: &[File] = &[File::new(
    "0001_product.sql",
    b"CREATE TABLE product (
        id uuid PRIMARY KEY,
        version bigint NOT NULL,
        created_at timestamptz NOT NULL,
        updated_at timestamptz NOT NULL,
        deleted_at timestamptz,
        search_text text NOT NULL DEFAULT '',
        account uuid NOT NULL,
        title text NOT NULL,
        stock bigint NOT NULL
    );",
)];
const TABLES: &[AggregateTable] = &[AggregateTable::of::<Product>()];
const MINE: Uuid = Uuid::from_u128(1);
const THEIRS: Uuid = Uuid::from_u128(2);

#[derive(Aggregate)]
#[owner]
pub struct Account {
    meta: Meta,
}

#[derive(Aggregate)]
#[aggregate(behaviors(SoftDelete, Searched))]
pub struct Product {
    meta: Meta,
    #[owner]
    account: Id<Account>,
    title: String,
    stock: i64,
}

fn product(account: Uuid, title: &str) -> Product {
    Product {
        meta: Meta::new().unwrap(),
        account: Id::from_uuid(account),
        title: title.to_owned(),
        stock: 1,
    }
}

/// Matches every word anywhere, whoever owns it, so only the database keeps an owner to its own.
#[derive(Default)]
struct FakeEngine {
    documents: Mutex<BTreeMap<Uuid, Document>>,
    owners_asked: Mutex<Vec<Option<Uuid>>>,
}

impl FakeEngine {
    fn documents(&self) -> Vec<Document> {
        let documents = unpoisoned::lock(&self.documents);
        documents.values().cloned().collect()
    }

    fn texts(&self) -> Vec<String> {
        self.documents()
            .into_iter()
            .map(|document| document.text)
            .collect()
    }
}

impl SearchEngine for FakeEngine {
    fn matching<'a>(
        &'a self,
        _index: &'a SearchIndex,
        words: &'a str,
        owner: Option<Uuid>,
    ) -> BoxFuture<'a, sofie::core::Result<Vec<Uuid>>> {
        unpoisoned::lock(&self.owners_asked).push(owner);
        let words = words.to_lowercase();
        let ids = self
            .documents()
            .into_iter()
            .filter(|document| {
                let text = document.text.to_lowercase();
                words.split_whitespace().all(|word| text.contains(word))
            })
            .map(|document| document.id)
            .collect();
        Box::pin(std::future::ready(Ok(ids)))
    }

    fn put<'a>(
        &'a self,
        _index: &'a SearchIndex,
        documents: &'a [Document],
    ) -> BoxFuture<'a, sofie::core::Result<()>> {
        let mut kept = unpoisoned::lock(&self.documents);
        for document in documents {
            kept.insert(document.id, document.clone());
        }
        Box::pin(std::future::ready(Ok(())))
    }

    fn remove<'a>(
        &'a self,
        _index: &'a SearchIndex,
        ids: &'a [Uuid],
    ) -> BoxFuture<'a, sofie::core::Result<()>> {
        let mut kept = unpoisoned::lock(&self.documents);
        for id in ids {
            kept.remove(id);
        }
        Box::pin(std::future::ready(Ok(())))
    }
}

async fn started(database: &TestDatabase) -> (Db, Arc<FakeEngine>) {
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    let migrations = [Migrations::new("search_engine", MIGRATIONS).aggregates(TABLES)];
    db.schema().migrate(&migrations).await.unwrap();
    let engine = Arc::new(FakeEngine::default());
    install_search(
        &db,
        Arc::clone(&engine) as Arc<dyn SearchEngine>,
        &migrations,
    );
    (db, engine)
}

async fn indexed(db: &Db) -> usize {
    index_searches(Worker::new(db.clone()))
        .drain()
        .await
        .unwrap()
}

async fn titles(db: &Db, reach: Reach, words: &str) -> Vec<String> {
    let read = sofie::db::Read::within(db, reach);
    let found = Product::query()
        .where_(Product::matching(words))
        .order_by(Product::TITLE.asc())
        .all(&read)
        .await
        .unwrap();
    found.into_iter().map(|product| product.title).collect()
}

#[tokio::test]
async fn the_worker_indexes_each_committed_change_and_reads_keep_to_their_owner() {
    let database = TestDatabase::create().await;
    let (db, engine) = started(&database).await;
    let mut products = vec![
        product(MINE, "Blue wool scarf"),
        product(THEIRS, "Red wool hat"),
        product(MINE, "Cotton shirt"),
    ];
    db.commit(&mut products).await.unwrap();
    assert!(
        engine.documents().is_empty(),
        "nothing is indexed inside the commit"
    );

    assert_eq!(indexed(&db).await, 3);

    assert_eq!(engine.documents().len(), 3);
    assert_eq!(
        titles(&db, Reach::OwnedBy(MINE), "wool").await,
        ["Blue wool scarf"]
    );
    assert_eq!(
        titles(&db, Reach::All, "wool").await,
        ["Blue wool scarf", "Red wool hat"]
    );
    let asked = unpoisoned::lock(&engine.owners_asked).clone();
    assert_eq!(asked, [Some(MINE), None]);
    let counted = Product::query()
        .where_(Product::matching("wool"))
        .count(&db.read())
        .await
        .unwrap();
    assert_eq!(counted, 2);

    products[2].stock = 5;
    db.commit(&mut products).await.unwrap();
    assert_eq!(
        indexed(&db).await,
        0,
        "a change the engine keeps nothing of"
    );

    products[0].title = "Blue linen scarf".to_owned();
    products[1].delete();
    db.commit(&mut products).await.unwrap();
    assert_eq!(indexed(&db).await, 2);
    assert_eq!(
        engine.texts(),
        [
            format!("{MINE}\nBlue linen scarf"),
            format!("{MINE}\nCotton shirt")
        ]
    );
    assert_eq!(titles(&db, Reach::All, "wool").await, Vec::<String>::new());
}

#[tokio::test]
async fn the_reindex_task_pages_through_every_record_and_leaves_deleted_ones_out() {
    let database = TestDatabase::create().await;
    let (db, engine) = started(&database).await;
    let mut kept = product(MINE, "Blue wool scarf");
    let mut deleted = product(THEIRS, "Red wool hat");
    db.commit(&mut kept).await.unwrap();
    deleted.delete();
    db.commit(&mut deleted).await.unwrap();
    indexed(&db).await;
    engine
        .put(
            &SearchIndex::new("search_engine", "product", None),
            &[Document {
                id: deleted.meta.id(),
                text: "stale".to_owned(),
                owner: None,
            }],
        )
        .await
        .unwrap();

    sofie::Tasks::new()
        .task(REINDEX_TASK, reindex)
        .run(&db, REINDEX_TASK, serde_json::Map::new())
        .await
        .unwrap();
    indexed(&db).await;

    assert_eq!(engine.texts(), [format!("{MINE}\nBlue wool scarf")]);
    assert_eq!(engine.documents()[0].owner, Some(MINE));
}
