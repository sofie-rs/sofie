mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use common::{Book, TestContext};
use sofie_cache::{Cache, Key, Store};
use sofie_db::{Aggregate, Db, File, Meta, Migrations, Resolve};
use sofie_sqlite::Sqlite;

const SHELF: Migrations = Migrations::new("shelf", &[File::new("0001_books.sql", b"")])
    .sqlite(&[File::new(
    "0001_books.sql",
    b"CREATE TABLE shelf_books (id uuid PRIMARY KEY, version bigint NOT NULL, title text NOT NULL);",
)]);

const TITLE: Key<String> = sofie_cache::key!("title");
const HOUR: Duration = Duration::from_secs(3600);

async fn started(store: Store) -> (Db, Cache) {
    let db = Db::new(Sqlite::memory().unwrap());
    db.schema().migrate(&[SHELF]).await.unwrap();
    let cache = Cache::start(&db, store).await.unwrap();
    (db, cache)
}

#[tokio::test]
async fn the_sqlite_store_keeps_an_entry_until_it_expires_or_is_deleted() {
    let (_db, cache) = started(Store::Database).await;
    let title = |part: &str| cache.entry(&TITLE).part(part);

    title("short")
        .expires_in(Duration::from_millis(200))
        .set(&"Emma".to_owned())
        .await
        .unwrap();
    title("kept")
        .expires_in(HOUR)
        .set(&"Dune".to_owned())
        .await
        .unwrap();
    title("gone")
        .expires_in(HOUR)
        .set(&"Ulysses".to_owned())
        .await
        .unwrap();
    title("gone").delete().await.unwrap();

    assert_eq!(title("short").get().await.unwrap().as_deref(), Some("Emma"));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(title("short").get().await.unwrap(), None);
    assert_eq!(title("kept").get().await.unwrap().as_deref(), Some("Dune"));
    assert_eq!(title("gone").get().await.unwrap(), None);
}

#[tokio::test]
async fn a_commit_drops_the_sqlite_entries_that_depended_on_it() {
    let (db, cache) = started(Store::Database).await;
    let mut dune = Book {
        meta: Meta::new().unwrap(),
        title: "Dune".to_owned(),
    };
    db.commit(&mut dune).await.unwrap();
    let dune_id = dune.id();
    let title = || cache.entry(&TITLE).part(dune_id);
    title()
        .expires_in(HOUR)
        .depends_on_record(dune_id)
        .set(&"Dune".to_owned())
        .await
        .unwrap();
    cache
        .entry(&TITLE)
        .part("other")
        .expires_in(HOUR)
        .set(&"Emma".to_owned())
        .await
        .unwrap();

    dune.title = "Dune Messiah".to_owned();
    db.commit(&mut dune).await.unwrap();

    assert_eq!(title().get().await.unwrap(), None);
    assert_eq!(
        cache
            .entry(&TITLE)
            .part("other")
            .get()
            .await
            .unwrap()
            .as_deref(),
        Some("Emma")
    );
}

#[tokio::test]
async fn a_cache_started_again_once_the_first_is_dropped_is_the_one_resolved() {
    let (db, first) = started(Store::Memory).await;
    drop(first);
    let second = Cache::start(&db, Store::Memory).await.unwrap();
    second
        .entry(&TITLE)
        .expires_in(HOUR)
        .set(&"Emma".to_owned())
        .await
        .unwrap();

    let resolved = Cache::resolve(&mut TestContext(db.clone())).await.unwrap();

    assert_eq!(
        resolved.entry(&TITLE).get().await.unwrap().as_deref(),
        Some("Emma")
    );
    assert!(Cache::start(&db, Store::Memory).await.is_err());
}

#[tokio::test]
async fn an_expiry_no_store_keeps_fails_before_computing() {
    let (_db, cache) = started(Store::Memory).await;
    let computed = AtomicBool::new(false);

    let refused = cache
        .entry(&TITLE)
        .expires_in(Duration::ZERO)
        .get_or_compute(|| async {
            computed.store(true, Ordering::SeqCst);
            Ok("Emma".to_owned())
        })
        .await;

    assert!(refused.is_err());
    assert!(!computed.load(Ordering::SeqCst));
}
