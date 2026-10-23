#![cfg(all(feature = "toolbar", debug_assertions))]

use sofie_core::{Clock, Error, Instant, Result, TestClock, Timestamp, Uuid};
use sofie_db::Db;
use sofie_http::{App, Area, Principal, Routes, Web};
use sofie_testing::{FakeDriver, TestResponse, TestServer};

const MEMBER: Uuid = Uuid::from_u128(0x0190_a7d2_0000_7000_8000_0000_0000_0001);

struct Account;

impl sofie_http::AreaPrincipal for Account {
    const ACCESS: sofie_http::Access = sofie_http::Access::Owned;
    type Record = Self;
}

struct Members;

impl Area for Members {
    const NAME: &'static str = "members";
    const PREFIX: &'static str = "/members";
    const SIGN_IN: &'static str = "/sign-in";
    type Principal = Account;
}

async fn mine(me: Principal<Members>) -> Result<()> {
    if me.id().uuid() == MEMBER {
        return Ok(());
    }
    Err(Error::Rule("someone else".to_owned()))
}

async fn in_2030(clock: Clock) -> Result<()> {
    let now = clock.now().timestamp();
    let year = Instant::parse("2030-01-01T00:00")?.timestamp()
        ..Instant::parse("2031-01-01T00:00")?.timestamp();
    if year.contains(&now) {
        return Ok(());
    }
    Err(Error::Rule(format!("it is {now}")))
}

async fn hello() -> Result<()> {
    Ok(())
}

fn app() -> App {
    let clock = TestClock::starting_at(Timestamp::from_second(1_700_000_000).unwrap());
    let screens = [("notes/web/hello", "<Section title=\"Hello\"></Section>")];
    App::new(Db::new(FakeDriver::default()).with_clock(clock.clock()))
        .screens(sofie_http::screens_from_sources(&screens).unwrap())
        .area("notes", |routes: &mut Routes<Web>| {
            routes.get("/year", in_2030);
            routes.screen("/hello", "hello", hello);
        })
        .area("notes", |routes: &mut Routes<Members>| {
            routes.get("/mine", mine);
        })
}

async fn toolbar(server: &TestServer, cookie: &str, form: &[(&str, &str)]) -> TestResponse {
    server
        .post("/_sofie/toolbar")
        .header("sec-fetch-site", "same-origin")
        .header(
            "referer",
            &format!("{}/members/mine?page=2", server.origin()),
        )
        .header("cookie", cookie)
        .form(form)
        .await
}

fn cookie_of(answer: &TestResponse) -> String {
    let set = answer.header("set-cookie");
    assert!(set.starts_with("__Host-sofie-toolbar="), "{answer:?}");
    set.split(';').next().unwrap().to_owned()
}

#[tokio::test]
async fn acting_as_a_member_signs_this_browser_in_as_them_until_it_stops() {
    let server = TestServer::start(app()).await;
    let principal = MEMBER.to_string();
    assert_eq!(server.get("/members/mine").accept_json().await.status, 401);

    let started = toolbar(
        &server,
        "",
        &[
            ("action", "act_as"),
            ("area", "members"),
            ("principal", &principal),
        ],
    )
    .await;
    let acting = cookie_of(&started);
    let mine = server
        .get("/members/mine")
        .header("cookie", &acting)
        .accept_json()
        .await;
    let stopped = toolbar(&server, &acting, &[("action", "stop_acting")]).await;

    started.assert_status(303);
    assert_eq!(started.header("location"), "/members/mine?page=2");
    mine.assert_status(204);
    assert!(
        stopped.header("set-cookie").contains("Max-Age=0"),
        "{stopped:?}"
    );
}

#[tokio::test]
async fn only_a_signed_in_area_can_be_acted_in() {
    let server = TestServer::start(app()).await;
    let member = MEMBER.to_string();

    let unknown = toolbar(
        &server,
        "",
        &[
            ("action", "act_as"),
            ("area", "web"),
            ("principal", &member),
        ],
    )
    .await;

    unknown.assert_status(400);
    assert_eq!(unknown.header("set-cookie"), "");
}

#[tokio::test]
async fn a_cross_site_form_changes_nothing() {
    let server = TestServer::start(app()).await;

    let answer = server
        .post("/_sofie/toolbar")
        .header("sec-fetch-site", "cross-site")
        .form(&[("action", "reset_clock")])
        .await;

    answer.assert_status(403);
    assert_eq!(answer.header("set-cookie"), "");
}

#[tokio::test]
async fn a_set_clock_moves_this_browsers_requests_only() {
    let server = TestServer::start(app()).await;
    let set = toolbar(
        &server,
        "",
        &[("action", "set_clock"), ("at", "2030-06-01T09:00")],
    )
    .await;
    let moved = cookie_of(&set);

    let in_2030 = server
        .get("/year")
        .header("cookie", &moved)
        .accept_json()
        .await;
    let elsewhere = server.get("/year").accept_json().await;
    let shifted = toolbar(
        &server,
        &moved,
        &[("action", "shift_clock"), ("hours", "-8784")],
    )
    .await;
    let back_a_year = server
        .get("/year")
        .header("cookie", &cookie_of(&shifted))
        .accept_json()
        .await;

    in_2030.assert_status(204);
    elsewhere.assert_status(422);
    back_a_year.assert_status(422);
}

#[tokio::test]
async fn a_toolbar_cookie_the_server_did_not_seal_overrides_nothing() {
    let server = TestServer::start(app()).await;
    let forged = format!("__Host-sofie-toolbar=as=members:{MEMBER}");

    let mine = server
        .get("/members/mine")
        .header("cookie", &forged)
        .accept_json()
        .await;

    mine.assert_status(401);
}

#[tokio::test]
async fn the_toolbar_names_the_overrides_this_browser_holds() {
    let _telemetry = sofie_telemetry::capture();
    let server = TestServer::start(app()).await;
    let member = MEMBER.to_string();
    let mut cookie = String::new();
    for form in [
        &[
            ("action", "act_as"),
            ("area", "members"),
            ("principal", &member),
        ][..],
        &[("action", "flag"), ("flag", "shop.beta"), ("answer", "on")],
        &[("action", "shift_clock"), ("hours", "1")],
    ] {
        cookie = cookie_of(&toolbar(&server, &cookie, form).await);
    }

    let page = server.get("/hello").header("cookie", &cookie).await;

    let toolbar = page
        .body
        .split("<details id=\"sofie-toolbar\"")
        .nth(1)
        .unwrap_or_default();
    assert!(
        toolbar.contains(&format!("acting as members {MEMBER}")),
        "{}",
        page.body
    );
    assert!(toolbar.contains("flag shop.beta=on"), "{toolbar}");
    assert!(toolbar.contains("clock +3600 s"), "{toolbar}");
    assert!(toolbar.contains("<option>members</option>"), "{toolbar}");
    assert!(toolbar.contains("value=\"stop_acting\""), "{toolbar}");
}
