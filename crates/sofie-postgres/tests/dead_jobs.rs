use sofie_core::{Clock, Error, Result, SqlValue, Uuid};
use sofie_db::worker::Worker;
use sofie_db::{Db, DeadMessages, query_values};
use sofie_postgres::Postgres;
use sofie_testing::TestDatabase;

async fn refuse() -> Result<()> {
    Err(Error::Rule("the carrier refused\nsecond line".to_owned()))
}

async fn ship() -> Result<()> {
    Ok(())
}

async fn queue_job(raw: &Postgres) -> Uuid {
    let id = Uuid::now_v7().unwrap();
    query_values(
        raw,
        "INSERT INTO sofie.messages (id, kind, name, payload, available_at) \
         VALUES ($1, 'job', 'shop.ship', '{}', $2)",
        &[
            SqlValue::Uuid(id),
            SqlValue::Timestamp(Clock::system().now().timestamp()),
        ],
    )
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn a_dead_job_is_listed_and_replayed_once_and_a_live_or_missing_one_is_refused() {
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    db.schema().migrate(&[]).await.unwrap();
    let raw = Postgres::connect(database.url()).unwrap();
    let messages = DeadMessages::new(db.clone());
    let dead = queue_job(&raw).await;
    Worker::new(db.clone())
        .job("shop.ship", refuse)
        .drain()
        .await
        .unwrap();
    let live = queue_job(&raw).await;

    let listed = messages.newest(10).await.unwrap();
    let replayed = messages.replay(dead).await.unwrap();
    let again = messages.replay(dead).await.unwrap_err();
    let waiting = messages.replay(live).await.unwrap_err();
    let missing = messages.replay(Uuid::from_u128(7)).await.unwrap_err();
    let handled = Worker::new(db.clone())
        .job("shop.ship", ship)
        .drain()
        .await
        .unwrap();

    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, dead);
    assert_eq!(listed[0].name, "shop.ship");
    assert_eq!(listed[0].attempts, 1);
    assert_eq!(
        listed[0].last_error.as_deref(),
        Some("refused: the carrier refused\nsecond line")
    );
    assert_eq!(replayed, listed[0]);
    assert!(again.to_string().contains("is not dead"), "{again}");
    assert!(waiting.to_string().contains("is not dead"), "{waiting}");
    assert!(
        missing.to_string().contains("there is no message"),
        "{missing}"
    );
    assert_eq!(handled, 2);
    assert_eq!(messages.newest(10).await.unwrap(), []);
}
