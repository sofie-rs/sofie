use std::num::NonZeroUsize;
use std::time::Duration;

use sofie_core::Error;
use sofie_postgres::{PoolSettings, Postgres};
use sofie_testing::TestDatabase;

#[tokio::test]
async fn a_caller_waiting_past_the_pool_wait_is_turned_away_as_unavailable() {
    let database = TestDatabase::create().await;
    let postgres = Postgres::connect_with(
        database.url(),
        PoolSettings {
            connections: NonZeroUsize::new(1),
            wait: Duration::from_millis(50),
        },
    )
    .unwrap();

    let (held, waiting) = tokio::join!(
        sofie_db::query_values(&postgres, "SELECT 1 FROM pg_sleep(0.5)", &[]),
        async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            sofie_db::query_values(&postgres, "SELECT 1", &[]).await
        }
    );

    assert!(held.is_ok(), "{held:?}");
    let refused = waiting.unwrap_err();
    assert!(
        matches!(&refused, Error::Unavailable(reason) if reason.contains("50 ms")),
        "{refused}"
    );
    assert!(
        sofie_db::query_values(&postgres, "SELECT 1", &[])
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn a_server_without_tls_is_reached_in_the_default_and_preferred_modes() {
    let database = TestDatabase::create().await;
    let separator = if database.url().contains('?') {
        '&'
    } else {
        '?'
    };

    for url in [
        database.url().to_owned(),
        format!("{}{separator}sslmode=prefer", database.url()),
    ] {
        let postgres = Postgres::connect(&url).unwrap();

        assert!(
            sofie_db::query_values(&postgres, "SELECT 1", &[])
                .await
                .is_ok()
        );
        let listening = sofie_db::Driver::wakeups(&postgres).unwrap().await;
        assert!(listening.is_ok(), "{url}");
    }
}

#[tokio::test]
async fn a_connection_keeps_a_bounded_number_of_prepared_statements() {
    let database = TestDatabase::create().await;
    let postgres = Postgres::connect_with(
        database.url(),
        PoolSettings {
            connections: NonZeroUsize::new(1),
            wait: Duration::from_secs(5),
        },
    )
    .unwrap();

    for shape in 0..600 {
        let sql = format!("SELECT {shape}");
        sofie_db::query_values(&postgres, &sql, &[]).await.unwrap();
    }
    let kept = sofie_db::query_values(
        &postgres,
        "SELECT count(*) FROM pg_prepared_statements",
        &[],
    )
    .await
    .unwrap();

    assert!(
        matches!(kept[..], [ref row] if matches!(row[..], [sofie_core::SqlValue::Int(count)] if count <= 512)),
        "{kept:?}"
    );
}
