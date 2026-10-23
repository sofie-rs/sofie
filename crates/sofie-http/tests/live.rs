#![cfg(feature = "live")]

use std::time::Duration;

use http::StatusCode;
use serde_json::json;
use sofie_core::{Cast, ColumnReader, Result, SqlValue, TestClock, Timestamp, Uuid};
use sofie_db::{Aggregate, Db, Field, Id, Meta, Outcome, Read, ReadOnly, Reply, Select, Topic};
use sofie_http::{Admitter, App, Area, Principal, Routes, Session, SignInRight, Web};
use sofie_testing::{FakeDriver, TestServer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const MEMBER: Uuid = Uuid::from_u128(0x0190_a7d2_0000_7000_8000_0000_0000_0001);
const STRANGER: Uuid = Uuid::from_u128(0x0190_a7d2_0000_7000_8000_0000_0000_0002);
const NOTES_SCREEN: &str = r#"<Section title="Notes">
  <Table id="notes" of={data} columns="author" />
</Section>
"#;

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

struct Notes(Vec<Note>);

impl Outcome for Notes {
    fn reply(self) -> Result<Reply> {
        let rows: Vec<_> = self
            .0
            .iter()
            .map(|note| json!({ "author": note.author.uuid().to_string() }))
            .collect();
        Ok(Reply::Json(json!(rows)))
    }
}

impl ReadOnly for Notes {}

async fn list_notes(read: Read) -> Result<Notes> {
    Note::query().all(&read).await.map(Notes)
}

async fn write_mine(me: Principal<Members>) -> Result<Note> {
    Ok(Note {
        meta: Meta::new()?,
        author: me.id(),
    })
}

/// Signs in as the test's own admitter.
struct Tests;

impl Admitter for Tests {}

const RIGHT: SignInRight = SignInRight::of::<Tests>();

async fn sign_in_member(session: Session) -> Result<()> {
    session.sign_in::<Members>(RIGHT, Id::from_uuid(MEMBER))?;
    Ok(())
}

async fn sign_in_stranger(session: Session) -> Result<()> {
    session.sign_in::<Members>(RIGHT, Id::from_uuid(STRANGER))?;
    Ok(())
}

fn web_routes(routes: &mut Routes<Web>) {
    routes.post("/sign-in", sign_in_member);
    routes.post("/sign-in/stranger", sign_in_stranger);
}

fn member_routes(routes: &mut Routes<Members>) {
    routes.screen("/notes", "notes", list_notes).live("notes");
    routes.post("/notes", write_mine);
}

async fn start() -> TestServer {
    start_at(&TestClock::starting_at(Timestamp::now())).await
}

async fn start_at(clock: &TestClock) -> TestServer {
    let screens = sofie_http::screens_from_sources(&[("notes/members/notes", NOTES_SCREEN)]);
    let app = App::new(Db::new(FakeDriver::default()).with_clock(clock.clock()))
        .screens(screens.unwrap())
        .area("notes", web_routes)
        .area("notes", member_routes);
    TestServer::start(app).await
}

async fn signed_in(server: &TestServer, path: &str) -> String {
    let answer = server
        .post(path)
        .header("sec-fetch-site", "same-origin")
        .await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT);
    let cookie = answer.header("set-cookie");
    cookie.split(';').next().unwrap().to_owned()
}

fn unsealed_stream_path() -> String {
    format!(
        "/_sofie/live/members?topics={}",
        Topic::named("notes", "note")
    )
}

/// As the page names it, sealed by the server.
async fn stream_path(server: &TestServer, cookie: &str) -> String {
    let page = server.get("/members/notes").header("cookie", cookie).await;
    let (_, rest) = page.body.split_once("data-stream=\"").unwrap();
    rest.split('"').next().unwrap().replace("&amp;", "&")
}

struct Stream {
    socket: TcpStream,
    read: String,
}

impl Stream {
    async fn open(server: &TestServer, path: &str, headers: &str) -> Self {
        let address = server.origin().trim_start_matches("http://");
        let mut socket = TcpStream::connect(address).await.unwrap();
        let request = format!("GET {path} HTTP/1.1\r\nHost: {address}\r\n{headers}\r\n");
        socket.write_all(request.as_bytes()).await.unwrap();
        Self {
            socket,
            read: String::new(),
        }
    }

    async fn signed_in(server: &TestServer, cookie: &str) -> Self {
        let headers = format!("Cookie: {cookie}\r\nSec-Fetch-Site: same-origin\r\n");
        let path = stream_path(server, cookie).await;
        let mut stream = Self::open(server, &path, &headers).await;
        assert!(stream.heard(": open").await, "{}", stream.read);
        stream
    }

    async fn heard(&mut self, text: &str) -> bool {
        let reading = async {
            let mut chunk = [0; 1024];
            while !self.read.contains(text) {
                let count = self.socket.read(&mut chunk).await.unwrap();
                if count == 0 {
                    return;
                }
                self.read
                    .push_str(&String::from_utf8_lossy(&chunk[..count]));
            }
        };
        let _ = tokio::time::timeout(Duration::from_secs(1), reading).await;
        let heard = self.read.contains(text);
        self.read.clear();
        heard
    }
}

async fn write_note(server: &TestServer, cookie: &str) {
    let written = server
        .post("/members/notes")
        .header("cookie", cookie)
        .header("sec-fetch-site", "same-origin")
        .accept_json()
        .await;
    assert_eq!(written.status, StatusCode::OK, "{}", written.body);
}

#[tokio::test]
async fn a_live_page_and_its_fragments_name_their_stream() {
    let server = start().await;
    let cookie = signed_in(&server, "/sign-in").await;

    let page = server.get("/members/notes").header("cookie", &cookie).await;
    let fragment = server
        .get("/members/notes")
        .header("cookie", &cookie)
        .htmx("notes")
        .await;

    let marker = format!(
        "<div id=\"sofie-live\" hidden data-stream=\"{}&amp;until=",
        unsealed_stream_path()
    );
    // Inside the body, wherever else the end of it gets; the development toolbar goes there too.
    let at = page.body.find(&marker);
    assert!(
        at.is_some_and(|at| page.body[at..].contains("</body>")),
        "{}",
        page.body
    );
    assert!(
        fragment
            .body
            .contains("id=\"sofie-live\" hidden hx-swap-oob=\"true\""),
        "{}",
        fragment.body
    );
}

#[tokio::test]
async fn a_stream_is_refused_to_a_stranger_another_site_and_an_unknown_area() {
    let server = start().await;
    let cookie = signed_in(&server, "/sign-in").await;
    let cross_site = format!("Cookie: {cookie}\r\nSec-Fetch-Site: cross-site\r\n");
    let same_site = format!("Cookie: {cookie}\r\nSec-Fetch-Site: same-origin\r\n");
    let sealed = stream_path(&server, &cookie).await;

    for (path, headers, status) in [
        (sealed.clone(), String::new(), "401"),
        (sealed, cross_site, "403"),
        (unsealed_stream_path(), same_site.clone(), "400"),
        (
            "/_sofie/live/members?topics=*&seal=AAAA".to_owned(),
            same_site.clone(),
            "400",
        ),
        (
            "/_sofie/live/nowhere?topics=*".to_owned(),
            String::new(),
            "404",
        ),
        ("/_sofie/live/members?topics=x".to_owned(), same_site, "400"),
    ] {
        let mut stream = Stream::open(&server, &path, &headers).await;
        assert!(stream.heard(&format!("HTTP/1.1 {status}")).await, "{path}");
    }
}

#[tokio::test]
async fn a_stream_hears_only_changes_its_principal_may_read() {
    let server = start().await;
    let member = signed_in(&server, "/sign-in").await;
    let stranger = signed_in(&server, "/sign-in/stranger").await;
    let mut members_stream = Stream::signed_in(&server, &member).await;
    let mut strangers_stream = Stream::signed_in(&server, &stranger).await;

    write_note(&server, &member).await;

    assert!(members_stream.heard("data: changed").await);
    assert!(!strangers_stream.heard("data: changed").await);
    write_note(&server, &stranger).await;
    assert!(strangers_stream.heard("data: changed").await);
}

#[tokio::test]
async fn a_principal_holds_a_few_streams_and_a_closed_one_frees_its_place() {
    let server = start().await;
    let member = signed_in(&server, "/sign-in").await;
    let mut held = Vec::new();
    for _ in 0..8 {
        held.push(Stream::signed_in(&server, &member).await);
    }
    let headers = format!("Cookie: {member}\r\nSec-Fetch-Site: same-origin\r\n");

    let path = stream_path(&server, &member).await;

    let mut refused = Stream::open(&server, &path, &headers).await;
    let refusal = refused.heard("429").await;
    drop(held.pop());
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut admitted = Stream::open(&server, &path, &headers).await;

    assert!(refusal, "a ninth stream is refused");
    assert!(admitted.heard(": open").await, "{}", admitted.read);
}

#[tokio::test]
async fn stopping_the_server_ends_open_streams() {
    let server = start().await;
    let member = signed_in(&server, "/sign-in").await;
    let _stream = Stream::signed_in(&server, &member).await;

    let stopped = tokio::time::timeout(Duration::from_secs(3), server.stop()).await;

    assert!(matches!(stopped, Ok(Ok(()))), "{stopped:?}");
}

#[tokio::test]
async fn a_stream_sealed_for_one_principal_is_refused_to_another() {
    let server = start().await;
    let member = signed_in(&server, "/sign-in").await;
    let stranger = signed_in(&server, "/sign-in/stranger").await;
    let members_path = stream_path(&server, &member).await;
    let headers = format!("Cookie: {stranger}\r\nSec-Fetch-Site: same-origin\r\n");

    let mut stream = Stream::open(&server, &members_path, &headers).await;

    assert!(stream.heard("HTTP/1.1 400").await, "{}", stream.read);
}

#[tokio::test]
async fn a_stream_seal_expires_and_a_refetched_page_seals_it_again() {
    let clock = TestClock::starting_at(Timestamp::now());
    let server = start_at(&clock).await;
    let member = signed_in(&server, "/sign-in").await;
    let headers = format!("Cookie: {member}\r\nSec-Fetch-Site: same-origin\r\n");
    let old_path = stream_path(&server, &member).await;

    clock.advance(Duration::from_mins(71)).unwrap();
    let mut stale = Stream::open(&server, &old_path, &headers).await;
    let fresh_path = stream_path(&server, &member).await;
    let mut fresh = Stream::open(&server, &fresh_path, &headers).await;

    assert!(stale.heard("HTTP/1.1 400").await, "{}", stale.read);
    assert!(fresh.heard(": open").await, "{}", fresh.read);
}

#[tokio::test]
async fn a_page_names_the_same_stream_for_some_minutes() {
    let clock = TestClock::starting_at(Timestamp::from_second(1_800_000_000).unwrap());
    let server = start_at(&clock).await;
    let member = signed_in(&server, "/sign-in").await;

    let first = stream_path(&server, &member).await;
    clock.advance(Duration::from_secs(30)).unwrap();
    let second = stream_path(&server, &member).await;

    assert_eq!(first, second);
}

/// A page kept by the browser must not hold a seal that has run out.
#[cfg(all(feature = "etag", not(feature = "development")))]
#[tokio::test]
async fn a_live_page_s_tag_changes_with_its_seal() {
    let clock = TestClock::starting_at(Timestamp::from_second(1_800_000_000).unwrap());
    let server = start_at(&clock).await;
    let member = signed_in(&server, "/sign-in").await;
    let first = server.get("/members/notes").header("cookie", &member).await;
    let tag = first.header("etag");

    clock.advance(Duration::from_mins(71)).unwrap();
    let again = server
        .get("/members/notes")
        .header("cookie", &member)
        .header("if-none-match", tag)
        .await;

    assert_eq!(again.status, StatusCode::OK);
    assert_ne!(stream_path(&server, &member).await, {
        let (_, rest) = first.body.split_once("data-stream=\"").unwrap();
        rest.split('"').next().unwrap().replace("&amp;", "&")
    });
}
