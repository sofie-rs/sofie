use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use sofie::core::Result;
use sofie::db::Db;
use sofie::sqlite::Sqlite;
use sofie::worker::{Worker, queue_schedule};

static TICKS: AtomicUsize = AtomicUsize::new(0);

async fn tick() -> Result<()> {
    TICKS.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

#[tokio::test]
async fn a_schedule_queued_by_hand_runs_before_it_is_due_and_an_unknown_one_is_named() {
    let db = Db::new(Sqlite::memory().unwrap());
    db.schema().migrate(&[]).await.unwrap();
    let worker = Worker::new(db).every("notes.tick", Duration::from_hours(1), tick);
    assert_eq!(worker.drain().await.unwrap(), 1);
    assert_eq!(worker.drain().await.unwrap(), 0);

    queue_schedule(&worker, "notes.tick").await.unwrap();
    let ran = worker.drain().await.unwrap();
    let unknown = queue_schedule(&worker, "notes.tock").await.unwrap_err();

    assert_eq!(ran, 1);
    assert_eq!(TICKS.load(Ordering::SeqCst), 2);
    assert!(
        unknown
            .to_string()
            .contains("no schedule `notes.tock`; its schedules: notes.tick"),
        "{unknown}"
    );
}
