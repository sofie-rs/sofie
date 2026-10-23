use std::time::{Duration, Instant};

use sofie_testing::TestDatabase;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

async fn half_open(database: &str) -> TcpStream {
    let mut stream = TcpStream::connect("127.0.0.1:25432").await.unwrap();
    let mut body = Vec::new();
    body.extend_from_slice(&196_608_i32.to_be_bytes());
    for (key, value) in [("user", "sofie"), ("database", database)] {
        body.extend_from_slice(key.as_bytes());
        body.push(0);
        body.extend_from_slice(value.as_bytes());
        body.push(0);
    }
    body.push(0);
    let length = i32::try_from(body.len() + 4).unwrap();
    stream.write_all(&length.to_be_bytes()).await.unwrap();
    stream.write_all(&body).await.unwrap();
    stream.flush().await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    stream
}

async fn exists(database: &str) -> bool {
    let (client, connection) = tokio_postgres::connect(
        "postgres://sofie:sofie@127.0.0.1:25432/sofie",
        tokio_postgres::NoTls,
    )
    .await
    .unwrap();
    tokio::spawn(connection);
    let rows = client
        .query("SELECT 1 FROM pg_database WHERE datname = $1", &[&database])
        .await
        .unwrap();
    !rows.is_empty()
}

/// Postgres drops a database only once a half-open connection times out or closes, which needs the test's runtime: dropping must not hold it.
#[tokio::test]
async fn a_connection_left_half_open_does_not_hold_up_the_test() {
    let database = TestDatabase::create().await;
    let name = database.url().rsplit('/').next().unwrap().to_owned();
    let connection = half_open(&name).await;

    let started = Instant::now();
    drop(database);
    let dropping = started.elapsed();
    drop(connection);
    tokio::task::spawn_blocking(sofie_testing::finish_teardown)
        .await
        .unwrap();

    assert!(dropping < Duration::from_secs(1), "{dropping:?}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    assert!(!exists(&name).await, "{name} is dropped");
}
