use sofie_db::worker::{Feeds, Worker};
use sofie_db::{Db, ProductEvents};
use sofie_sqlite::Sqlite;

async fn migrated() -> Db {
    let db = Db::new(Sqlite::memory().unwrap());
    db.schema().migrate(&[]).await.unwrap();
    db
}

#[tokio::test]
async fn a_worker_keeping_an_event_feed_is_refused_before_any_work() {
    let db = migrated().await;
    let worker = Feeds::default()
        .publish("members.MemberChanged")
        .attach(Worker::new(db));

    let refused = worker.drain().await.unwrap_err().to_string();

    assert!(refused.contains("needs Postgres"), "{refused}");
}

#[tokio::test]
async fn keeping_product_events_is_refused_at_start() {
    let db = migrated().await;

    let refused = ProductEvents::keep(&db).err().unwrap().to_string();

    assert!(refused.contains("needs Postgres"), "{refused}");
}
