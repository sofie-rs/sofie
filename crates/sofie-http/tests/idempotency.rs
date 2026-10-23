#![cfg(feature = "webhooks")]

use http::StatusCode;
use serde_json::json;
use sofie_core::{Error, Result, SqlValue, Uuid};
use sofie_db::{Db, File, IdempotencyKey, Migrations, Outcome, Plan, Reply, Step};
use sofie_http::{App, Delivery, Routes, Verify, Web};
use sofie_testing::{TestApp, TestRequest};

const MIGRATIONS: Migrations = Migrations::new(
    "payments",
    &[File::new(
        "0001_charge.sql",
        b"CREATE TABLE charge (id uuid PRIMARY KEY, key text NOT NULL)",
    )],
);
const SECRET: &str = "whsec_test";

struct Charge {
    id: Uuid,
    key: IdempotencyKey,
}

impl Outcome for Charge {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        plan.push(Step::Statement {
            sql: "INSERT INTO payments.charge (id, key) VALUES ($1, $2)".to_owned(),
            params: vec![
                SqlValue::Uuid(self.id),
                SqlValue::Text(self.key.as_str().to_owned()),
            ],
            expect_rows: None,
        });
        Ok(())
    }

    fn reply(self) -> Result<Reply> {
        Ok(Reply::Json(json!({ "charge": self.id.to_string() })))
    }
}

async fn charge(key: IdempotencyKey) -> Result<Charge> {
    Ok(Charge {
        id: Uuid::now_v7()?,
        key,
    })
}

struct SharedSecret;

impl Verify for SharedSecret {
    fn verify(&self, delivery: &Delivery<'_>) -> Result<String> {
        if delivery.header("x-signature") != Some(SECRET) || delivery.body().is_empty() {
            return Err(Error::BadRequest("the signature does not match".to_owned()));
        }
        delivery
            .header("delivery-id")
            .map(str::to_owned)
            .ok_or_else(|| Error::BadRequest("no delivery id".to_owned()))
    }
}

fn routes(routes: &mut Routes<Web>) {
    routes.post("/charges", charge).idempotent();
    routes.post("/charges/unkeyed", charge);
    routes
        .webhook("/payments/callback", charge)
        .verify(SharedSecret);
}

async fn start() -> TestApp {
    sofie_testing::start(
        |db: Db| async move {
            db.schema().migrate(&[MIGRATIONS]).await?;
            Ok(App::new(db).area("payments", routes))
        },
        sofie_db::worker::Worker::new,
    )
    .await
}

async fn charges(app: &TestApp) -> usize {
    app.query("SELECT id FROM payments.charge", &[]).await.len()
}

fn keyed(request: TestRequest, key: &str, amount: u32) -> TestRequest {
    request
        .header("idempotency-key", key)
        .json(&json!({ "amount": amount }))
}

fn delivery(request: TestRequest, id: &str) -> TestRequest {
    request
        .header("x-signature", SECRET)
        .header("delivery-id", id)
        .json(&json!({ "type": "payment.succeeded" }))
}

#[tokio::test]
async fn an_idempotent_route_answers_each_key_once() {
    let app = start().await;

    let first = keyed(app.post("/charges"), "order-7", 10).await;
    let retried = keyed(app.post("/charges"), "order-7", 10).await;
    let another = keyed(app.post("/charges"), "order-8", 10).await;

    assert_eq!(first.status, StatusCode::OK, "{}", first.body);
    assert_eq!(retried.status, StatusCode::OK);
    assert_eq!(retried.json(), first.json());
    assert_ne!(another.json(), first.json());
    assert_eq!(charges(&app).await, 2);
}

#[tokio::test]
async fn an_idempotent_route_refuses_a_missing_or_reused_key() {
    let app = start().await;
    keyed(app.post("/charges"), "order-7", 10).await;

    let missing = app.post("/charges").json(&json!({ "amount": 10 })).await;
    let reused = keyed(app.post("/charges"), "order-7", 99).await;
    let unkeyed = app.post("/charges/unkeyed").accept_json().await;

    assert_eq!(missing.status, StatusCode::BAD_REQUEST);
    assert_eq!(reused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        reused.json()["error"],
        "This key was already used for a different request."
    );
    assert_eq!(unkeyed.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(charges(&app).await, 1);
}

#[tokio::test]
async fn retries_running_alongside_write_once_and_share_the_answer() {
    let app = start().await;

    let (first, second, third) = tokio::join!(
        keyed(app.post("/charges"), "order-9", 10),
        keyed(app.post("/charges"), "order-9", 10),
        keyed(app.post("/charges"), "order-9", 10),
    );

    let answered: Vec<_> = [first, second, third]
        .into_iter()
        .filter(|answer| answer.status == StatusCode::OK)
        .map(|answer| answer.json())
        .collect();
    assert_ne!(answered.len(), 0, "no retry was answered");
    assert!(
        answered.iter().all(|answer| *answer == answered[0]),
        "{answered:?}"
    );
    assert_eq!(charges(&app).await, 1);
}

#[tokio::test]
async fn a_webhook_runs_each_verified_delivery_once_outside_any_session() {
    let app = start().await;

    let first = delivery(app.post("/payments/callback"), "evt_1")
        .header("sec-fetch-site", "cross-site")
        .header("cookie", "__Host-sofie-session=forged")
        .await;
    let redelivered = delivery(app.post("/payments/callback"), "evt_1").await;
    let unsigned = app
        .post("/payments/callback")
        .header("delivery-id", "evt_2")
        .json(&json!({ "type": "payment.succeeded" }))
        .await;

    assert_eq!(first.status, StatusCode::OK, "{}", first.body);
    assert_eq!(first.header("set-cookie"), "");
    assert_eq!(redelivered.json(), first.json());
    assert_eq!(unsigned.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        unsigned.json()["error"],
        "The delivery could not be verified."
    );
    assert_eq!(charges(&app).await, 1);
}

#[tokio::test]
async fn answering_never_sweeps_and_the_sweep_drops_only_expired_keys() {
    let app = start().await;
    let stored = || app.query("SELECT key FROM sofie.idempotency", &[]);
    keyed(app.post("/charges"), "order-7", 10).await;
    app.clock()
        .advance(std::time::Duration::from_hours(25))
        .unwrap();
    keyed(app.post("/charges"), "order-8", 10).await;
    let before_sweep = stored().await.len();

    sofie_db::sweep_expired_keys(app.db()).await.unwrap();

    assert_eq!(before_sweep, 2);
    assert_eq!(stored().await.len(), 1);
}
