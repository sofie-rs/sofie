#![cfg(all(feature = "shield", feature = "trusted-proxies"))]

use std::time::Duration;

use sofie::core::Result;
use sofie::db::Db;
use sofie::http::{App, Routes, Shield, ShieldBlocks, ShieldMode, Thresholds, TrustedProxies, Web};
use sofie::postgres::Postgres;
use sofie_testing::{TestDatabase, TestServer};

const ATTACKER: &str = "203.0.113.9";
/// Past the 10 s poll that follows shared blocks when listening fails, as it may under load.
const FOLLOWED_WITHIN: Duration = Duration::from_secs(30);

async fn hello() -> Result<()> {
    Ok(())
}

async fn lift_all(blocks: ShieldBlocks) -> Result<()> {
    for block in blocks.active() {
        blocks.lift(block.id).await?;
    }
    Ok(())
}

fn routes(routes: &mut Routes<Web>) {
    routes.get("/hello", hello);
    routes.post("/lift", lift_all);
}

async fn instance(url: &str) -> TestServer {
    let shield = Shield::new(ShieldMode::Enforce)
        .thresholds(Thresholds::parse("scan=2").unwrap())
        .shared();
    let app = App::new(Db::new(Postgres::connect(url).unwrap()))
        .area("shop", routes)
        .serving(|serving| {
            serving
                .trusted_proxies(TrustedProxies::parse("127.0.0.1").unwrap())
                .shield(shield)
        });
    TestServer::start(app).await
}

async fn status(server: &TestServer, path: &str) -> u16 {
    server
        .get(path)
        .header("x-forwarded-for", ATTACKER)
        .await
        .status
        .as_u16()
}

async fn eventually(server: &TestServer, wanted: u16) -> u16 {
    let reached = tokio::time::timeout(FOLLOWED_WITHIN, async {
        while status(server, "/hello").await != wanted {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    match reached {
        Ok(()) => wanted,
        Err(_) => status(server, "/hello").await,
    }
}

#[tokio::test]
async fn a_block_decided_by_one_instance_holds_on_the_other() {
    let database = TestDatabase::create().await;
    Db::new(Postgres::connect(database.url()).unwrap())
        .schema()
        .migrate(&[])
        .await
        .unwrap();
    let (first, second) = (
        instance(database.url()).await,
        instance(database.url()).await,
    );

    for index in 0..4 {
        assert_eq!(status(&first, &format!("/wp-admin/{index}")).await, 404);
    }

    assert_eq!(eventually(&first, 429).await, 429);
    assert_eq!(eventually(&second, 429).await, 429, "the block is shared");
    assert_eq!(first.post("/lift").accept_json().await.status, 204);
    assert_eq!(eventually(&second, 204).await, 204, "and so is lifting it");
}
