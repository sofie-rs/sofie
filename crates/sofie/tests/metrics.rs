#![cfg(feature = "metrics")]

use std::time::Duration;

use sofie::core::Result;
use sofie::db::Db;
use sofie::postgres::Postgres;
use sofie::worker::Worker;
use sofie_testing::TestDatabase;
use tokio::sync::Mutex;

/// The gauges are process-wide, so one test's connections and outbox would show in the other's.
static GAUGES: Mutex<()> = Mutex::const_new(());

async fn tick() -> Result<()> {
    Ok(())
}

#[tokio::test]
async fn statements_connections_and_job_outcomes_are_measured() {
    let _gauges = GAUGES.lock().await;
    sofie::telemetry::metrics::start().unwrap();
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    db.schema().migrate(&[]).await.unwrap();
    let worker = Worker::new(db).every("shop.jobs.tick", Duration::from_hours(1), tick);

    worker.drain().await.unwrap();
    let text = sofie::telemetry::metrics::prometheus().await.unwrap();

    for sample in [
        "messaging_process_outcomes_total{messaging_work=\"job\",\
         messaging_name=\"shop.jobs.tick\",outcome=\"completed\"} 1\n",
        "messaging_process_lag_seconds_count{messaging_work=\"job\"} 1\n",
        "db_client_connection_count 0\n",
    ] {
        assert!(text.contains(sample), "{sample} in {text}");
    }
    assert!(
        text.contains("db_client_operation_duration_seconds_count{db_operation_name=\"SELECT\"}"),
        "{text}"
    );
}

#[tokio::test]
async fn the_outbox_depth_is_polled_while_the_worker_runs() {
    let _gauges = GAUGES.lock().await;
    sofie::telemetry::metrics::start().unwrap();
    let database = TestDatabase::create().await;
    let postgres = Postgres::connect(database.url()).unwrap();
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    db.schema().migrate(&[]).await.unwrap();
    sofie::db::query_values(
        &postgres,
        "INSERT INTO sofie.messages (id, kind, name, payload, available_at)
             VALUES (gen_random_uuid(), 'job', 'shop.jobs.later', '{}', now() + interval '1 day')",
        &[],
    )
    .await
    .unwrap();

    Worker::new(db)
        .run(tokio::time::sleep(Duration::from_millis(500)))
        .await
        .unwrap();
    let text = sofie::telemetry::metrics::prometheus().await.unwrap();

    assert!(text.contains("messaging_outbox_depth 1\n"), "{text}");
}
