#![cfg(feature = "session-list")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use http::StatusCode;
use sofie_core::{
    Cast, ColumnReader, Error, Result, SqlValue, TestClock, Timestamp, Uuid, unpoisoned,
};
use sofie_db::{
    Aggregate, BoxFuture, Db, Dialect, Driver, Field, Id, Meta, Notifications, RowSink, Step,
    Topic, committed_changes, hears_other_instances,
};
use sofie_http::{
    Admitter, App, Area, AreaMembers, Routes, Session, SessionRecord, SignInRight, Web,
};
use sofie_testing::{FakeDriver, TestServer, TestSession};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

const MEMBER: Uuid = Uuid::from_u128(0x0190_a7d2_0000_7000_8000_0000_0000_0001);
const STRANGER: Uuid = Uuid::from_u128(0x0190_a7d2_0000_7000_8000_0000_0000_0002);
/// Not this process's: the listener skips its own instance's payloads.
const ANOTHER_INSTANCE: u64 = 1;

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

/// A session record of the store's table, owned by its account as sofie-identity's is.
struct Recorded {
    meta: Meta,
    account: Uuid,
}

impl Recorded {
    fn of(account: Uuid) -> Self {
        Self {
            meta: Meta::new().unwrap(),
            account,
        }
    }
}

impl Aggregate for Recorded {
    const NAME: &'static str = "recorded_session";
    const MODULE: &'static str = "sessions";
    const FIELDS: &'static [Field] = &[Field::new("account", Uuid::COLUMNS, false)];
    const OWNER: Option<&'static str> = Some("account");

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.account.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            account: Uuid::decode(row)?,
        })
    }

    fn owner(&self) -> Option<Uuid> {
        Some(self.account)
    }
}

/// Reads the database once per answer, so the driver's queries count what it was asked.
#[derive(Clone, Default)]
struct Store {
    ended: Arc<AtomicBool>,
}

impl Store {
    fn end(&self) {
        self.ended.store(true, Ordering::SeqCst);
    }
}

impl AreaMembers for Store {
    fn session_record<'a>(
        &'a self,
        db: &'a Db,
        _session: Uuid,
        _seen: Option<Timestamp>,
    ) -> BoxFuture<'a, Result<SessionRecord>> {
        Box::pin(async move {
            db.ping().await?;
            if self.ended.load(Ordering::SeqCst) {
                return Ok(SessionRecord::Ended);
            }
            Ok(SessionRecord::Live {
                account: MEMBER,
                expires_at: None,
                impersonator: None,
            })
        })
    }

    fn session_topic(&self) -> Option<Topic> {
        Some(Topic::of::<Recorded>())
    }
}

type Payloads = Result<Vec<String>>;

/// A fake database whose change notifications the test sends, and whose listening it ends
/// by sending an error; it never listens again.
#[derive(Clone)]
struct Listening {
    fake: FakeDriver,
    feed: Arc<Mutex<Option<UnboundedReceiver<Payloads>>>>,
}

impl Listening {
    fn new() -> (Self, UnboundedSender<Payloads>) {
        let (sender, feed) = unbounded_channel();
        let listening = Self {
            fake: FakeDriver::default(),
            feed: Arc::new(Mutex::new(Some(feed))),
        };
        (listening, sender)
    }
}

impl Driver for Listening {
    fn dialect(&self) -> &dyn Dialect {
        self.fake.dialect()
    }

    fn query<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [SqlValue],
        rows: &'a mut dyn RowSink,
    ) -> BoxFuture<'a, Result<()>> {
        self.fake.query(sql, params, rows)
    }

    fn transaction(&self, steps: Vec<Step>) -> BoxFuture<'_, Result<()>> {
        self.fake.transaction(steps)
    }

    fn change_notifications(&self) -> Option<BoxFuture<'_, Result<Box<dyn Notifications>>>> {
        Some(Box::pin(async move {
            let feed = unpoisoned::lock(&self.feed)
                .take()
                .ok_or_else(|| Error::internal_message("listening is down"))?;
            Ok(Box::new(Feed(feed)) as Box<dyn Notifications>)
        }))
    }
}

struct Feed(UnboundedReceiver<Payloads>);

impl Notifications for Feed {
    fn next(&mut self) -> BoxFuture<'_, Result<Vec<String>>> {
        Box::pin(async move {
            self.0
                .recv()
                .await
                .unwrap_or_else(|| Err(Error::internal_message("the feed closed")))
        })
    }
}

struct Tests;

impl Admitter for Tests {}

async fn sign_in(session: Session) -> Result<()> {
    session.sign_in::<Members>(SignInRight::of::<Tests>(), Id::from_uuid(MEMBER))?;
    Ok(())
}

async fn me() -> Result<()> {
    Ok(())
}

fn web_routes(routes: &mut Routes<Web>) {
    routes.post("/sign-in", sign_in);
}

fn member_routes(routes: &mut Routes<Members>) {
    routes.get("/me", me);
}

struct Running {
    server: TestServer,
    fake: FakeDriver,
    db: Db,
    clock: TestClock,
    store: Store,
}

async fn start() -> Running {
    let fake = FakeDriver::default();
    start_on(fake.clone(), fake).await
}

async fn start_on(driver: impl Driver, fake: FakeDriver) -> Running {
    let clock = TestClock::starting_at(Timestamp::from_second(1_700_000_000).unwrap());
    let db = Db::new(driver).with_clock(clock.clock());
    let store = Store::default();
    let app = App::new(db.clone())
        .membership(store.clone())
        .area("sessions", web_routes)
        .area("sessions", member_routes);
    Running {
        server: TestServer::start(app).await,
        fake,
        db,
        clock,
        store,
    }
}

impl Running {
    async fn signed_in(&self) -> TestSession {
        let answer = self.server.post("/sign-in").await;
        assert_eq!(answer.status, StatusCode::NO_CONTENT);
        answer.session().expect("signing in sets the cookie")
    }

    async fn me(&self, session: &TestSession) -> StatusCode {
        let answer = self
            .server
            .get("/members/me")
            .session(session)
            .accept_json()
            .await;
        answer.status
    }

    fn records_read(&self) -> usize {
        self.fake.queries().len()
    }

    /// Another instance's commit, told through the database.
    async fn commit_elsewhere(&self, account: Uuid) {
        let elsewhere = Db::new(self.fake.clone()).sharing_arrivals(&self.db);
        elsewhere.commit(&mut Recorded::of(account)).await.unwrap();
    }
}

/// Listening starts with the first reader; until it holds, nothing is cached.
async fn hearing(db: &Db) {
    drop(committed_changes(db));
    for _ in 0..500 {
        if hears_other_instances(db) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the listener never listened");
}

#[tokio::test]
async fn a_second_request_reads_no_session_record() {
    let running = start().await;
    let session = running.signed_in().await;

    assert_eq!(running.me(&session).await, StatusCode::NO_CONTENT);
    assert_eq!(running.me(&session).await, StatusCode::NO_CONTENT);

    assert_eq!(running.records_read(), 1, "the second came from the cache");
}

#[tokio::test]
async fn a_committed_change_drops_only_its_account_s_sessions() {
    let running = start().await;
    let session = running.signed_in().await;
    running.me(&session).await;

    running.commit_elsewhere(STRANGER).await;
    assert_eq!(running.me(&session).await, StatusCode::NO_CONTENT);
    assert_eq!(running.records_read(), 1, "another account's change");

    running.store.end();
    running.commit_elsewhere(MEMBER).await;
    assert_eq!(running.me(&session).await, StatusCode::UNAUTHORIZED);
    assert_eq!(running.records_read(), 2);
}

#[tokio::test]
async fn changes_committed_while_no_one_looks_do_not_empty_the_cache() {
    let running = start().await;
    let session = running.signed_in().await;
    running.me(&session).await;

    for _ in 0..1_000 {
        running.commit_elsewhere(STRANGER).await;
        tokio::task::yield_now().await;
    }
    running.me(&session).await;

    assert_eq!(running.records_read(), 1);
}

#[tokio::test]
async fn another_instance_s_notification_drops_the_account_s_sessions() {
    let (driver, feed) = Listening::new();
    let running = start_on(driver.clone(), driver.fake.clone()).await;
    hearing(&running.db).await;
    let session = running.signed_in().await;
    running.me(&session).await;
    let mut heard = committed_changes(&running.db);

    running.store.end();
    let payload = format!(
        "{ANOTHER_INSTANCE:016x} {}:{MEMBER}",
        Topic::of::<Recorded>()
    );
    feed.send(Ok(vec![payload])).unwrap();
    heard.recv().await.unwrap();

    assert_eq!(running.me(&session).await, StatusCode::UNAUTHORIZED);
    assert_eq!(running.records_read(), 2);
}

#[tokio::test]
async fn losing_the_listener_empties_the_cache() {
    let (driver, feed) = Listening::new();
    let running = start_on(driver.clone(), driver.fake.clone()).await;
    hearing(&running.db).await;
    let session = running.signed_in().await;
    running.me(&session).await;
    running.me(&session).await;
    assert_eq!(running.records_read(), 1);

    feed.send(Err(Error::internal_message("connection reset")))
        .unwrap();
    for _ in 0..500 {
        if !hears_other_instances(&running.db) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    running.me(&session).await;

    assert!(!hears_other_instances(&running.db));
    assert_eq!(
        running.records_read(),
        2,
        "a change told meanwhile would be missed"
    );
}

#[tokio::test]
async fn an_entry_is_trusted_for_a_minute() {
    let running = start().await;
    let session = running.signed_in().await;
    running.me(&session).await;

    running.clock.advance(Duration::from_secs(59)).unwrap();
    running.me(&session).await;
    assert_eq!(running.records_read(), 1);
    running.clock.advance(Duration::from_secs(1)).unwrap();
    running.me(&session).await;

    assert_eq!(running.records_read(), 2);
}

#[tokio::test]
async fn a_refreshed_cookie_of_an_ended_session_is_refused() {
    let running = start().await;
    let session = running.signed_in().await;
    running.clock.advance(Duration::from_mins(5)).unwrap();
    let refreshed = running
        .server
        .get("/members/me")
        .session(&session)
        .accept_json()
        .await
        .session()
        .expect("an active session is resealed every five minutes");

    running.store.end();
    running.commit_elsewhere(MEMBER).await;

    assert_eq!(running.me(&refreshed).await, StatusCode::UNAUTHORIZED);
    assert_eq!(running.me(&session).await, StatusCode::UNAUTHORIZED);
}
