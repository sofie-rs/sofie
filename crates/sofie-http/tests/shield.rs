#![cfg(all(feature = "shield", feature = "trusted-proxies"))]

use std::time::Duration;

use http::StatusCode;
use sofie_core::{Result, TestClock, Timestamp};
use sofie_db::Db;
use sofie_http::{App, Routes, Shield, ShieldMode, Thresholds, TrustedProxies, Web};
use sofie_testing::{FakeDriver, TestResponse, TestServer};

const ATTACKER: &str = "203.0.113.9";
const NEIGHBOUR: &str = "198.51.100.4";
/// The detector decides on its own thread.
const DECIDED_WITHIN: Duration = Duration::from_secs(5);

async fn hello() -> Result<()> {
    Ok(())
}

fn routes(routes: &mut Routes<Web>) {
    routes.get("/hello", hello);
}

/// Behind a loopback proxy naming each client in `X-Forwarded-For`.
async fn served(clock: &TestClock) -> TestServer {
    let shield = Shield::new(ShieldMode::Enforce).thresholds(Thresholds::parse("scan=3").unwrap());
    let app = App::new(Db::new(FakeDriver::default()).with_clock(clock.clock()))
        .area("shop", routes)
        .serving(|serving| {
            serving
                .trusted_proxies(TrustedProxies::parse("127.0.0.1").unwrap())
                .shield(shield)
        });
    TestServer::start(app).await
}

fn clock() -> TestClock {
    TestClock::starting_at(Timestamp::from_second(1_700_000_000).unwrap())
}

/// A second apart, so a slow-down decided off the request path never meets a later scan.
async fn scan(server: &TestServer, clock: &TestClock, client: Option<&str>, count: usize) {
    for index in 0..count {
        if index > 0 {
            clock.advance(Duration::from_secs(1)).unwrap();
        }
        let request = server.get(&format!("/wp-admin/{index}"));
        let request = match client {
            Some(client) => request.header("x-forwarded-for", client),
            None => request,
        };
        assert_eq!(request.await.status, StatusCode::NOT_FOUND);
    }
}

async fn hello_from(server: &TestServer, client: &str) -> TestResponse {
    server
        .get("/hello")
        .header("x-forwarded-for", client)
        .accept_json()
        .await
}

async fn refused(server: &TestServer, client: &str, seconds: &str) -> TestResponse {
    let started = std::time::Instant::now();
    loop {
        let answer = hello_from(server, client).await;
        if answer.header("retry-after") == seconds || started.elapsed() > DECIDED_WITHIN {
            return answer;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn a_scanning_caller_is_slowed_down_to_one_request_a_second() {
    let clock = clock();
    let server = served(&clock).await;
    scan(&server, &clock, Some(ATTACKER), 3).await;

    let slowed = refused(&server, ATTACKER, "1").await;
    clock.advance(Duration::from_secs(1)).unwrap();
    let next_second = hello_from(&server, ATTACKER).await;

    assert_eq!(slowed.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(slowed.header("retry-after"), "1");
    assert_eq!(next_second.status, StatusCode::NO_CONTENT);
    assert_eq!(
        hello_from(&server, ATTACKER).await.status,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn a_caller_scanning_on_is_blocked_until_its_block_ends() {
    let clock = clock();
    let server = served(&clock).await;
    scan(&server, &clock, Some(ATTACKER), 6).await;

    let blocked = refused(&server, ATTACKER, "900").await;
    assert_eq!(blocked.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(blocked.header("retry-after"), "900");
    assert_eq!(
        blocked.json()["error"],
        "Too many requests; try again in 900 seconds."
    );
    assert_eq!(
        hello_from(&server, NEIGHBOUR).await.status,
        StatusCode::NO_CONTENT,
        "another caller passes"
    );
    let unmatched = server
        .get("/wp-login.php")
        .header("x-forwarded-for", ATTACKER)
        .await;
    assert_eq!(
        unmatched.status,
        StatusCode::TOO_MANY_REQUESTS,
        "no free 404 either"
    );

    clock.advance(Duration::from_mins(15)).unwrap();
    assert_eq!(
        hello_from(&server, ATTACKER).await.status,
        StatusCode::NO_CONTENT,
        "every block ends"
    );
}

#[tokio::test]
async fn a_blocked_caller_is_refused_the_framework_s_own_endpoints_too() {
    let clock = clock();
    let server = served(&clock).await;
    scan(&server, &clock, Some(ATTACKER), 6).await;
    refused(&server, ATTACKER, "900").await;

    let ready = server
        .get("/ready")
        .header("x-forwarded-for", ATTACKER)
        .await;
    let neighbours = server
        .get("/ready")
        .header("x-forwarded-for", NEIGHBOUR)
        .await;

    assert_eq!(ready.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(neighbours.status, StatusCode::OK);
}

#[tokio::test]
async fn the_trusted_proxy_itself_is_never_blocked() {
    let clock = clock();
    let server = served(&clock).await;
    scan(&server, &clock, None, 20).await;
    scan(&server, &clock, Some(ATTACKER), 6).await;

    // Signals are counted in order, so the proxy's were counted by now.
    assert_eq!(
        refused(&server, ATTACKER, "900").await.status,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(server.get("/hello").await.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn paths_browsers_ask_for_on_their_own_are_no_scan() {
    let clock = clock();
    let server = served(&clock).await;
    for path in [
        "/favicon.ico",
        "/robots.txt",
        "/.well-known/security.txt",
        "/apple-touch-icon-120x120-precomposed.png",
    ] {
        let missing = server.get(path).header("x-forwarded-for", NEIGHBOUR).await;
        assert_eq!(missing.status, StatusCode::NOT_FOUND, "{path}");
    }
    scan(&server, &clock, Some(ATTACKER), 3).await;

    // Signals are counted in order, so the neighbour's were counted by now.
    refused(&server, ATTACKER, "1").await;
    for _ in 0..2 {
        assert_eq!(
            hello_from(&server, NEIGHBOUR).await.status,
            StatusCode::NO_CONTENT
        );
    }
}

#[cfg(feature = "webhooks")]
mod webhooks {
    use http::StatusCode;
    use sofie_core::{Error, Result};
    use sofie_db::Db;
    use sofie_http::{
        App, Delivery, Routes, Shield, ShieldMode, Thresholds, TrustedProxies, Verify, Web,
    };
    use sofie_testing::{FakeDriver, TestServer};

    use super::{ATTACKER, clock, hello, refused, scan};

    struct Unsigned;

    impl Verify for Unsigned {
        fn verify(&self, _delivery: &Delivery<'_>) -> Result<String> {
            Err(Error::BadRequest("the signature does not match".to_owned()))
        }
    }

    fn routes(routes: &mut Routes<Web>) {
        routes.get("/hello", hello);
        routes.webhook("/callback", hello).verify(Unsigned);
    }

    #[tokio::test]
    async fn a_blocked_caller_is_refused_at_a_webhook_too() {
        let clock = clock();
        let shield =
            Shield::new(ShieldMode::Enforce).thresholds(Thresholds::parse("scan=3").unwrap());
        let app = App::new(Db::new(FakeDriver::default()).with_clock(clock.clock()))
            .area("shop", routes)
            .serving(|serving| {
                serving
                    .trusted_proxies(TrustedProxies::parse("127.0.0.1").unwrap())
                    .shield(shield)
            });
        let server = TestServer::start(app).await;
        scan(&server, &clock, Some(ATTACKER), 6).await;
        let blocked = refused(&server, ATTACKER, "900").await;

        let delivered = server
            .post("/callback")
            .header("x-forwarded-for", ATTACKER)
            .accept_json()
            .await;

        assert_eq!(blocked.status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(delivered.status, StatusCode::TOO_MANY_REQUESTS);
    }
}
