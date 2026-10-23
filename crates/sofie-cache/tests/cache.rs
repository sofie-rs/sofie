//! Redis is tested against a fake server inside the crate.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use common::{Book, TestContext};
use serde::{Deserialize, Serialize};
use sofie_cache::{Cache, Key, MAX_EXPIRY, Store};
use sofie_db::{Aggregate, Db, File, Id, Meta, Migrations, Resolve, query_values};
use sofie_postgres::Postgres;
use sofie_testing::TestDatabase;

const SHELF: Migrations = Migrations::new(
    "shelf",
    &[File::new(
        "0001_books.sql",
        b"CREATE TABLE books (id uuid PRIMARY KEY, version bigint NOT NULL, title text NOT NULL);",
    )],
);

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Shelf {
    titles: Vec<String>,
}

const SHELF_KEY: Key<Shelf> = sofie_cache::key!("shelf");
const TITLE: Key<String> = sofie_cache::key!("title");
const HOUR: Duration = Duration::from_secs(3600);

async fn migrated(database: &TestDatabase) -> Db {
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    db.schema().migrate(&[SHELF]).await.unwrap();
    db
}

async fn book(db: &Db, title: &str) -> Book {
    let mut book = Book {
        meta: Meta::new().unwrap(),
        title: title.to_owned(),
    };
    db.commit(&mut book).await.unwrap();
    book
}

async fn rename(db: &Db, book: &mut Book, title: &str) {
    title.clone_into(&mut book.title);
    db.commit(book).await.unwrap();
}

fn shelf(titles: &[&str]) -> Shelf {
    Shelf {
        titles: titles.iter().map(ToString::to_string).collect(),
    }
}

async fn eventually(mut done: impl AsyncFnMut() -> bool) {
    for _ in 0..100 {
        if done().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the condition never held");
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_callers_compute_a_missing_value_once() {
    let database = TestDatabase::create().await;
    let db = migrated(&database).await;
    let cache = Cache::start(&db, Store::Memory).await.unwrap();
    let computed = Arc::new(AtomicUsize::new(0));

    let callers: Vec<_> = (0..8)
        .map(|_| {
            let (cache, computed) = (cache.clone(), Arc::clone(&computed));
            tokio::spawn(async move {
                let entry = cache.entry(&TITLE).part(1).expires_in(HOUR);
                entry
                    .get_or_compute(|| async {
                        computed.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        Ok("Dune".to_owned())
                    })
                    .await
                    .unwrap()
            })
        })
        .collect();
    for caller in callers {
        assert_eq!(caller.await.unwrap(), "Dune");
    }

    assert_eq!(computed.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_computation_is_not_cached() {
    let database = TestDatabase::create().await;
    let db = migrated(&database).await;
    let cache = Cache::start(&db, Store::Memory).await.unwrap();
    let entry = cache.entry(&TITLE).expires_in(HOUR);

    let failed = entry
        .get_or_compute(|| async { Err(sofie_core::Error::Conflict) })
        .await;

    assert!(failed.is_err());
    assert_eq!(cache.entry(&TITLE).get().await.unwrap(), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn every_entry_needs_an_expiry_a_cache_keeps() {
    let database = TestDatabase::create().await;
    let db = migrated(&database).await;
    let cache = Cache::start(&db, Store::Memory).await.unwrap();

    let title = "x".to_owned();
    for refused in [Duration::ZERO, MAX_EXPIRY + Duration::from_secs(1)] {
        let entry = cache.entry(&TITLE).expires_in(refused);
        assert!(entry.set(&title).await.is_err(), "{refused:?}");
    }
    cache
        .entry(&TITLE)
        .expires_in(MAX_EXPIRY)
        .set(&title)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_handler_resolves_the_cache_its_database_started() {
    let database = TestDatabase::create().await;
    let db = migrated(&database).await;
    assert!(Cache::resolve(&mut TestContext(db.clone())).await.is_err());
    let cache = Cache::start(&db, Store::Memory).await.unwrap();
    cache
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

async fn a_commit_drops_what_depended_on_it(store: Store) {
    let database = TestDatabase::create().await;
    let db = migrated(&database).await;
    let mut dune = book(&db, "Dune").await;
    let emma = book(&db, "Emma").await;
    let cache = Cache::start(&db, store).await.unwrap();
    let all = cache
        .entry(&SHELF_KEY)
        .expires_in(HOUR)
        .depends_on::<Book>();
    all.set(&shelf(&["Dune", "Emma"])).await.unwrap();
    let title_of = |id: Id<Book>| cache.entry(&TITLE).part(id);
    let dune_id = dune.id();
    title_of(dune_id)
        .expires_in(HOUR)
        .depends_on_record(dune_id)
        .set(&"Dune".to_owned())
        .await
        .unwrap();
    title_of(emma.id())
        .expires_in(HOUR)
        .depends_on_record(emma.id())
        .set(&"Emma".to_owned())
        .await
        .unwrap();

    rename(&db, &mut dune, "Dune Messiah").await;

    // A shared store hears of the commit off the request's path, so it may lag a moment.
    eventually(async || {
        cache.entry(&SHELF_KEY).get().await.unwrap().is_none()
            && title_of(dune_id).get().await.unwrap().is_none()
    })
    .await;
    assert_eq!(
        title_of(emma.id()).get().await.unwrap().as_deref(),
        Some("Emma")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_commit_drops_memory_entries_that_depended_on_it() {
    a_commit_drops_what_depended_on_it(Store::Memory).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_commit_drops_database_entries_that_depended_on_it() {
    a_commit_drops_what_depended_on_it(Store::Database).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_database_store_is_shared_and_honours_expiry() {
    let database = TestDatabase::create().await;
    let first_db = migrated(&database).await;
    let second_db = Db::new(Postgres::connect(database.url()).unwrap());
    let first = Cache::start(&first_db, Store::Database).await.unwrap();
    let second = Cache::start(&second_db, Store::Database).await.unwrap();

    first
        .entry(&TITLE)
        .expires_in(Duration::from_millis(300))
        .set(&"Emma".to_owned())
        .await
        .unwrap();

    assert_eq!(
        second.entry(&TITLE).get().await.unwrap().as_deref(),
        Some("Emma")
    );
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(first.entry(&TITLE).get().await.unwrap(), None);
    assert_eq!(second.entry(&TITLE).get().await.unwrap(), None);
}

/// Another instance holds its own copy in memory; the database's notification drops it.
async fn another_instance_s_commit_reaches_this_memory(store: fn() -> Store) {
    let database = TestDatabase::create().await;
    let first_db = migrated(&database).await;
    let second_db = Db::new(Postgres::connect(database.url()).unwrap());
    let mut dune = book(&first_db, "Dune").await;
    let first = Cache::start(&first_db, store()).await.unwrap();
    let _second = Cache::start(&second_db, store()).await.unwrap();
    let all = first
        .entry(&SHELF_KEY)
        .expires_in(HOUR)
        .depends_on::<Book>();
    all.set(&shelf(&["Dune"])).await.unwrap();

    rename(&second_db, &mut dune, "Dune Messiah").await;

    eventually(async || first.entry(&SHELF_KEY).get().await.unwrap().is_none()).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn another_instance_s_commit_drops_this_memory_store_s_entries() {
    another_instance_s_commit_reaches_this_memory(|| Store::Memory).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn another_instance_s_commit_drops_this_database_store_s_memory_copy() {
    another_instance_s_commit_reaches_this_memory(|| Store::Database).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_value_set_right_after_start_outlives_the_start_of_listening() {
    let database = TestDatabase::create().await;
    let first_db = migrated(&database).await;
    let second_db = Db::new(Postgres::connect(database.url()).unwrap());
    let first = Cache::start(&first_db, Store::Memory).await.unwrap();
    let _second = Cache::start(&second_db, Store::Memory).await.unwrap();
    let title = first.entry(&TITLE).expires_in(HOUR);
    title.set(&"Emma".to_owned()).await.unwrap();
    let all = first
        .entry(&SHELF_KEY)
        .expires_in(HOUR)
        .depends_on::<Book>();
    all.set(&shelf(&[])).await.unwrap();

    book(&second_db, "Dune").await;

    eventually(async || first.entry(&SHELF_KEY).get().await.unwrap().is_none()).await;
    assert_eq!(
        first.entry(&TITLE).get().await.unwrap().as_deref(),
        Some("Emma")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_value_computed_while_its_dependency_changed_is_not_kept() {
    let database = TestDatabase::create().await;
    let db = migrated(&database).await;
    let cache = Cache::start(&db, Store::Memory).await.unwrap();
    let mut dune = book(&db, "Dune").await;

    let stale = cache
        .entry(&SHELF_KEY)
        .expires_in(HOUR)
        .depends_on::<Book>()
        .get_or_compute(|| async {
            let read_before_the_commit = shelf(&["Dune"]);
            rename(&db, &mut dune, "Dune Messiah").await;
            Ok(read_before_the_commit)
        })
        .await
        .unwrap();

    assert_eq!(stale, shelf(&["Dune"]));
    assert_eq!(cache.entry(&SHELF_KEY).get().await.unwrap(), None);
}

/// A leader whose computation commits a book, then three waiters of which the first
/// `dropping_waiters` commit one too; answers how many waiters computed, and most at once.
async fn waiting_through_drops(dropping_waiters: usize) -> (usize, usize) {
    let database = TestDatabase::create().await;
    let db = migrated(&database).await;
    let cache = Cache::start(&db, Store::Memory).await.unwrap();
    let computing = |db: Db, drops: bool| async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if drops {
            book(&db, "Dune").await;
        }
        Ok(shelf(&["Dune"]))
    };
    let leading = {
        let (cache, db) = (cache.clone(), db.clone());
        tokio::spawn(async move {
            let entry = cache.entry(&SHELF_KEY).expires_in(HOUR);
            let entry = entry.depends_on::<Book>();
            entry.get_or_compute(|| computing(db, true)).await.unwrap();
        })
    };
    tokio::time::sleep(Duration::from_millis(20)).await;
    let (computed, inside, most) = (
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(0)),
    );
    let waiters: Vec<_> = (0..3)
        .map(|_| {
            let (cache, db) = (cache.clone(), db.clone());
            let (computed, inside, most) = (
                Arc::clone(&computed),
                Arc::clone(&inside),
                Arc::clone(&most),
            );
            tokio::spawn(async move {
                let entry = cache.entry(&SHELF_KEY).expires_in(HOUR);
                let entry = entry.depends_on::<Book>();
                entry
                    .get_or_compute(|| async {
                        let order = computed.fetch_add(1, Ordering::SeqCst);
                        most.fetch_max(inside.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
                        let value = computing(db, order < dropping_waiters).await;
                        inside.fetch_sub(1, Ordering::SeqCst);
                        value
                    })
                    .await
                    .unwrap()
            })
        })
        .collect();
    leading.await.unwrap();
    for waiter in waiters {
        waiter.await.unwrap();
    }
    (computed.load(Ordering::SeqCst), most.load(Ordering::SeqCst))
}

#[tokio::test(flavor = "multi_thread")]
async fn one_waiter_computes_again_for_the_rest_when_the_leader_s_value_was_dropped() {
    assert_eq!(waiting_through_drops(0).await, (1, 1));
}

#[tokio::test(flavor = "multi_thread")]
async fn waiters_leave_rather_than_compute_in_turn_when_a_second_value_was_dropped() {
    let (computed, most) = waiting_through_drops(1).await;

    assert_eq!(computed, 3);
    assert!(most > 1, "the waiters computed one after another");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_value_set_while_another_was_computed_is_not_overwritten_by_it() {
    let database = TestDatabase::create().await;
    let db = migrated(&database).await;
    let cache = Cache::start(&db, Store::Memory).await.unwrap();

    cache
        .entry(&TITLE)
        .expires_in(HOUR)
        .get_or_compute(|| async {
            let read_before_the_set = "Dune".to_owned();
            let entry = cache.entry(&TITLE).expires_in(HOUR);
            entry.set(&"Dune Messiah".to_owned()).await.unwrap();
            Ok(read_before_the_set)
        })
        .await
        .unwrap();

    let held = cache.entry(&TITLE).get().await.unwrap();
    assert_eq!(held.as_deref(), Some("Dune Messiah"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_value_whose_dependency_changed_while_it_was_written_is_taken_back_out() {
    let database = TestDatabase::create().await;
    let db = migrated(&database).await;
    let mut dune = book(&db, "Dune").await;
    let cache = Cache::start(&db, Store::Database).await.unwrap();
    let raw = Postgres::connect(database.url()).unwrap();
    let slow_writes = [
        "CREATE FUNCTION sofie.slow_cache() RETURNS trigger AS $$ \
         BEGIN PERFORM pg_sleep(0.4); RETURN NEW; END $$ LANGUAGE plpgsql",
        "CREATE TRIGGER slow BEFORE INSERT ON sofie.cache \
         FOR EACH ROW EXECUTE FUNCTION sofie.slow_cache()",
    ];
    for statement in slow_writes {
        query_values(&raw, statement, &[]).await.unwrap();
    }
    let renamer = db.clone();
    let renaming = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        rename(&renamer, &mut dune, "Dune Messiah").await;
    });

    cache
        .entry(&SHELF_KEY)
        .expires_in(HOUR)
        .depends_on::<Book>()
        .get_or_compute(|| async { Ok(shelf(&["Dune"])) })
        .await
        .unwrap();
    renaming.await.unwrap();

    let held = query_values(&raw, "SELECT count(*) FROM sofie.cache", &[])
        .await
        .unwrap();
    assert_eq!(held[0][0], sofie_core::SqlValue::Int(0));
    assert_eq!(cache.entry(&SHELF_KEY).get().await.unwrap(), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lookup_racing_a_delete_does_not_keep_the_deleted_value_in_memory() {
    let database = TestDatabase::create().await;
    let db = migrated(&database).await;
    let cache = Cache::start(&db, Store::Database).await.unwrap();
    let title = cache.entry(&TITLE).expires_in(HOUR);
    title.set(&"Dune".to_owned()).await.unwrap();
    let raw = Postgres::connect(database.url()).unwrap();
    let slow_deletes = [
        "CREATE FUNCTION sofie.slow_delete() RETURNS trigger AS $$ \
         BEGIN PERFORM pg_sleep(0.4); RETURN OLD; END $$ LANGUAGE plpgsql",
        "CREATE TRIGGER slow BEFORE DELETE ON sofie.cache \
         FOR EACH ROW EXECUTE FUNCTION sofie.slow_delete()",
    ];
    for statement in slow_deletes {
        query_values(&raw, statement, &[]).await.unwrap();
    }
    let deleter = cache.clone();
    let deleting = tokio::spawn(async move { deleter.entry(&TITLE).delete().await.unwrap() });
    tokio::time::sleep(Duration::from_millis(100)).await;

    let read_while_deleting = cache.entry(&TITLE).get().await.unwrap();
    deleting.await.unwrap();

    assert_eq!(read_while_deleting.as_deref(), Some("Dune"));
    assert_eq!(cache.entry(&TITLE).get().await.unwrap(), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_drop_the_shared_store_failed_still_reaches_other_instances_and_is_retried() {
    let database = TestDatabase::create().await;
    let first_db = migrated(&database).await;
    let second_db = Db::new(Postgres::connect(database.url()).unwrap());
    let mut dune = book(&first_db, "Dune").await;
    let first = Cache::start(&first_db, Store::Database).await.unwrap();
    let second = Cache::start(&second_db, Store::Database).await.unwrap();
    let all = first
        .entry(&SHELF_KEY)
        .expires_in(HOUR)
        .depends_on::<Book>();
    all.set(&shelf(&["Dune"])).await.unwrap();
    assert!(second.entry(&SHELF_KEY).get().await.unwrap().is_some());
    let raw = Postgres::connect(database.url()).unwrap();
    let failing_once = [
        "CREATE SEQUENCE sofie.drop_tries",
        "CREATE FUNCTION sofie.failing_drop() RETURNS trigger AS $$ BEGIN \
         IF nextval('sofie.drop_tries') = 1 THEN RAISE EXCEPTION 'the cache is down'; END IF; \
         RETURN OLD; END $$ LANGUAGE plpgsql",
        "CREATE TRIGGER failing BEFORE DELETE ON sofie.cache \
         FOR EACH ROW EXECUTE FUNCTION sofie.failing_drop()",
    ];
    for statement in failing_once {
        query_values(&raw, statement, &[]).await.unwrap();
    }

    rename(&first_db, &mut dune, "Dune Messiah").await;

    eventually(async || {
        let held = query_values(&raw, "SELECT count(*) FROM sofie.cache", &[])
            .await
            .unwrap();
        held[0][0] == sofie_core::SqlValue::Int(0)
            && second.entry(&SHELF_KEY).get().await.unwrap().is_none()
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_commit_does_not_wait_for_the_shared_store_yet_reads_its_own_write() {
    let database = TestDatabase::create().await;
    let db = migrated(&database).await;
    let mut dune = book(&db, "Dune").await;
    let cache = Cache::start(&db, Store::Database).await.unwrap();
    let all = cache
        .entry(&SHELF_KEY)
        .expires_in(HOUR)
        .depends_on::<Book>();
    all.set(&shelf(&["Dune"])).await.unwrap();
    let raw = Postgres::connect(database.url()).unwrap();
    let slow_drops = [
        "CREATE FUNCTION sofie.slow_drop() RETURNS trigger AS $$ \
         BEGIN PERFORM pg_sleep(1); RETURN OLD; END $$ LANGUAGE plpgsql",
        "CREATE TRIGGER slow BEFORE DELETE ON sofie.cache \
         FOR EACH ROW EXECUTE FUNCTION sofie.slow_drop()",
    ];
    for statement in slow_drops {
        query_values(&raw, statement, &[]).await.unwrap();
    }

    let started = std::time::Instant::now();
    rename(&db, &mut dune, "Dune Messiah").await;

    assert!(
        started.elapsed() < Duration::from_millis(500),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(cache.entry(&SHELF_KEY).get().await.unwrap(), None);
    eventually(async || {
        let held = query_values(&raw, "SELECT count(*) FROM sofie.cache", &[])
            .await
            .unwrap();
        held[0][0] == sofie_core::SqlValue::Int(0)
    })
    .await;
}

#[tokio::test]
async fn an_overlong_key_or_a_sub_millisecond_expiry_is_refused() {
    let database = TestDatabase::create().await;
    let db = migrated(&database).await;
    let cache = Cache::start(&db, Store::Memory).await.unwrap();

    let overlong = cache.entry(&TITLE).part("x".repeat(600));
    let error = overlong.get().await.unwrap_err();
    let brief = cache
        .entry(&TITLE)
        .expires_in(Duration::from_micros(500))
        .set(&"t".to_owned())
        .await
        .unwrap_err();

    assert!(error.to_string().contains(":title`"), "{error}");
    assert!(!error.to_string().contains("xxxx"), "{error}");
    assert!(brief.to_string().contains("expires in"), "{brief}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cache_hit_keys_the_answer_s_fingerprint_by_the_value_it_gave() {
    let database = TestDatabase::create().await;
    let db = migrated(&database).await;
    let cache = Cache::start(&db, Store::Memory).await.unwrap();
    let hit = async |title: &str| {
        let entry = cache.entry(&TITLE).expires_in(HOUR);
        entry.set(&title.to_owned()).await.unwrap();
        let read = entry.get_or_compute(|| async { Ok(String::new()) });
        sofie_db::track_reads(1, read).await.1
    };

    let dune = hit("Dune").await;

    assert!(dune.is_some());
    assert_eq!(dune, hit("Dune").await);
    assert_ne!(dune, hit("Emma").await);
}
