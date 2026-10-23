#![cfg(feature = "retention")]

use std::time::Duration;

use sofie::db::{Db, File, Migrations, query_values};
use sofie::postgres::Postgres;
use sofie::prelude::*;
use sofie::worker::Worker;
use sofie_testing::TestDatabase;

const MIGRATIONS: &[File] = &[File::new(
    "0001_visit.sql",
    b"CREATE TABLE visit (
        id uuid PRIMARY KEY,
        version bigint NOT NULL,
        created_at timestamptz NOT NULL,
        updated_at timestamptz NOT NULL,
        page text NOT NULL
    );",
)];

#[derive(Aggregate)]
pub struct Visit {
    meta: Meta,
    page: String,
}

async fn forget_old_visits(prune: Prune, clock: Clock) -> Result<()> {
    let before = clock.now().checked_sub(Duration::from_hours(24 * 30))?;
    prune
        .before::<Visit, _>(Visit::CREATED_AT, before)
        .await
        .map(drop)
}

#[tokio::test]
async fn a_scheduled_job_deletes_records_past_their_retention_in_batches() {
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    db.schema()
        .migrate(&[Migrations::new("retention", MIGRATIONS)])
        .await
        .unwrap();
    let raw = Postgres::connect(database.url()).unwrap();
    query_values(
        &raw,
        "INSERT INTO retention.visit
         SELECT gen_random_uuid(), 1, now() - interval '40 days', now(), 'old'
         FROM generate_series(1, 2500)",
        &[],
    )
    .await
    .unwrap();
    let mut recent = Visit {
        meta: Meta::new().unwrap(),
        page: "new".to_owned(),
    };
    db.commit(&mut recent).await.unwrap();

    let ran = Worker::new(db.clone())
        .every(
            "retention.forget_old_visits",
            Duration::from_hours(1),
            forget_old_visits,
        )
        .drain()
        .await
        .unwrap();

    assert_eq!(ran, 1);

    let left = query_values(&raw, "SELECT page FROM retention.visit", &[])
        .await
        .unwrap();
    assert_eq!(left, [vec![sofie::core::SqlValue::Text("new".to_owned())]]);
}

// What `sofie gen` writes into the module's contract.
#[derive(Event)]
pub struct VisitPruned {
    pub id: sofie::core::Uuid,
    pub version: i64,
}

static PRUNED_HEARD: std::sync::Mutex<Vec<sofie::core::Uuid>> = std::sync::Mutex::new(Vec::new());

async fn hear_pruned(pruned: VisitPruned) -> Result<()> {
    PRUNED_HEARD.lock().unwrap().push(pruned.id);
    Ok(())
}

#[tokio::test]
async fn a_listener_hears_each_pruned_record_as_its_own_event() {
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    db.schema()
        .migrate(&[Migrations::new("retention", MIGRATIONS)])
        .await
        .unwrap();
    let raw = Postgres::connect(database.url()).unwrap();
    let old = query_values(
        &raw,
        "INSERT INTO retention.visit
         SELECT gen_random_uuid(), 1, now() - interval '40 days', now(), 'old'
         FROM generate_series(1, 2)
         RETURNING id",
        &[],
    )
    .await
    .unwrap();

    Worker::new(db.clone())
        .every(
            "retention.forget_old_visits",
            Duration::from_hours(1),
            forget_old_visits,
        )
        .on::<VisitPruned, _>("retention.hear_pruned", hear_pruned)
        .drain()
        .await
        .unwrap();

    let mut heard: Vec<sofie::core::SqlValue> = PRUNED_HEARD
        .lock()
        .unwrap()
        .iter()
        .map(|id| sofie::core::SqlValue::Uuid(*id))
        .collect();
    let mut pruned: Vec<sofie::core::SqlValue> = old.into_iter().flatten().collect();
    heard.sort_by_key(|value| format!("{value:?}"));
    pruned.sort_by_key(|value| format!("{value:?}"));
    assert_eq!(heard, pruned);
}

async fn ignore_pruned(_pruned: VisitPruned) -> Result<()> {
    Ok(())
}

/// How many pruned records a listener that only another worker of the process registered ran for.
async fn heard_by_another_worker(db: &Db) -> usize {
    let hearing = Worker::new(db.clone())
        .on::<VisitPruned, _>("retention.hear_pruned_elsewhere", ignore_pruned);
    sofie::worker::publish_heard(&hearing);
    Worker::new(db.clone())
        .every(
            "retention.forget_old_visits",
            Duration::from_hours(1),
            forget_old_visits,
        )
        .drain()
        .await
        .unwrap();
    hearing.drain().await.unwrap()
}

#[tokio::test]
async fn a_pruned_record_is_announced_to_a_listener_another_worker_of_the_process_has() {
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    db.schema()
        .migrate(&[Migrations::new("retention", MIGRATIONS)])
        .await
        .unwrap();
    let raw = Postgres::connect(database.url()).unwrap();
    query_values(
        &raw,
        "INSERT INTO retention.visit
         VALUES (gen_random_uuid(), 1, now() - interval '40 days', now(), 'old')",
        &[],
    )
    .await
    .unwrap();

    assert_eq!(heard_by_another_worker(&db).await, 1);
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn a_pruned_record_is_announced_to_a_listener_another_worker_has_on_sqlite() {
    use sofie::sqlite::Sqlite;

    const SQLITE_MIGRATIONS: &[File] = &[File::new(
        "0001_visit.sql",
        b"CREATE TABLE retention_visit (
            id uuid PRIMARY KEY,
            version bigint NOT NULL,
            created_at timestamptz NOT NULL,
            updated_at timestamptz NOT NULL,
            page text NOT NULL
        );",
    )];
    let path = std::env::temp_dir().join(format!("sofie-retention-{}.db", std::process::id()));
    let db = Db::new(Sqlite::open(&path).unwrap());
    db.schema()
        .migrate(&[Migrations::new("retention", MIGRATIONS).sqlite(SQLITE_MIGRATIONS)])
        .await
        .unwrap();
    query_values(
        &Sqlite::open(&path).unwrap(),
        "INSERT INTO retention_visit VALUES ('0190a7d2-0000-7000-8000-000000000001', 1,
            CAST((unixepoch('subsec') - 40 * 86400) * 1000000 AS INTEGER), 0, 'old')",
        &[],
    )
    .await
    .unwrap();

    let heard = heard_by_another_worker(&db).await;

    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    assert_eq!(heard, 1);
}
