use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use sofie_core::{Clock, Result, SqlValue, Timestamp, Uuid};
use sofie_db::worker::{Worker, publish_heard};
use sofie_db::{Db, Driver, Meta, Outcome, Plan, Step, query_values};
use sofie_postgres::Postgres;
use sofie_testing::TestDatabase;

static RESTOCKS: AtomicUsize = AtomicUsize::new(0);

/// Raised by a newer version than the worker that sees it first.
struct Restocked;

impl serde::Serialize for Restocked {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_unit()
    }
}

impl sofie_db::Input for Restocked {
    fn from_json(_: serde_json::Value) -> Result<Self> {
        Ok(Self)
    }
}

impl sofie_db::Event for Restocked {
    const NAME: &'static str = "shop.Restocked";
}

async fn restock() -> Result<()> {
    RESTOCKS.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

struct Queue {
    _database: TestDatabase,
    db: Db,
    raw: Postgres,
}

async fn queue() -> Queue {
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    db.schema().migrate(&[]).await.unwrap();
    let raw = Postgres::connect(database.url()).unwrap();
    Queue {
        _database: database,
        db,
        raw,
    }
}

async fn raise_restocked(raw: &Postgres, raised_at: Timestamp) {
    query_values(
        raw,
        "INSERT INTO sofie.messages (id, kind, name, payload, available_at) \
         VALUES ($1, 'event', 'shop.Restocked', '{}', $2)",
        &[
            SqlValue::Uuid(Uuid::now_v7().unwrap()),
            SqlValue::Timestamp(raised_at),
        ],
    )
    .await
    .unwrap();
}

async fn waiting(raw: &Postgres) -> Vec<Vec<SqlValue>> {
    query_values(
        raw,
        "SELECT name, attempts FROM sofie.messages ORDER BY kind",
        &[],
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn an_event_no_listener_hears_waits_unclaimed_for_a_worker_that_hears_it() {
    let queue = queue().await;
    raise_restocked(&queue.raw, Clock::system().now().timestamp()).await;

    Worker::new(queue.db.clone()).drain().await.unwrap();

    assert_eq!(
        waiting(&queue.raw).await,
        [[
            SqlValue::Text("shop.Restocked".to_owned()),
            SqlValue::Int(0)
        ]]
    );
    let newer = Worker::new(queue.db.clone()).on::<Restocked, _>("shop.restock", restock);
    assert_eq!(newer.drain().await.unwrap(), 1);
    assert_eq!(RESTOCKS.load(Ordering::SeqCst), 1);
    assert_eq!(waiting(&queue.raw).await, Vec::<Vec<SqlValue>>::new());
}

#[tokio::test]
async fn an_event_no_listener_heard_for_an_hour_is_dropped() {
    let queue = queue().await;
    let now = Clock::system().now().timestamp();
    let earlier = now.checked_sub(Duration::from_mins(61)).unwrap();
    raise_restocked(&queue.raw, earlier).await;

    Worker::new(queue.db.clone()).drain().await.unwrap();

    assert_eq!(waiting(&queue.raw).await, Vec::<Vec<SqlValue>>::new());
}

struct Raising(Meta);

impl Outcome for Raising {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        plan.raise(self.0.events());
        Ok(())
    }
}

async fn raise(db: &Db) {
    let mut meta = Meta::new().unwrap();
    meta.raise(Restocked).unwrap();
    db.commit(&mut Raising(meta)).await.unwrap();
}

#[tokio::test]
async fn a_commit_drops_an_event_no_published_worker_hears_and_fans_out_one_it_hears() {
    let queue = queue().await;
    publish_heard(&Worker::new(queue.db.clone()));

    raise(&queue.db).await;

    assert_eq!(waiting(&queue.raw).await, Vec::<Vec<SqlValue>>::new());
    publish_heard(&Worker::new(queue.db.clone()).on::<Restocked, _>("shop.restock", restock));
    raise(&queue.db).await;
    assert_eq!(
        waiting(&queue.raw).await,
        [[SqlValue::Text("shop.restock".to_owned()), SqlValue::Int(0)]]
    );
}

const RECORD_EVENT_ROWS: &str = "
CREATE TABLE event_rows (name text NOT NULL);
CREATE FUNCTION record_event_row() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO event_rows VALUES (NEW.name);
    RETURN NEW;
END $$;
CREATE TRIGGER record_event_row AFTER INSERT ON sofie.messages
    FOR EACH ROW WHEN (NEW.kind = 'event') EXECUTE FUNCTION record_event_row();
";

#[tokio::test]
async fn a_published_commit_writes_its_listener_jobs_and_no_event_row() {
    let queue = queue().await;
    let record = Step::Script(RECORD_EVENT_ROWS.to_owned());
    queue.raw.transaction(vec![record]).await.unwrap();
    publish_heard(&Worker::new(queue.db.clone()).on::<Restocked, _>("shop.restock", ignore));

    raise(&queue.db).await;

    let event_rows = query_values(&queue.raw, "SELECT count(*) FROM event_rows", &[])
        .await
        .unwrap();
    assert_eq!(event_rows, [[SqlValue::Int(0)]]);
    assert_eq!(
        waiting(&queue.raw).await,
        [[SqlValue::Text("shop.restock".to_owned()), SqlValue::Int(0)]]
    );
}

#[tokio::test]
async fn a_queued_event_fans_out_to_every_listener_its_process_heard() {
    let queue = queue().await;
    raise_restocked(&queue.raw, Clock::system().now().timestamp()).await;
    let first = Worker::new(queue.db.clone()).on::<Restocked, _>("shop.restock", ignore);
    let second = Worker::new(queue.db.clone()).on::<Restocked, _>("shop.announce_restock", ignore);
    publish_heard(&second);

    assert_eq!(first.drain().await.unwrap(), 1);

    assert_eq!(
        waiting(&queue.raw).await,
        [[
            SqlValue::Text("shop.announce_restock".to_owned()),
            SqlValue::Int(0)
        ]]
    );
}

#[tokio::test]
async fn a_commit_while_a_worker_is_being_built_drops_nothing() {
    let queue = queue().await;
    let building = Worker::new(queue.db.clone());

    raise(&queue.db).await;

    let _built = building.on::<Restocked, _>("shop.restock", restock);
    assert_eq!(waiting(&queue.raw).await, [restocked_waiting()]);
}

async fn ignore() -> Result<()> {
    Ok(())
}

#[tokio::test]
async fn a_listener_only_the_raising_version_has_waits_for_a_worker_that_knows_it() {
    let queue = queue().await;
    let newer = Worker::new(queue.db.clone())
        .on::<Restocked, _>("shop.restock", ignore)
        .on::<Restocked, _>("shop.announce_restock", ignore);
    publish_heard(&newer);
    raise(&queue.db).await;
    let older = Worker::new(queue.db.clone()).on::<Restocked, _>("shop.restock", ignore);

    assert_eq!(older.drain().await.unwrap(), 1);

    assert_eq!(
        waiting(&queue.raw).await,
        [[
            SqlValue::Text("shop.announce_restock".to_owned()),
            SqlValue::Int(0)
        ]]
    );
    assert_eq!(newer.drain().await.unwrap(), 1);
    assert_eq!(waiting(&queue.raw).await, Vec::<Vec<SqlValue>>::new());
}

#[tokio::test]
async fn two_workers_of_one_process_hearing_one_event_each_run_their_listener() {
    let queue = queue().await;
    let first = Worker::new(queue.db.clone()).on::<Restocked, _>("shop.restock", ignore);
    let second = Worker::new(queue.db.clone()).on::<Restocked, _>("shop.announce_restock", ignore);
    publish_heard(&first);
    publish_heard(&second);

    raise(&queue.db).await;

    assert_eq!(first.drain().await.unwrap(), 1);
    assert_eq!(second.drain().await.unwrap(), 1);
    assert_eq!(waiting(&queue.raw).await, Vec::<Vec<SqlValue>>::new());
}

#[tokio::test]
async fn a_job_another_worker_of_its_process_handles_waits_the_same_hour() {
    let queue = queue().await;
    let hearing =
        Worker::new(queue.db.clone()).every("shop.reprice", Duration::from_hours(1), ignore);
    publish_heard(&hearing);
    let now = Clock::system().now().timestamp();
    queue_job(&queue.raw, "shop.reprice", now).await;
    queue_job(
        &queue.raw,
        "shop.reprice",
        now.checked_sub(Duration::from_mins(61)).unwrap(),
    )
    .await;

    Worker::new(queue.db.clone()).drain().await.unwrap();

    let left = query_values(
        &queue.raw,
        "SELECT attempts, dead_at IS NOT NULL FROM sofie.messages ORDER BY available_at",
        &[],
    )
    .await
    .unwrap();
    assert_eq!(
        left,
        [
            [SqlValue::Int(1), SqlValue::Bool(true)],
            [SqlValue::Int(0), SqlValue::Bool(false)]
        ]
    );
}

async fn queue_job(raw: &Postgres, name: &str, queued_at: Timestamp) {
    query_values(
        raw,
        "INSERT INTO sofie.messages (id, kind, name, payload, available_at) \
         VALUES ($1, 'job', $2, '{}', $3)",
        &[
            SqlValue::Uuid(Uuid::now_v7().unwrap()),
            SqlValue::Text(name.to_owned()),
            SqlValue::Timestamp(queued_at),
        ],
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn a_job_no_worker_knows_waits_an_hour_then_dies_at_once() {
    let queue = queue().await;
    let now = Clock::system().now().timestamp();
    queue_job(&queue.raw, "shop.reprice", now).await;
    queue_job(
        &queue.raw,
        "shop.recount",
        now.checked_sub(Duration::from_mins(61)).unwrap(),
    )
    .await;

    Worker::new(queue.db.clone()).drain().await.unwrap();

    let left = query_values(
        &queue.raw,
        "SELECT name, attempts, dead_at IS NOT NULL FROM sofie.messages ORDER BY name",
        &[],
    )
    .await
    .unwrap();
    assert_eq!(
        left,
        [
            [
                SqlValue::Text("shop.recount".to_owned()),
                SqlValue::Int(1),
                SqlValue::Bool(true)
            ],
            [
                SqlValue::Text("shop.reprice".to_owned()),
                SqlValue::Int(0),
                SqlValue::Bool(false)
            ]
        ]
    );
}

fn restocked_waiting() -> [SqlValue; 2] {
    [
        SqlValue::Text("shop.Restocked".to_owned()),
        SqlValue::Int(0),
    ]
}
