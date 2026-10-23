#![cfg(all(
    feature = "signed-links",
    feature = "steps",
    feature = "trusted-proxies"
))]

use std::net::IpAddr;
use std::sync::Mutex;
use std::time::Duration;

use http::StatusCode;
use sofie_core::{Error, Result, TestClock, Timestamp};
use sofie_db::worker::Worker;
use sofie_db::{BoxFuture, Db, Outcome, ReadOnly, Reply};
use sofie_http::{App, PipelineStep, Rate, RequestHead, Routes, SignedUrls, TrustedProxies, Web};
use sofie_testing::{FakeDriver, TestServer};

const HOUR: Duration = Duration::from_hours(1);

struct Link(String);

impl Outcome for Link {
    fn reply(self) -> Result<Reply> {
        Ok(Reply::Json(serde_json::json!(self.0)))
    }
}

impl ReadOnly for Link {}

async fn hello() -> Result<()> {
    Ok(())
}

async fn download_link(urls: SignedUrls) -> Result<Link> {
    urls.sign("/download?file=report", HOUR).map(Link)
}

struct OnlyFrom(IpAddr);

impl PipelineStep for OnlyFrom {
    fn check<'a>(&'a self, request: &'a RequestHead<'a>) -> BoxFuture<'a, Result<()>> {
        let allowed = request.client() == self.0;
        Box::pin(async move {
            if allowed {
                Ok(())
            } else {
                Err(Error::NotFound)
            }
        })
    }
}

fn routes(routes: &mut Routes<Web>) {
    routes.get("/search", hello).throttle(Rate::per_minute(2));
    routes.get("/hello", hello);
    routes.get("/link", download_link);
    routes.get("/download", hello).signed();
    routes.post("/note", hello);
    routes.post("/upload", hello).body_limit(256 * 1024);
}

fn clock() -> TestClock {
    TestClock::starting_at(Timestamp::from_second(1_700_000_000).unwrap())
}

fn app(clock: &TestClock) -> App {
    App::new(Db::new(FakeDriver::default()).with_clock(clock.clock())).area("files", routes)
}

#[tokio::test]
async fn a_throttled_route_refuses_past_its_rate_until_the_budget_refills() {
    let clock = clock();
    let server = TestServer::start(app(&clock)).await;

    for _ in 0..2 {
        assert_eq!(server.get("/search").await.status, StatusCode::NO_CONTENT);
    }
    let refused = server.get("/search").accept_json().await;
    let other_route = server.get("/hello").await;
    clock.advance(Duration::from_secs(30)).unwrap();
    let refilled = server.get("/search").await;

    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.header("retry-after"), "30");
    assert_eq!(
        refused.json()["error"],
        "Too many requests; try again in 30 seconds."
    );
    assert_eq!(other_route.status, StatusCode::NO_CONTENT);
    assert_eq!(refilled.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn an_area_limit_counts_every_route_of_the_area() {
    let clock = clock();
    let server = TestServer::start(app(&clock).throttle::<Web>(Rate::per_hour(3))).await;

    let answers = [
        server.get("/hello").await,
        server.get("/link").await,
        server.get("/hello").await,
        server.get("/link").await,
    ];

    let statuses: Vec<StatusCode> = answers.iter().map(|answer| answer.status).collect();
    assert_eq!(statuses[3], StatusCode::TOO_MANY_REQUESTS);
    assert!(
        statuses[..3].iter().all(StatusCode::is_success),
        "{statuses:?}"
    );
    assert_eq!(answers[3].header("retry-after"), "1200");
}

#[tokio::test]
async fn a_signed_route_opens_only_through_a_live_signed_link() {
    let clock = clock();
    let server = TestServer::start(app(&clock)).await;
    let link = server.get("/link").await.json();
    let link = link.as_str().unwrap();

    let signed = server.get(link).await;
    let unsigned = server.get("/download?file=report").accept_json().await;
    let altered = server.get(&link.replace("report", "secrets")).await;
    clock.advance(HOUR).unwrap();
    let expired = server.get(link).accept_json().await;

    assert_eq!(signed.status, StatusCode::NO_CONTENT);
    assert_eq!(unsigned.status, StatusCode::FORBIDDEN);
    assert_eq!(
        unsigned.json()["error"],
        "This link is invalid or has expired."
    );
    assert_eq!(altered.status, StatusCode::FORBIDDEN);
    assert_eq!(expired.status, StatusCode::FORBIDDEN);
}

static JOB_LINK: Mutex<Option<String>> = Mutex::new(None);

async fn sign_in_a_job(urls: SignedUrls) -> Result<()> {
    let link = urls.sign("/download?file=report", HOUR)?;
    *JOB_LINK.lock().unwrap() = Some(link);
    Ok(())
}

#[tokio::test]
async fn a_job_signs_links_with_the_keys_of_the_app() {
    let app = sofie_testing::start(
        |db: Db| async move { Ok(App::new(db).area("files", routes)) },
        |db| Worker::new(db).every("sign", HOUR, sign_in_a_job),
    )
    .await;

    app.drain().await;
    let link = JOB_LINK.lock().unwrap().clone().unwrap();

    assert_eq!(app.get(&link).await.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn a_body_is_small_unless_its_route_raises_the_limit() {
    let server = TestServer::start(app(&clock())).await;
    let body = vec![b'x'; 100 * 1024];

    let note = server.post("/note").body(body.clone()).await;
    let upload = server.post("/upload").body(body).await;
    let too_large = server
        .post("/upload")
        .body(vec![b'x'; 256 * 1024 + 1])
        .await;

    assert_eq!(note.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(upload.status, StatusCode::NO_CONTENT);
    assert_eq!(too_large.status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn an_area_step_sees_the_client_named_by_trusted_proxies_only() {
    let office: IpAddr = "203.0.113.7".parse().unwrap();
    let guarded = || app(&clock()).step::<Web>(OnlyFrom(office));
    let behind_proxy =
        TestServer::start(guarded().serving(|serving| {
            serving.trusted_proxies(TrustedProxies::parse("127.0.0.1").unwrap())
        }))
        .await;
    let direct = TestServer::start(guarded()).await;
    let forwarded =
        |server: &TestServer, client: &str| server.get("/hello").header("x-forwarded-for", client);

    let from_office = forwarded(&behind_proxy, "198.51.100.1, 203.0.113.7").await;
    let from_elsewhere = forwarded(&behind_proxy, "203.0.113.7, 198.51.100.1").await;
    let claimed_directly = forwarded(&direct, "203.0.113.7").await;

    assert_eq!(from_office.status, StatusCode::NO_CONTENT);
    assert_eq!(from_elsewhere.status, StatusCode::NOT_FOUND);
    assert_eq!(claimed_directly.status, StatusCode::NOT_FOUND);
}

/// Each request names another principal of another area, as fresh sign-ups would.
#[tokio::test]
async fn another_area_s_session_cookies_do_not_lift_a_public_route_s_throttle() {
    let clock = clock();
    let server = TestServer::start(app(&clock)).await;
    let keys = server.signing_keys();
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let mut statuses = Vec::new();

    for _ in 0..3 {
        let session = sofie_core::Uuid::now_v7().unwrap();
        let principal = sofie_core::Uuid::now_v7().unwrap();
        let sealed = sofie_auth::SessionClaims::new(session, "members", principal, now)
            .seal(&keys)
            .unwrap();
        let cookie = format!("__Host-sofie-session={sealed}");
        statuses.push(server.get("/search").header("cookie", &cookie).await.status);
    }

    assert_eq!(statuses[2], StatusCode::TOO_MANY_REQUESTS, "{statuses:?}");
}
