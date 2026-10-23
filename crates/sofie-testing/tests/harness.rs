#[cfg(feature = "flags")]
use std::sync::OnceLock;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sofie::db::{Db, File, Migrations};
#[cfg(feature = "flags")]
use sofie::flags::{Flags, Rule};
use sofie::prelude::*;
use sofie::values::Fixture;
use sofie::worker::Worker;
use sofie_core::unpoisoned;
use sofie_testing::{
    TestApp, TestPrincipal, assert_boundary, assert_moves, assert_raised, assert_store,
    assert_unique, assert_value, drain_failing, fixture_input,
};

const TABLES: &str = "
CREATE TABLE product (
    id uuid PRIMARY KEY, version bigint NOT NULL,
    created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL,
    seller uuid NOT NULL, name text NOT NULL, sku text NOT NULL UNIQUE,
    price bigint NOT NULL, price_currency text NOT NULL, stock bigint NOT NULL,
    status text NOT NULL
);
CREATE TABLE note (
    id uuid PRIMARY KEY, version bigint NOT NULL,
    created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL,
    text text NOT NULL
);
";

pub struct Seller;

impl sofie::http::AreaPrincipal for Seller {
    const ACCESS: sofie::http::Access = sofie::http::Access::Owned;
    type Record = Self;
}

#[derive(Aggregate)]
pub struct Product {
    meta: Meta,
    #[owner]
    seller: Id<Seller>,
    #[required]
    #[max(12)]
    name: String,
    #[unique]
    sku: String,
    price: Money,
    #[range(min = 0, max = 500)]
    stock: i64,
    status: ProductStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Enum)]
pub enum ProductStatus {
    #[to(Listed)]
    Draft,
    #[to(Retired)]
    Listed,
    Retired,
}

#[derive(Event)]
pub struct NoteWritten {
    pub text: String,
}

#[derive(Aggregate)]
pub struct Note {
    meta: Meta,
    text: String,
}

impl Note {
    fn write(text: &str) -> Self {
        let mut note = Self {
            meta: Meta::new().unwrap(),
            text: text.to_owned(),
        };
        note.meta
            .raise(NoteWritten {
                text: text.to_owned(),
            })
            .unwrap();
        note
    }
}

pub struct Sellers;

impl Area for Sellers {
    const NAME: &'static str = "sellers";
    const PREFIX: &'static str = "/sellers";
    const SIGN_IN: &'static str = "/sign-in";
    type Principal = Seller;
}

async fn signed_in(_seller: Principal<Sellers>) -> Result<()> {
    Ok(())
}

#[cfg(feature = "contracts")]
async fn reserve(_seller: Principal<Sellers>, stock: Dep<dyn Stock>) -> Result<()> {
    stock.reserve("MUG-1", 1).await.map(drop)
}

fn routes(routes: &mut Routes<Sellers>) {
    routes.get("/me", signed_in);
    #[cfg(feature = "contracts")]
    routes.post("/reserve", reserve);
}

async fn harness(db: Db) -> Result<App> {
    db.schema()
        .migrate(&[Migrations::new(
            "harness",
            const { &[File::new("0001_tables.sql", TABLES.as_bytes())] },
        )])
        .await?;
    Ok(App::new(db).area::<Sellers>("harness", routes))
}

async fn started() -> TestApp {
    sofie_testing::start(harness, Worker::new).await
}

#[tokio::test]
async fn a_fixture_is_made_by_the_default_factory_held_to_its_constraints_and_saved() {
    let app = started().await;
    let seller = TestPrincipal::<Sellers>::fresh(&app);

    let product: Product = app.fixture().owned_by(seller.id()).await;

    let rows = app
        .query(
            "SELECT seller, name, status, stock FROM harness.product",
            &[],
        )
        .await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], sofie::core::SqlValue::Uuid(seller.id().uuid()));
    let sofie::core::SqlValue::Text(name) = &rows[0][1] else {
        panic!("{rows:?}");
    };
    assert!(!name.is_empty() && name.chars().count() <= 12, "{name}");
    assert_eq!(rows[0][2], sofie::core::SqlValue::Text("DRAFT".to_owned()));
    assert_eq!(product.meta().version(), 1);
}

#[tokio::test]
async fn fixtures_are_the_same_in_every_run() {
    let first = started().await;
    let second = started().await;

    let inputs = |app: &TestApp| {
        [
            fixture_input::<<Product as sofie::db::Create>::Input>(app),
            fixture_input::<<Product as sofie::db::Create>::Input>(app),
        ]
    };

    let made = inputs(&first);
    assert_eq!(made, inputs(&second));
    assert_ne!(made[0], made[1]);
}

#[tokio::test]
async fn a_record_loads_as_saved_and_a_stale_save_conflicts() {
    let app = started().await;
    let first: Product = app.fixture().await;
    let second: Product = app.fixture().await;

    assert_store(&app, first.id(), second.id()).await;
}

#[tokio::test]
async fn a_unique_field_refuses_a_second_record_with_the_same_value() {
    let app = started().await;

    assert_unique::<Product>(&app, "sku").await;
}

#[tokio::test]
async fn values_round_trip_through_json_and_the_database() {
    let app = started().await;

    assert_value::<Money>(&app).await;
    assert_value::<Email>(&app).await;
    assert_value::<Instant>(&app).await;
}

#[test]
fn each_constraint_is_refused_past_its_bound() {
    assert_boundary("name", Product::__NAME, "required");
    assert_boundary("name", Product::__NAME, "max 12");
    assert_boundary("stock", Product::__STOCK, "min 0");
    assert_boundary("stock", Product::__STOCK, "max 500");
}

#[test]
fn a_status_moves_only_along_its_transitions() {
    assert_moves::<ProductStatus>(&[("DRAFT", &["LISTED"]), ("LISTED", &["RETIRED"])]);
}

#[test]
#[should_panic(expected = "transitions changed")]
fn a_changed_transition_fails_the_test_of_the_old_one() {
    assert_moves::<ProductStatus>(&[("DRAFT", &["LISTED", "RETIRED"]), ("LISTED", &["RETIRED"])]);
}

#[tokio::test]
async fn a_test_calls_as_a_principal_without_signing_in() {
    let app = started().await;
    let seller = TestPrincipal::<Sellers>::fresh(&app);

    let anonymous = app.get("/sellers/me").accept_json().await;
    let signed_in = app.as_(&seller).get("/sellers/me").accept_json().await;

    anonymous.assert_status(401);
    signed_in.assert_ok();
}

async fn deliver(_event: NoteWritten) -> Result<()> {
    Ok(())
}

#[tokio::test]
async fn a_raised_event_is_found_before_and_after_it_is_delivered() {
    let app = sofie_testing::start(harness, |db| {
        Worker::new(db).on::<NoteWritten, _>("harness.deliver", deliver)
    })
    .await;
    let mut note = Note::write("restock on Monday");
    app.db().commit(&mut note).await.unwrap();

    let before = assert_raised::<NoteWritten>(&app);
    assert_eq!(app.drain().await, 1);
    let after = assert_raised::<NoteWritten>(&app);

    assert_eq!(before.len(), 1);
    assert_eq!(after[0].text, "restock on Monday");
}

async fn refuse(_event: NoteWritten) -> Result<()> {
    Err(Error::internal_message("the shelf is full"))
}

async fn started_refusing() -> TestApp {
    let app = sofie_testing::start(harness, |db| {
        Worker::new(db).on::<NoteWritten, _>("harness.refuse", refuse)
    })
    .await;
    let mut note = Note::write("restock on Monday");
    app.db().commit(&mut note).await.unwrap();
    app
}

#[tokio::test]
#[should_panic(expected = "the shelf is full")]
async fn a_failing_job_fails_the_drain() {
    started_refusing().await.drain().await;
}

#[tokio::test]
async fn a_drain_expecting_failures_names_them() {
    let app = started_refusing().await;

    let failures = drain_failing(&app).await;

    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].job, "harness.refuse");
}

static FIRED: Mutex<Vec<(&str, i64)>> = Mutex::new(Vec::new());

fn fired(name: &'static str, clock: &Clock) {
    let second = clock.now().timestamp().as_second();
    unpoisoned::lock(&FIRED).push((name, second));
}

async fn hourly(clock: Clock) -> Result<()> {
    fired("hourly", &clock);
    Ok(())
}

async fn every_ninety_minutes(clock: Clock) -> Result<()> {
    fired("ninety", &clock);
    Ok(())
}

#[tokio::test]
async fn advancing_the_clock_runs_each_schedule_when_due_in_order() {
    let app = sofie_testing::start(harness, |db| {
        Worker::new(db)
            .every("harness.hourly", Duration::from_hours(1), hourly)
            .every(
                "harness.ninety",
                Duration::from_mins(90),
                every_ninety_minutes,
            )
    })
    .await;
    let start = app.clock().clock().now().timestamp().as_second();

    let completed = app.advance(Duration::from_hours(3)).await;

    let fired: Vec<(&str, i64)> = unpoisoned::lock(&FIRED)
        .iter()
        .map(|(name, second)| (*name, second - start))
        .collect();
    let mut first = fired[..2].to_vec();
    first.sort_unstable();
    assert_eq!(first, [("hourly", 0), ("ninety", 0)]);
    assert_eq!(
        fired[2..5],
        [("hourly", 3_600), ("ninety", 5_400), ("hourly", 7_200)]
    );
    let mut last = fired[5..].to_vec();
    last.sort_unstable();
    assert_eq!(last, [("hourly", 10_800), ("ninety", 10_800)]);
    assert_eq!(completed, 7);
}

#[cfg(feature = "flags")]
static WORKER_FLAGS: OnceLock<Flags> = OnceLock::new();

#[cfg(feature = "flags")]
#[tokio::test]
async fn the_app_and_its_worker_share_one_set_of_flags() {
    let app = sofie_testing::start_with_flags(
        |db, _flags| harness(db),
        |db, flags| {
            WORKER_FLAGS.set(flags.clone()).ok();
            Worker::new(db)
        },
    )
    .await;

    app.flags()
        .set("harness.beta", Rule::everyone())
        .await
        .unwrap();

    let seen = WORKER_FLAGS.get().unwrap().rules();
    assert_eq!(seen, [("harness.beta".to_owned(), Rule::everyone())]);
}

#[sofie::contract]
pub trait Stock: Send + Sync {
    async fn reserve(&self, sku: &str, count: i64) -> Result<i64>;
}

#[tokio::test]
async fn a_contract_s_fake_records_calls_and_fails_when_told() {
    let fake = Arc::new(FakeStock::default());
    let stock: Arc<dyn Stock> = fake.clone();

    let reserved = stock.reserve("MUG-1", 2).await;
    fake.calls.fail("reserve");
    let failed = stock.reserve("MUG-1", 2).await;

    assert_eq!(
        reserved.unwrap(),
        i64::fixture(&mut sofie::values::Seed::new(0))
    );
    assert!(matches!(failed, Err(Error::Unavailable(_))), "{failed:?}");
    assert_eq!(fake.calls.made(), ["reserve", "reserve"]);
}

#[cfg(feature = "contracts")]
#[tokio::test]
async fn a_test_answers_a_dependency_of_the_app_with_its_fake() {
    let app = started().await;
    let seller = TestPrincipal::<Sellers>::fresh(&app);
    let fake = Arc::new(FakeStock::default());

    sofie_testing::fake::<dyn Stock>(&app, fake.clone());
    let reserved = app
        .as_(&seller)
        .post("/sellers/reserve")
        .accept_json()
        .await;

    reserved.assert_ok();
    assert_eq!(fake.calls.made(), ["reserve"]);
}

async fn listed() -> Result<()> {
    Ok(())
}

#[tokio::test]
#[should_panic(expected = "are served at one path")]
async fn a_test_server_whose_app_cannot_start_says_why() {
    let app = sofie::http::App::new(Db::new(sofie_testing::FakeDriver::default())).area(
        "shop",
        |routes: &mut sofie::http::Routes<sofie::http::Web>| {
            routes.get("/orders/{order}", listed);
            routes.get("/orders/{id}", listed);
        },
    );

    sofie_testing::TestServer::start(app).await;
}
