#![cfg(all(feature = "remote-contracts", feature = "jobs"))]
//! Two apps in one process, each with its own database and server.

use std::sync::{Arc, Mutex};

use sofie::auth::{Acting, SigningKeys};
use sofie::core::{Error, Result, SqlValue};
use sofie::db::{
    Context, Db, Dep, Event, FeedSource, File, Id, Lookup, Migrations, Reach, Resolve, Uuid, Writes,
};
use sofie::http::{Admitter, App, Area, Principal, Routes, Session, SignInRight, Web};
use sofie::postgres::Postgres;
use sofie::prelude::*;
use sofie::remote::{Remote, RemoteApp, RemoteFeed, RemoteLookup, Served};
use sofie::worker::{Feeds, Worker};
use sofie_testing::{TestDatabase, TestServer};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

const MEMBERS: &str = "
CREATE TABLE member (
    id uuid PRIMARY KEY, version bigint NOT NULL,
    created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL,
    name text NOT NULL, city text NOT NULL
);
";
const SHOP: &str = "
CREATE TABLE member_copy (
    id uuid PRIMARY KEY, version bigint NOT NULL,
    created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL,
    name text NOT NULL, city text NOT NULL
);
";

#[derive(Aggregate)]
pub struct Member {
    meta: Meta,
    #[expose]
    name: String,
    #[expose]
    city: String,
}

#[derive(sofie::serde::Deserialize, sofie::serde::Serialize, Debug, PartialEq)]
#[serde(crate = "sofie::serde")]
pub struct MemberView {
    pub id: Uuid,
    pub name: String,
    pub city: String,
}

#[derive(Event)]
pub struct MemberChanged {
    pub id: Uuid,
    pub version: i64,
    pub name: String,
    pub city: String,
}

#[derive(Aggregate)]
#[mirror(Member)]
pub struct MemberCopy {
    meta: Meta,
    name: String,
    city: String,
}

#[sofie::contract]
pub trait Directory: Send + Sync {
    async fn city_of(&self, name: &str) -> Result<String>;

    async fn cities(&self, names: &[String], limit: i64) -> Result<Vec<String>>;
}

pub struct LocalDirectory {
    db: Db,
    acting: Arc<Mutex<Vec<Option<Acting>>>>,
}

#[sofie::contract]
impl Directory for LocalDirectory {
    async fn city_of(&self, name: &str) -> Result<String> {
        self.acting.lock().unwrap().push(sofie::remote::acting());
        let member = Member::query()
            .where_(Member::NAME.eq(name.to_owned()))
            .first(&self.db.read())
            .await?;
        member.map(|member| member.city).ok_or(Error::NotFound)
    }

    async fn cities(&self, names: &[String], limit: i64) -> Result<Vec<String>> {
        let mut cities = Vec::new();
        for name in names.iter().take(usize::try_from(limit).unwrap()) {
            cities.push(self.city_of(name).await?);
        }
        Ok(cities)
    }
}

struct TestContext(Db);

impl Context for TestContext {
    fn db(&self) -> &Db {
        &self.0
    }

    fn reach(&self) -> Reach {
        Reach::All
    }
}

impl Writes for TestContext {}

fn keys(byte: u8) -> SigningKeys {
    SigningKeys::new(&[[byte; 32]]).unwrap()
}

async fn database(tables: &'static str) -> (Db, TestDatabase) {
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    let migrations = Box::leak(Box::new([File::new("0001_tables.sql", tables.as_bytes())]));
    db.schema()
        .migrate(&[Migrations::new("remote", migrations)])
        .await
        .unwrap();
    (db, database)
}

struct MembersApp {
    db: Db,
    base: String,
    acting: Arc<Mutex<Vec<Option<Acting>>>>,
    stop: oneshot::Sender<()>,
    _database: TestDatabase,
}

impl MembersApp {
    async fn start() -> Self {
        let (db, database) = database(MEMBERS).await;
        let acting = Arc::new(Mutex::new(Vec::new()));
        let directory = LocalDirectory {
            db: db.clone(),
            acting: Arc::clone(&acting),
        };
        sofie::db::provide::<dyn Directory>(&db, Arc::new(directory));
        let lookup = sofie::db::Exposed::<Member, MemberView>::new(db.clone());
        sofie::db::provide::<dyn Lookup<MemberView>>(&db, Arc::new(lookup));
        Served::default()
            .contract::<dyn Directory>()
            .lookup::<MemberView>()
            .feed()
            .serve(&db);
        let app = App::new(db.clone()).serving(|serving| serving.signing_keys(keys(7)));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (stop, stopped) = oneshot::channel::<()>();
        tokio::spawn(app.serve(listener, async {
            let _ = stopped.await;
        }));
        Self {
            db,
            base,
            acting,
            stop,
            _database: database,
        }
    }

    fn reached_with(&self, keys: SigningKeys) -> RemoteApp {
        let settings = sofie_client::Vendor {
            base_url: Some(self.base.clone()),
            private: Some(true),
            ..sofie_client::Vendor::default()
        };
        RemoteApp::new(
            "members",
            sofie_client::for_vendor("members", settings).unwrap(),
            keys,
        )
    }

    fn worker(&self) -> Worker {
        Feeds::default()
            .publish(MemberChanged::NAME)
            .attach(Worker::new(self.db.clone()))
    }

    async fn join(&self, name: &str, city: &str) -> Member {
        let mut member = Member {
            meta: Meta::new().unwrap(),
            name: name.to_owned(),
            city: city.to_owned(),
        };
        self.db.commit(&mut member).await.unwrap();
        member
    }
}

struct NoMembers;

impl Lookup<Member> for NoMembers {
    fn get_many<'a>(&'a self, _: &'a [Uuid]) -> sofie::db::BoxFuture<'a, Result<Vec<Member>>> {
        Box::pin(std::future::ready(Ok(Vec::new())))
    }

    fn changes(
        &self,
        _: Option<Uuid>,
        _: u64,
    ) -> sofie::db::BoxFuture<
        '_,
        Result<Vec<sofie::serde_json::Map<String, sofie::serde_json::Value>>>,
    > {
        Box::pin(std::future::ready(Ok(Vec::new())))
    }
}

fn shop_worker(db: &Db, members: RemoteApp) -> Worker {
    sofie::db::provide::<dyn Lookup<Member>>(db, Arc::new(NoMembers));
    let worker = Worker::new(db.clone()).mirror::<MemberCopy>("remote.copy_member");
    let members = Arc::new(RemoteFeed::new(members)) as Arc<dyn FeedSource>;
    Feeds::default().pull([members]).attach(worker)
}

async fn rows(database: &TestDatabase, sql: &str) -> Vec<Vec<SqlValue>> {
    let raw = Postgres::connect(database.url()).unwrap();
    sofie::db::query_values(&raw, sql, &[]).await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_contract_is_called_across_apps_for_the_caller_s_principal() {
    let members = MembersApp::start().await;
    members.join("Ada", "London").await;
    let (shop, _database) = database(SHOP).await;
    let remote = Remote::<dyn Directory>::new(members.reached_with(keys(7)));
    sofie::db::provide::<dyn Directory>(&shop, Arc::new(remote));
    let directory = Dep::<dyn Directory>::resolve(&mut TestContext(shop.clone()))
        .await
        .unwrap();
    let customer = Acting {
        area: "customer".to_owned(),
        principal: Uuid::now_v7().unwrap(),
    };

    let city = sofie::remote::acting_as(Some(customer.clone()), directory.city_of("Ada")).await;
    let cities = directory
        .cities(&["Ada".to_owned(), "Ada".to_owned()], 1)
        .await;
    let missing = directory.city_of("Grace").await;

    assert_eq!(city.unwrap(), "London");
    assert_eq!(cities.unwrap(), ["London"]);
    assert!(matches!(missing, Err(Error::NotFound)), "{missing:?}");
    assert_eq!(members.acting.lock().unwrap()[..2], [Some(customer), None]);
    let _ = members.stop.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_signed_with_other_keys_is_refused() {
    let members = MembersApp::start().await;
    let remote = Remote::<dyn Directory>::new(members.reached_with(keys(8)));

    let refused = remote.city_of("Ada").await.unwrap_err().to_string();

    assert!(refused.contains("401"), "{refused}");
    assert!(members.acting.lock().unwrap().is_empty());
    let _ = members.stop.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_batch_lookup_crosses_apps_in_one_call() {
    let members = MembersApp::start().await;
    let ada = members.join("Ada", "London").await;
    let grace = members.join("Grace", "New York").await;
    let lookup = RemoteLookup::<MemberView>::new(members.reached_with(keys(7)));

    let mut found = lookup
        .get_many(&[grace.id().uuid(), Uuid::now_v7().unwrap(), ada.id().uuid()])
        .await
        .unwrap();
    found.sort_by(|left, right| left.name.cmp(&right.name));

    let names: Vec<&str> = found.iter().map(|member| member.name.as_str()).collect();
    assert_eq!(names, ["Ada", "Grace"]);
    assert_eq!(found[1].city, "New York");
    let _ = members.stop.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_event_is_pulled_from_the_feed_and_delivered_once() {
    let members = MembersApp::start().await;
    let mut ada = members.join("Ada", "London").await;
    ada.city = "Paris".to_owned();
    members.db.commit(&mut ada).await.unwrap();
    members.worker().drain().await.unwrap();
    let (shop, shop_database) = database(SHOP).await;
    let worker = shop_worker(&shop, members.reached_with(keys(7)));

    let delivered = worker.drain().await.unwrap();
    let again = worker.drain().await.unwrap();

    assert_eq!(
        (delivered, again),
        (4, 0),
        "two changes, the backfill and the sweep"
    );
    let copy = rows(
        &shop_database,
        "SELECT version, name, city FROM remote.member_copy",
    )
    .await;
    assert_eq!(
        copy,
        [[
            SqlValue::Int(2),
            SqlValue::Text("Ada".to_owned()),
            SqlValue::Text("Paris".to_owned())
        ]]
    );
    let cursor = sofie::db::feed_cursor(&shop, "members").await.unwrap();
    let replayed = sofie::db::read_feed(&members.db, 0, 10).await.unwrap();
    let racing = sofie::db::receive_feed(&shop, "members", 0, &replayed).await;
    assert_eq!(cursor, replayed.last().unwrap().position);
    assert!(matches!(racing, Err(Error::Conflict)), "{racing:?}");
    let _ = members.stop.send(());
}

const CUSTOMER: Uuid = Uuid::from_u128(0x0190_a7d2_0000_7000_8000_0000_0000_0001);

struct Customers;

impl Area for Customers {
    const NAME: &'static str = "customer";
    const PREFIX: &'static str = "/account";
    const SIGN_IN: &'static str = "/sign-in";
    type Principal = Member;
}

/// Signs in as the test's own admitter.
struct Tests;

impl Admitter for Tests {}

const RIGHT: SignInRight = SignInRight::of::<Tests>();

async fn sign_in(session: Session) -> Result<()> {
    session.sign_in::<Customers>(RIGHT, Id::from_uuid(CUSTOMER))?;
    Ok(())
}

async fn ask_city(directory: Dep<dyn Directory>, _me: Principal<Customers>) -> Result<()> {
    directory.city_of("Ada").await.map(drop)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_handler_s_contract_call_acts_for_its_signed_in_principal() {
    let members = MembersApp::start().await;
    members.join("Ada", "London").await;
    let (shop, _database) = database(SHOP).await;
    let remote = Remote::<dyn Directory>::new(members.reached_with(keys(7)));
    sofie::db::provide::<dyn Directory>(&shop, Arc::new(remote));
    let app = App::new(shop)
        .area("remote", |routes: &mut Routes<Web>| {
            routes.post("/sign-in", sign_in);
        })
        .area("remote", |routes: &mut Routes<Customers>| {
            routes.post("/city", ask_city);
        });
    let server = TestServer::start(app).await;
    let session = server.post("/sign-in").await.session().unwrap();

    server
        .post("/account/city")
        .session(&session)
        .await
        .assert_status(204);

    let customer = Acting {
        area: "customer".to_owned(),
        principal: CUSTOMER,
    };
    assert_eq!(*members.acting.lock().unwrap(), [Some(customer)]);
    let _ = members.stop.send(());
}

struct OneEntry(sofie::db::FeedEntry);

impl FeedSource for OneEntry {
    fn producer(&self) -> &'static str {
        "members"
    }

    fn after(
        &self,
        position: i64,
        _limit: i64,
    ) -> sofie::db::BoxFuture<'_, Result<Vec<sofie::db::FeedEntry>>> {
        let entries = if position < self.0.position {
            vec![self.0.clone()]
        } else {
            Vec::new()
        };
        Box::pin(std::future::ready(Ok(entries)))
    }
}

async fn note_change(_change: MemberChanged) -> Result<()> {
    Ok(())
}

#[tokio::test]
async fn a_pulled_event_fans_out_to_a_listener_another_worker_of_the_process_has() {
    let (shop, shop_database) = database(SHOP).await;
    let entry = sofie::db::FeedEntry {
        position: 1,
        id: Uuid::now_v7().unwrap(),
        name: MemberChanged::NAME.to_owned(),
        payload: sofie::serde_json::json!({
            "id": Uuid::now_v7().unwrap(), "version": 1, "name": "Ada", "city": "Paris"
        }),
        traceparent: None,
    };
    let hearing =
        Worker::new(shop.clone()).on::<MemberChanged, _>("remote.note_change", note_change);
    sofie::worker::publish_heard(&hearing);
    let source = Arc::new(OneEntry(entry.clone())) as Arc<dyn FeedSource>;
    let pulling = Feeds::default()
        .pull([source])
        .attach(Worker::new(shop.clone()));

    pulling.drain().await.unwrap();

    let queued = rows(&shop_database, "SELECT name, event_id FROM sofie.messages").await;
    assert_eq!(
        queued,
        [[
            SqlValue::Text("remote.note_change".to_owned()),
            SqlValue::Uuid(entry.id)
        ]]
    );
    assert_eq!(hearing.drain().await.unwrap(), 1);
}
