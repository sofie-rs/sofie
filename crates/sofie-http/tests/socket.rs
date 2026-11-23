#![cfg(feature = "socket")]

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use http::StatusCode;
use serde_json::{Value, json};
use sofie_core::{Cast, ColumnReader, Result, SqlValue, Uuid};
use sofie_db::{Aggregate, Db, Field, Id, Meta, Outcome, Read, ReadOnly, Reply, Select};
use sofie_http::{Admitter, App, Area, Principal, Routes, Session, SignInRight, Web};
use sofie_testing::{FakeDriver, TestServer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const MEMBER: Uuid = Uuid::from_u128(0x0190_a7d2_0000_7000_8000_0000_0000_0001);
const STRANGER: Uuid = Uuid::from_u128(0x0190_a7d2_0000_7000_8000_0000_0000_0002);
const KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";
const TEXT: u8 = 0x1;
const CLOSE: u8 = 0x8;
const PING: u8 = 0x9;
/// Each load answers differently, as the fake database keeps no rows a write would add.
static LOADS: AtomicU64 = AtomicU64::new(0);

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
        let load = LOADS.fetch_add(1, Ordering::Relaxed);
        Ok(Reply::Json(json!({ "count": self.0.len(), "load": load })))
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
    routes.get("/notes", list_notes);
    routes.post("/notes", write_mine);
}

async fn start() -> TestServer {
    start_on(FakeDriver::default()).await
}

async fn start_on(driver: FakeDriver) -> TestServer {
    let app = App::new(Db::new(driver))
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

async fn write_note(server: &TestServer, cookie: &str) {
    let written = server
        .post("/members/notes")
        .header("cookie", cookie)
        .header("sec-fetch-site", "same-origin")
        .accept_json()
        .await;
    assert_eq!(written.status, StatusCode::OK, "{}", written.body);
}

struct Client {
    socket: TcpStream,
    head: String,
}

impl Client {
    async fn open(server: &TestServer, headers: &str) -> Self {
        let address = server.origin().trim_start_matches("http://").to_owned();
        let mut socket = TcpStream::connect(&address).await.unwrap();
        let request = format!(
            "GET /_sofie/socket HTTP/1.1\r\nHost: {address}\r\nUpgrade: websocket\r\n\
             Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n{headers}\r\n"
        );
        socket.write_all(request.as_bytes()).await.unwrap();
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            if socket.read(&mut byte).await.unwrap() == 0 {
                break;
            }
            head.push(byte[0]);
        }
        Self {
            socket,
            head: String::from_utf8(head).unwrap(),
        }
    }

    async fn signed_in(server: &TestServer, cookie: &str) -> Self {
        let origin = server.origin();
        let headers =
            format!("Sec-WebSocket-Key: {KEY}\r\nOrigin: {origin}\r\nCookie: {cookie}\r\n");
        let client = Self::open(server, &headers).await;
        assert!(client.head.starts_with("HTTP/1.1 101"), "{}", client.head);
        client
    }

    async fn send(&mut self, message: &Value) {
        let payload = message.to_string().into_bytes();
        let mask = [0x11, 0x22, 0x33, 0x44];
        let mut frame = vec![0x80 | TEXT];
        let length = u8::try_from(payload.len()).expect("a short test message");
        assert!(length < 126);
        frame.push(0x80 | length);
        frame.extend_from_slice(&mask);
        frame.extend(
            payload
                .iter()
                .enumerate()
                .map(|(index, byte)| byte ^ mask[index % 4]),
        );
        self.socket.write_all(&frame).await.unwrap();
    }

    /// The next frame other than a ping, within a second.
    async fn frame(&mut self) -> Option<(u8, Vec<u8>)> {
        let reading = async {
            loop {
                let mut head = [0; 2];
                self.socket.read_exact(&mut head).await.ok()?;
                let length = match head[1] & 0x7F {
                    126 => usize::from(self.socket.read_u16().await.ok()?),
                    127 => usize::try_from(self.socket.read_u64().await.ok()?).ok()?,
                    short => usize::from(short),
                };
                let mut payload = vec![0; length];
                self.socket.read_exact(&mut payload).await.ok()?;
                if head[0] & 0x0F != PING {
                    return Some((head[0] & 0x0F, payload));
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(1), reading)
            .await
            .ok()
            .flatten()
    }

    async fn answer(&mut self) -> Option<Value> {
        let (opcode, payload) = self.frame().await?;
        assert_eq!(opcode, TEXT, "{payload:?}");
        Some(serde_json::from_slice(&payload).unwrap())
    }

    async fn closed_with(&mut self) -> Option<u16> {
        let (opcode, payload) = self.frame().await?;
        (opcode == CLOSE).then(|| u16::from_be_bytes([payload[0], payload[1]]))
    }
}

#[tokio::test]
async fn a_handshake_is_answered_with_its_accept_key() {
    let server = start().await;
    let member = signed_in(&server, "/sign-in").await;

    let client = Client::signed_in(&server, &member).await;

    let head = client.head.to_ascii_lowercase();
    assert!(head.contains("upgrade: websocket"), "{head}");
    assert!(
        head.contains("sec-websocket-accept: s3pplmbitxaq9kygzzhzrbk+xoo="),
        "{head}"
    );
}

#[tokio::test]
async fn a_socket_is_refused_to_another_site_and_to_a_cookie_without_an_origin() {
    let server = start().await;
    let member = signed_in(&server, "/sign-in").await;
    let origin = server.origin();

    for (headers, status) in [
        (
            format!(
                "Sec-WebSocket-Key: {KEY}\r\nOrigin: https://elsewhere.test\r\nCookie: {member}\r\n"
            ),
            "403",
        ),
        (
            format!("Sec-WebSocket-Key: {KEY}\r\nCookie: {member}\r\n"),
            "403",
        ),
        (format!("Origin: {origin}\r\n"), "400"),
        (format!("Sec-WebSocket-Key: {KEY}\r\n"), "101"),
    ] {
        let client = Client::open(&server, &headers).await;
        assert!(
            client.head.starts_with(&format!("HTTP/1.1 {status}")),
            "{headers}: {}",
            client.head
        );
    }
}

#[tokio::test]
async fn a_watched_get_is_answered_again_when_its_principal_writes_what_it_read() {
    let server = start().await;
    let member = signed_in(&server, "/sign-in").await;
    let stranger = signed_in(&server, "/sign-in/stranger").await;
    let mut client = Client::signed_in(&server, &member).await;

    client
        .send(&json!({"id": 1, "method": "GET", "path": "/members/notes"}))
        .await;
    let first = client.answer().await;
    write_note(&server, &stranger).await;
    let after_strangers = client.answer().await;
    write_note(&server, &member).await;
    let after_own = client.answer().await;

    let status = |answer: Option<Value>| {
        answer.map(|answer| (answer["id"].clone(), answer["status"].clone()))
    };
    assert_eq!(status(first), Some((json!(1), json!(200))));
    assert_eq!(after_strangers, None);
    assert_eq!(status(after_own), Some((json!(1), json!(200))));
}

#[tokio::test]
async fn a_stopped_watch_hears_no_more() {
    let server = start().await;
    let member = signed_in(&server, "/sign-in").await;
    let mut client = Client::signed_in(&server, &member).await;
    client
        .send(&json!({"id": 1, "method": "GET", "path": "/members/notes"}))
        .await;
    assert!(client.answer().await.is_some());

    client.send(&json!({"stop": 1})).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    write_note(&server, &member).await;

    assert_eq!(client.answer().await, None);
}

#[tokio::test]
async fn a_call_runs_through_its_route_as_the_socket_s_principal() {
    let driver = FakeDriver::default();
    let server = start_on(driver.clone()).await;
    let member = signed_in(&server, "/sign-in").await;
    let mut signed_in = Client::signed_in(&server, &member).await;
    let origin = server.origin();
    let mut anyone = Client::open(
        &server,
        &format!("Sec-WebSocket-Key: {KEY}\r\nOrigin: {origin}\r\n"),
    )
    .await;

    signed_in
        .send(&json!({"id": 7, "method": "POST", "path": "/members/notes", "body": {}}))
        .await;
    anyone
        .send(&json!({"id": 8, "method": "GET", "path": "/members/notes"}))
        .await;
    let written = signed_in.answer().await.unwrap();
    let refused = anyone.answer().await.unwrap();

    assert_eq!(
        (written["id"].as_u64(), written["status"].as_u64()),
        (Some(7), Some(200)),
        "{written}"
    );
    assert_eq!(
        (refused["id"].as_u64(), refused["status"].as_u64()),
        (Some(8), Some(401)),
        "{refused}"
    );
    assert_eq!(driver.transaction_count(), 1);
}

#[tokio::test]
async fn a_path_of_another_host_is_refused_and_a_malformed_message_closes_the_socket() {
    let server = start().await;
    let member = signed_in(&server, "/sign-in").await;
    let mut client = Client::signed_in(&server, &member).await;

    client
        .send(&json!({"id": 1, "method": "GET", "path": "//elsewhere.test/"}))
        .await;
    let refused = client.answer().await.unwrap();
    client.send(&json!({"hello": true})).await;

    assert_eq!(refused["status"], 400, "{refused}");
    assert_eq!(client.closed_with().await, Some(1008));
}

#[tokio::test]
async fn a_principal_holds_a_few_sockets() {
    let server = start().await;
    let member = signed_in(&server, "/sign-in").await;
    let mut held = Vec::new();
    for _ in 0..8 {
        held.push(Client::signed_in(&server, &member).await);
    }
    let origin = server.origin();
    let headers = format!("Sec-WebSocket-Key: {KEY}\r\nOrigin: {origin}\r\nCookie: {member}\r\n");

    let refused = Client::open(&server, &headers).await;

    assert!(refused.head.starts_with("HTTP/1.1 429"), "{}", refused.head);
}

#[tokio::test]
async fn stopping_the_server_closes_open_sockets() {
    let server = start().await;
    let member = signed_in(&server, "/sign-in").await;
    let mut client = Client::signed_in(&server, &member).await;

    let stopped = tokio::time::timeout(Duration::from_secs(3), server.stop()).await;

    assert!(matches!(stopped, Ok(Ok(()))), "{stopped:?}");
    assert_eq!(client.closed_with().await, Some(1001));
}
