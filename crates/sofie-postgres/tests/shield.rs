#![cfg(feature = "shield")]

use std::time::Duration;

use sofie_core::Timestamp;
use sofie_db::{Db, SharedBlock, SharedBlocks};
use sofie_postgres::Postgres;
use sofie_testing::TestDatabase;

fn block(key: &str, until: Timestamp) -> SharedBlock {
    SharedBlock {
        key: key.to_owned(),
        rule: "scan".to_owned(),
        grade: "block".to_owned(),
        until,
    }
}

#[tokio::test]
async fn a_shared_block_is_read_while_in_force_moved_lifted_and_announced() {
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    db.schema().migrate(&[]).await.unwrap();
    let shared = SharedBlocks::of(&db);
    let mut wakeups = shared.wakeups().unwrap().await.unwrap();
    let now = Timestamp::from_second(Timestamp::now().as_second()).unwrap();
    let later = now + Duration::from_mins(15);

    shared
        .share(&block("address:203.0.113.9", later))
        .await
        .unwrap();
    shared
        .share(&block("address:198.51.100.4", now))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), wakeups.next())
        .await
        .expect("another instance is woken")
        .unwrap();

    assert_eq!(
        shared.active(now).await.unwrap(),
        [block("address:203.0.113.9", later)],
        "a block that ended is not in force"
    );
    let moved = later + Duration::from_mins(15);
    shared
        .share(&block("address:203.0.113.9", moved))
        .await
        .unwrap();
    assert_eq!(shared.active(now).await.unwrap()[0].until, moved);
    shared.lift("address:203.0.113.9").await.unwrap();
    assert_eq!(shared.active(now).await.unwrap(), []);
}
