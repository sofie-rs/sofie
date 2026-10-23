use std::time::Duration;

use http::StatusCode;
use sofie_core::{Cast, ColumnReader, Result, SqlValue, TestClock, Timestamp, Uuid};
use sofie_db::{Aggregate, Db, Field, Id, Meta, Outcome, Read, ReadOnly, Reply, Select};
use sofie_http::{Admitter, App, Area, Principal, Routes, Session, SignInRight, Web};
use sofie_testing::{FakeDriver, TestRequest, TestResponse, TestServer, TestSession};
use tokio::net::TcpListener;

const MEMBER: Uuid = Uuid::from_u128(0x0190_a7d2_0000_7000_8000_0000_0000_0001);
const STRANGER: Uuid = Uuid::from_u128(0x0190_a7d2_0000_7000_8000_0000_0000_0002);

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

struct Others;

impl Area for Others {
    const NAME: &'static str = "others";
    const PREFIX: &'static str = "/others";
    const SIGN_IN: &'static str = "/sign-in";
    type Principal = Account;
}

struct Note {
    meta: Meta,
    author: Id<Account>,
}

impl Aggregate for Note {
    const NAME: &'static str = "note";
    const MODULE: &'static str = "notes";
    const FIELDS: &'static [Field] = &[Field::new("author", Uuid::COLUMNS, false)];
    const OWNER: Option<&'static str> = Some("author");

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.author.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            author: Id::decode(row)?,
        })
    }

    fn owner(&self) -> Option<Uuid> {
        Some(self.author.uuid())
    }
}

/// Signs in as the test's own admitter.
struct Tests;

impl Admitter for Tests {}

const RIGHT: SignInRight = SignInRight::of::<Tests>();

async fn sign_in_member(session: Session) -> Result<()> {
    session.sign_in::<Members>(RIGHT, Id::from_uuid(MEMBER))?;
    Ok(())
}

async fn sign_in_then_fail(session: Session) -> Result<()> {
    session.sign_in::<Members>(RIGHT, Id::from_uuid(MEMBER))?;
    Err(sofie_core::Error::Rule("not today".to_owned()))
}

async fn sign_out(session: Session) -> Result<()> {
    session.sign_out();
    Ok(())
}

async fn hello() -> Result<()> {
    Ok(())
}

struct Notes(Vec<Note>);

impl Outcome for Notes {
    fn reply(self) -> Result<Reply> {
        Ok(Reply::Json(serde_json::json!(self.0.len())))
    }
}

impl ReadOnly for Notes {}

async fn list_notes(read: Read) -> Result<Notes> {
    Note::query().all(&read).await.map(Notes)
}

async fn write_mine(me: Principal<Members>) -> Result<Note> {
    Ok(Note {
        meta: Meta::new().unwrap(),
        author: me.id(),
    })
}

async fn write_theirs(_me: Principal<Members>) -> Result<Note> {
    Ok(Note {
        meta: Meta::new().unwrap(),
        author: Id::from_uuid(STRANGER),
    })
}

fn web_routes(routes: &mut Routes<Web>) {
    routes.post("/sign-in", sign_in_member);
    routes
        .post("/sign-in/form", sign_in_member)
        .then_redirect("/members/notes");
    routes.post("/sign-in/failing", sign_in_then_fail);
    routes.post("/sign-out", sign_out);
    routes.get("/hello", hello);
}

fn member_routes(routes: &mut Routes<Members>) {
    routes.get("/notes", list_notes);
    routes.post("/notes/mine", write_mine);
    routes.post("/notes/theirs", write_theirs);
}

fn other_routes(routes: &mut Routes<Others>) {
    routes.get("/notes", hello);
}

struct Running {
    server: TestServer,
    driver: FakeDriver,
    clock: TestClock,
}

async fn start() -> Running {
    let driver = FakeDriver::default();
    let clock = TestClock::starting_at(Timestamp::from_second(1_700_000_000).unwrap());
    let app = App::new(Db::new(driver.clone()).with_clock(clock.clock()))
        .area("notes", web_routes)
        .area("notes", member_routes)
        .area("notes", other_routes);
    Running {
        server: TestServer::start(app).await,
        driver,
        clock,
    }
}

fn set_cookie(answer: &TestResponse) -> &str {
    answer.header("set-cookie")
}

fn from_page(request: TestRequest, session: &TestSession) -> TestRequest {
    request.session(session).accept_json()
}

async fn signed_in(server: &Running) -> TestSession {
    let answer = server.server.post("/sign-in").await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT);
    answer
        .session()
        .expect("signing in sets the session cookie")
}

#[tokio::test]
async fn signing_in_issues_a_fresh_host_only_cookie_each_time() {
    let running = start().await;

    let first = running.server.post("/sign-in").await;
    let second = running.server.post("/sign-in").await;

    let cookie = set_cookie(&first);
    assert!(
        cookie.starts_with("__Host-sofie-session="),
        "a session cookie"
    );
    assert!(
        cookie.ends_with("; Max-Age=2592000; Path=/; Secure; HttpOnly; SameSite=Lax"),
        "its attributes"
    );
    assert!(!cookie.contains("Domain"));
    assert_ne!(
        first.session(),
        second.session(),
        "every sign-in seals anew"
    );
}

#[tokio::test]
async fn a_session_is_signed_in_only_once_the_handler_succeeded() {
    let running = start().await;

    let failing = running.server.post("/sign-in/failing").await;

    assert_eq!(failing.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(!failing.headers.contains_key("set-cookie"));
}

async fn sign_in_toward(running: &Running, next: &str) -> String {
    let answer = running
        .server
        .post(&format!("/sign-in/form?next={next}"))
        .header("content-type", "application/x-www-form-urlencoded")
        .await;
    assert_eq!(answer.status, StatusCode::SEE_OTHER, "{next}");
    answer.header("location").to_owned()
}

#[tokio::test]
async fn signing_in_follows_next_only_to_a_page_the_new_session_opens() {
    let running = start().await;

    let public = sign_in_toward(&running, "/hello").await;
    let same_area = sign_in_toward(&running, "/members/notes%3Fpage%3D2").await;
    let other_area = sign_in_toward(&running, "/others/notes").await;
    let unknown = sign_in_toward(&running, "/nowhere").await;

    assert_eq!(public, "/hello");
    assert_eq!(same_area, "/members/notes?page=2");
    assert_eq!(other_area, "/members/notes", "another area would bounce");
    assert_eq!(unknown, "/members/notes");
    assert_eq!(running.driver.queries().len(), 0, "no database read");
}

#[tokio::test]
async fn an_owned_area_sends_each_caller_without_a_session_to_sign_in() {
    let running = start().await;
    let server = &running.server;

    let page = server.get("/members/notes?page=2").await;
    let htmx = server
        .get("/members/notes")
        .header("hx-request", "true")
        .await;
    let json = server.get("/members/notes").accept_json().await;

    assert_eq!(page.status, StatusCode::SEE_OTHER);
    assert_eq!(
        page.header("location"),
        "/sign-in?next=/members/notes%3Fpage%3D2"
    );
    assert_eq!(htmx.status, StatusCode::UNAUTHORIZED);
    assert_eq!(htmx.header("hx-redirect"), "/sign-in?next=/members/notes");
    assert_eq!(json.status, StatusCode::UNAUTHORIZED);
    assert_eq!(running.driver.queries().len(), 0);
}

#[tokio::test]
async fn an_owned_area_reads_only_its_principals_records() {
    let running = start().await;
    let session = signed_in(&running).await;

    let listed = from_page(running.server.get("/members/notes"), &session).await;

    assert_eq!(listed.status, StatusCode::OK);
    let queries = running.driver.queries();
    let [(sql, params)] = queries.as_slice() else {
        panic!("expected one query");
    };
    assert!(
        sql.ends_with("FROM \"notes\".\"note\" WHERE (\"author\" = $1)"),
        "{sql}"
    );
    assert_eq!(params, &[SqlValue::Uuid(MEMBER)]);
}

#[tokio::test]
async fn an_owned_area_writes_its_principals_records_and_refuses_any_other() {
    let running = start().await;
    let session = signed_in(&running).await;

    let mine = from_page(running.server.post("/members/notes/mine"), &session).await;
    let theirs = from_page(running.server.post("/members/notes/theirs"), &session).await;

    assert_eq!(mine.status, StatusCode::OK);
    assert_eq!(theirs.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(running.driver.transaction_count(), 1);
}

#[tokio::test]
async fn a_request_carrying_a_session_must_say_it_came_from_the_apps_pages() {
    let running = start().await;
    let session = signed_in(&running).await;
    let mine = || running.server.post("/members/notes/mine");

    let bare = mine().cookie(&session).await;
    let cross_site = mine()
        .session(&session)
        .header("sec-fetch-site", "cross-site")
        .await;

    assert_eq!(bare.status, StatusCode::FORBIDDEN);
    assert_eq!(cross_site.status, StatusCode::FORBIDDEN);
    assert_eq!(running.driver.transaction_count(), 0);
}

#[tokio::test]
async fn an_active_session_is_resealed_at_most_every_five_minutes() {
    let running = start().await;
    let session = signed_in(&running).await;
    let notes = || from_page(running.server.get("/members/notes"), &session);

    running.clock.advance(Duration::from_secs(299)).unwrap();
    let soon = notes().await;
    running.clock.advance(Duration::from_secs(1)).unwrap();
    let later = notes().await;

    assert!(!soon.headers.contains_key("set-cookie"));
    assert!(set_cookie(&later).contains("Max-Age=2591700"));
    assert_ne!(later.session(), Some(session.clone()));
}

#[tokio::test]
async fn an_idle_or_signed_out_session_no_longer_opens_the_area() {
    let running = start().await;
    let session = signed_in(&running).await;

    let signed_out = from_page(running.server.post("/sign-out"), &session).await;
    running.clock.advance(Duration::from_hours(12)).unwrap();
    let idle = from_page(running.server.get("/members/notes"), &session).await;

    assert!(set_cookie(&signed_out).contains("Max-Age=0"));
    assert_eq!(idle.status, StatusCode::UNAUTHORIZED);
    assert!(
        set_cookie(&idle).contains("Max-Age=0"),
        "a dead cookie is cleared"
    );
}

/// What an app without a session store keeps, as one with sofie-identity's `sessions` off.
#[tokio::test]
async fn without_a_session_store_a_copied_cookie_outlives_signing_out() {
    let running = start().await;
    let session = signed_in(&running).await;

    from_page(running.server.post("/sign-out"), &session).await;
    let copy = from_page(running.server.get("/members/notes"), &session).await;

    assert_eq!(copy.status, StatusCode::OK);
    assert!(!copy.headers.contains_key("set-cookie"));
}

#[tokio::test]
async fn a_forged_cookie_is_no_session_and_is_cleared() {
    let running = start().await;
    let forged = "__Host-sofie-session=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    let from_page = |path: &str| {
        running
            .server
            .get(path)
            .header("cookie", forged)
            .header("sec-fetch-site", "same-origin")
            .accept_json()
    };

    let public = from_page("/hello").await;
    let owned = from_page("/members/notes").await;

    assert_eq!(public.status, StatusCode::NO_CONTENT);
    assert!(set_cookie(&public).contains("Max-Age=0"));
    assert_eq!(owned.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn an_app_without_signing_keys_refuses_to_serve_naming_the_variable() {
    if ["SOFIE_SIGNING_KEYS", "SOFIE_SESSION_KEYS"]
        .iter()
        .any(|key| std::env::var_os(key).is_some())
    {
        return;
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();

    let error = App::new(Db::new(FakeDriver::default()))
        .serve(listener, std::future::pending())
        .await
        .unwrap_err();

    assert!(
        error
            .to_string()
            .starts_with("SOFIE_SIGNING_KEYS is not set"),
        "{error}"
    );
}

async fn named(name: &'static str) -> TestServer {
    let app = App::new(Db::new(FakeDriver::default()))
        .serving(|serving| serving.named(name))
        .area("notes", web_routes);
    TestServer::start(app).await
}

#[tokio::test]
async fn apps_named_apart_neither_read_nor_clear_each_other_s_session_cookie() {
    let shop = named("shop").await;
    let islands = named("islands-demo").await;

    let signed_in = shop.post("/sign-in").await;
    let cookie = set_cookie(&signed_in).split(';').next().unwrap().to_owned();
    let public = islands.get("/hello").header("cookie", &cookie).await;
    let foreign_write = islands.post("/sign-in").header("cookie", &cookie).await;
    let own_write = shop.post("/sign-in").header("cookie", &cookie).await;

    assert!(cookie.starts_with("__Host-shop-session="), "{cookie}");
    assert_eq!(public.status, StatusCode::NO_CONTENT);
    assert!(!public.headers.contains_key("set-cookie"), "not cleared");
    assert_eq!(foreign_write.status, StatusCode::NO_CONTENT);
    assert!(set_cookie(&foreign_write).starts_with("__Host-islands-demo-session="));
    assert_eq!(
        own_write.status,
        StatusCode::FORBIDDEN,
        "its own cookie needs an origin"
    );
}

#[tokio::test]
async fn an_app_name_that_cannot_name_a_cookie_refuses_to_serve() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();

    let error = App::new(Db::new(FakeDriver::default()))
        .serving(|serving| serving.named("my shop"))
        .serve(listener, std::future::pending())
        .await
        .unwrap_err();

    assert!(
        error.to_string().contains("`my shop` cannot name a cookie"),
        "{error}"
    );
}
