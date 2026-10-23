#![cfg(all(feature = "contracts", feature = "jobs", feature = "idempotency"))]

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use sofie::core::{Result, SqlValue};
use sofie::db::{Context, Event, Resolve};
use sofie::db::{
    Db, Dep, Exposed, Extensions, File, IdempotencyKey, Jobs, Lookup, Migrations, Queued, Reach,
    Uuid, Writes,
};
use sofie::postgres::Postgres;
use sofie::prelude::*;
use sofie::serde_json::json;
use sofie::worker::Worker;
use sofie_testing::TestDatabase;

const TABLES: &str = "
CREATE TABLE member (
    id uuid PRIMARY KEY, version bigint NOT NULL,
    created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL,
    name text NOT NULL, city text NOT NULL, note text NOT NULL
);
CREATE TABLE member_copy (
    id uuid PRIMARY KEY, version bigint NOT NULL,
    created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL,
    name text NOT NULL, city text NOT NULL
);
CREATE TABLE place (
    id uuid PRIMARY KEY, version bigint NOT NULL,
    created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL,
    deleted_at timestamptz, name text NOT NULL
);
CREATE TABLE place_copy (
    id uuid PRIMARY KEY, version bigint NOT NULL,
    created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL,
    deleted_at timestamptz, name text NOT NULL
);
";
const MIGRATIONS: &[File] = &[File::new("0001_tables.sql", TABLES.as_bytes())];

#[derive(Aggregate)]
pub struct Member {
    meta: Meta,
    #[expose]
    name: String,
    #[expose]
    city: String,
    note: String,
}

impl Member {
    fn new(name: &str, city: &str) -> Self {
        Self {
            meta: Meta::new().unwrap(),
            name: name.to_owned(),
            city: city.to_owned(),
            note: "kept to the module".to_owned(),
        }
    }
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

#[cfg(feature = "soft-delete")]
#[derive(Aggregate)]
#[aggregate(behaviors(SoftDelete))]
pub struct Place {
    meta: Meta,
    #[expose]
    name: String,
}

/// As `sofie gen` writes a contract's types.
#[cfg(feature = "soft-delete")]
mod place_contract {
    use sofie::db::Uuid;
    use sofie::prelude::*;

    #[derive(sofie::serde::Deserialize, sofie::serde::Serialize)]
    #[serde(crate = "sofie::serde")]
    pub struct Place {
        pub id: Uuid,
        pub name: String,
    }

    #[derive(Event)]
    pub struct PlaceChanged {
        pub id: Uuid,
        pub version: i64,
        pub name: String,
        pub deleted_at: Option<sofie::core::Instant>,
    }
}

#[cfg(feature = "soft-delete")]
#[derive(Aggregate)]
#[aggregate(behaviors(SoftDelete))]
#[mirror(place_contract::Place)]
pub struct PlaceCopy {
    meta: Meta,
    name: String,
}

#[derive(Aggregate)]
pub struct HTTPLog {
    meta: Meta,
}

struct Setup {
    db: Db,
    raw: Postgres,
    _database: TestDatabase,
}

impl Setup {
    async fn new() -> Self {
        let database = TestDatabase::create().await;
        let db = Db::new(Postgres::connect(database.url()).unwrap());
        db.schema()
            .migrate(&[Migrations::new("contracts", MIGRATIONS)])
            .await
            .unwrap();
        Self {
            db,
            raw: Postgres::connect(database.url()).unwrap(),
            _database: database,
        }
    }

    async fn rows(&self, sql: &str) -> Vec<Vec<SqlValue>> {
        sofie::db::query_values(&self.raw, sql, &[]).await.unwrap()
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

async fn resolved<T: Resolve<TestContext>>(db: &Db) -> Result<T> {
    T::resolve(&mut TestContext(db.clone())).await
}

fn text(value: &str) -> SqlValue {
    SqlValue::Text(value.to_owned())
}

#[test]
fn a_change_event_is_named_after_the_type_as_written() {
    assert_eq!(HTTPLog::CHANGED, Some("contracts.HTTPLogChanged"));
    assert_eq!(Member::CHANGED, Some(MemberChanged::NAME));
}

#[tokio::test]
async fn a_change_publishes_the_exposed_fields_and_nothing_else() {
    let setup = Setup::new().await;
    let mut member = Member::new("Ada", "London");
    setup.db.commit(&mut member).await.unwrap();
    member.city = "Paris".to_owned();
    setup.db.commit(&mut member).await.unwrap();

    let messages = setup
        .rows("SELECT name, payload FROM sofie.messages ORDER BY payload->>'version'")
        .await;

    let id = member.id().to_string();
    let changed = |version: i64, city: &str| json!({ "id": id, "version": version, "name": "Ada", "city": city });
    assert_eq!(
        messages,
        [
            [
                text("contracts.MemberChanged"),
                SqlValue::Json(changed(1, "London"))
            ],
            [
                text("contracts.MemberChanged"),
                SqlValue::Json(changed(2, "Paris"))
            ],
        ]
    );
}

#[tokio::test]
async fn a_batch_lookup_reads_the_exposed_fields_of_many_records_at_once() {
    let setup = Setup::new().await;
    let mut ada = Member::new("Ada", "London");
    let mut grace = Member::new("Grace", "New York");
    setup.db.commit(&mut ada).await.unwrap();
    setup.db.commit(&mut grace).await.unwrap();
    let lookup = Exposed::<Member, MemberView>::new(setup.db.clone());
    sofie::db::provide::<dyn Lookup<MemberView>>(&setup.db, Arc::new(lookup));
    let members: Dep<dyn Lookup<MemberView>> = resolved(&setup.db).await.unwrap();

    let ids = [grace.id().uuid(), Uuid::now_v7().unwrap(), ada.id().uuid()];
    let mut found = members.get_many(&ids).await.unwrap();
    found.sort_by(|left, right| left.name.cmp(&right.name));

    assert_eq!(
        found,
        [
            MemberView {
                id: ada.id().uuid(),
                name: "Ada".to_owned(),
                city: "London".to_owned(),
            },
            MemberView {
                id: grace.id().uuid(),
                name: "Grace".to_owned(),
                city: "New York".to_owned(),
            },
        ]
    );
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

#[tokio::test]
async fn a_mirror_follows_its_source_and_ignores_an_older_change() {
    let setup = Setup::new().await;
    sofie::db::provide::<dyn Lookup<Member>>(&setup.db, Arc::new(NoMembers));
    let worker = Worker::new(setup.db.clone()).mirror::<MemberCopy>("contracts.MemberCopy");
    let mut member = Member::new("Ada", "London");
    setup.db.commit(&mut member).await.unwrap();
    member.city = "Paris".to_owned();
    setup.db.commit(&mut member).await.unwrap();

    worker.drain().await.unwrap();
    let stale = json!({ "id": member.id(), "version": 1, "name": "Ada", "city": "London" });
    setup
        .rows(&format!(
            "INSERT INTO sofie.messages (id, kind, name, payload, available_at) \
             VALUES (gen_random_uuid(), 'event', 'contracts.MemberChanged', '{stale}', now())"
        ))
        .await;
    worker.drain().await.unwrap();

    let copy = setup
        .rows("SELECT version, name, city FROM contracts.member_copy")
        .await;
    assert_eq!(copy, [[SqlValue::Int(2), text("Ada"), text("Paris")]]);
}

#[cfg(feature = "soft-delete")]
#[tokio::test]
async fn a_new_mirror_copies_its_source_once_then_hides_what_the_source_deletes() {
    use sofie::db::SoftDeletes;

    let setup = Setup::new().await;
    let place = |name: &str| Place {
        meta: Meta::new().unwrap(),
        name: name.to_owned(),
    };
    let (mut kept, mut gone) = (place("Kept"), place("Gone"));
    setup.db.commit(&mut kept).await.unwrap();
    setup.db.commit(&mut gone).await.unwrap();
    Worker::new(setup.db.clone()).drain().await.unwrap();
    sofie::db::provide::<dyn Lookup<place_contract::Place>>(
        &setup.db,
        Arc::new(Exposed::<Place, place_contract::Place>::new(
            setup.db.clone(),
        )),
    );
    let worker = Worker::new(setup.db.clone()).mirror::<PlaceCopy>("contracts.PlaceCopy");
    let copies = "SELECT name, deleted_at IS NULL FROM contracts.place_copy ORDER BY name";

    worker.drain().await.unwrap();
    assert_eq!(
        setup.rows(copies).await,
        [
            [text("Gone"), SqlValue::Bool(true)],
            [text("Kept"), SqlValue::Bool(true)]
        ]
    );
    gone.delete();
    setup.db.commit(&mut gone).await.unwrap();
    worker.drain().await.unwrap();

    assert_eq!(
        setup.rows(copies).await,
        [
            [text("Gone"), SqlValue::Bool(false)],
            [text("Kept"), SqlValue::Bool(true)]
        ]
    );
    let again = Worker::new(setup.db.clone()).mirror::<PlaceCopy>("contracts.PlaceCopy");
    assert_eq!(again.drain().await.unwrap(), 0, "the copy runs once");
}

#[sofie::contract]
pub trait Directory: Send + Sync {
    async fn city_of(&self, name: &str) -> Result<String>;
}

pub struct LocalDirectory(Db);

#[sofie::contract]
impl Directory for LocalDirectory {
    async fn city_of(&self, name: &str) -> Result<String> {
        let member = Member::query()
            .where_(Member::NAME.eq(name.to_owned()))
            .first(&self.0.read())
            .await?;
        Ok(member.map(|member| member.city).unwrap_or_default())
    }
}

pub struct Greeting(&'static str);

#[sofie::contract]
impl Directory for Greeting {
    async fn city_of(&self, _name: &str) -> Result<String> {
        Ok(self.0.to_owned())
    }
}

#[tokio::test]
async fn a_contract_and_its_extensions_resolve_from_what_the_wiring_provides() {
    let setup = Setup::new().await;
    let missing = resolved::<Dep<dyn Directory>>(&setup.db).await;
    assert!(missing.is_err(), "nothing provided yet");
    let mut ada = Member::new("Ada", "London");
    setup.db.commit(&mut ada).await.unwrap();
    sofie::db::provide::<dyn Directory>(&setup.db, Arc::new(LocalDirectory(setup.db.clone())));
    sofie::db::extend::<dyn Directory>(&setup.db, Arc::new(Greeting("first")));
    sofie::db::extend::<dyn Directory>(&setup.db, Arc::new(Greeting("second")));

    let directory: Dep<dyn Directory> = resolved(&setup.db).await.unwrap();
    let extensions: Extensions<dyn Directory> = resolved(&setup.db).await.unwrap();

    assert_eq!(directory.city_of("Ada").await.unwrap(), "London");
    let mut answers = Vec::new();
    for extension in extensions.iter() {
        answers.push(extension.city_of("Ada").await.unwrap());
    }
    assert_eq!(answers, ["first", "second"]);
}

static KEYS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

#[job]
async fn rename(mut member: Member, key: IdempotencyKey) -> Result<Member> {
    KEYS.lock().unwrap().push(key.as_str().to_owned());
    "Ada Lovelace".clone_into(&mut member.name);
    Ok(member)
}

async fn queue_rename(jobs: Jobs, member: &Member) -> Result<Queued> {
    jobs.queue(rename, &json!({ "member": member.id() }))
}

#[tokio::test]
async fn a_queued_job_runs_once_with_its_message_as_the_key() {
    let setup = Setup::new().await;
    sofie::db::name_job(&setup.db, "contracts.rename", rename);
    let worker = Worker::new(setup.db.clone()).job("contracts.rename", rename);
    let mut member = Member::new("Ada", "London");
    setup.db.commit(&mut member).await.unwrap();
    let jobs: Jobs = resolved(&setup.db).await.unwrap();
    let mut queued = queue_rename(jobs, &member).await.unwrap();
    setup.db.commit(&mut queued).await.unwrap();
    let message = setup
        .rows("SELECT id::text FROM sofie.messages WHERE name = 'contracts.rename'")
        .await;

    assert_eq!(worker.drain().await.unwrap(), 1);

    let renamed = setup.rows("SELECT name FROM contracts.member").await;
    assert_eq!(renamed, [[text("Ada Lovelace")]]);
    let SqlValue::Text(message) = &message[0][0] else {
        panic!("{message:?}")
    };
    assert_eq!(*KEYS.lock().unwrap(), std::slice::from_ref(message));
}

#[tokio::test]
async fn a_function_not_marked_as_a_job_is_refused() {
    let setup = Setup::new().await;
    let jobs: Jobs = resolved(&setup.db).await.unwrap();

    let refused = jobs.queue(rename, &json!({}));

    assert!(refused.is_err());
}

static RELEASED: AtomicBool = AtomicBool::new(false);
static QUICK: AtomicUsize = AtomicUsize::new(0);

async fn slow() -> Result<()> {
    while !RELEASED.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Ok(())
}

async fn quick() -> Result<()> {
    QUICK.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

async fn queue(db: &Db, name: &'static str) {
    let jobs: Jobs = resolved(db).await.unwrap();
    let mut queued = match name {
        "slow" => jobs.queue(slow, &json!({})),
        _ => jobs.queue(quick, &json!({})),
    }
    .unwrap();
    db.commit(&mut queued).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_slow_job_holds_one_slot_while_the_others_keep_flowing() {
    let setup = Setup::new().await;
    sofie::db::name_job(&setup.db, "slow", slow);
    sofie::db::name_job(&setup.db, "quick", quick);
    queue(&setup.db, "slow").await;
    for _ in 0..5 {
        queue(&setup.db, "quick").await;
    }
    let worker = Worker::new(setup.db.clone())
        .job("slow", slow)
        .job("quick", quick)
        .batch(NonZeroUsize::new(2).unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let running = tokio::spawn(worker.run(async {
        let _ = stopped.await;
    }));

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while QUICK.load(Ordering::SeqCst) < 5 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert_eq!(
        QUICK.load(Ordering::SeqCst),
        5,
        "the quick jobs pass the slow one"
    );
    RELEASED.store(true, Ordering::SeqCst);
    stop.send(()).unwrap();
    running.await.unwrap().unwrap();
    let left = setup.rows("SELECT count(*) FROM sofie.messages").await;
    assert_eq!(
        left,
        [[SqlValue::Int(0)]],
        "the slow job finished before the worker stopped"
    );
}
