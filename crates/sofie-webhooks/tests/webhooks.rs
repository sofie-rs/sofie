use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ring::hmac;
use serde_json::{Value, json};
use sofie::core::encoding::base64::{self, STANDARD};
use sofie::db::{BoxFuture, Db, Queued};
use sofie::http::{Access, AreaPrincipal};
use sofie::prelude::*;
use sofie_testing::{TestApp, TestCaller, TestPrincipal};
use sofie_webhooks::{WebhookArea, WebhookEvent, WebhookSender, Webhooks};

const PAID: WebhookEvent = WebhookEvent::new("order.paid", "An order was paid.");
const REFUNDED: WebhookEvent = WebhookEvent::new("order.refunded", "An order was refunded.");

struct Customer;

impl AreaPrincipal for Customer {
    const ACCESS: Access = Access::Owned;
    type Record = Self;
}

struct Member;

impl Area for Member {
    const NAME: &'static str = "member";
    const PREFIX: &'static str = "/account";
    const SIGN_IN: &'static str = "/sign-in";
    type Principal = Customer;
}

impl WebhookArea for Member {
    const WEBHOOK_EVENTS: &'static [WebhookEvent] = &[PAID, REFUNDED];
}

async fn pay(me: Principal<Member>, webhooks: Webhooks) -> Result<Queued> {
    webhooks.send(me.id().uuid(), &PAID, &json!({ "order": "o_1" }))
}

async fn refund(me: Principal<Member>, webhooks: Webhooks) -> Result<Queued> {
    webhooks.send(me.id().uuid(), &REFUNDED, &json!({ "order": "o_1" }))
}

fn orders(routes: &mut Routes<Member>) {
    routes.post("/orders/pay", pay);
    routes.post("/orders/refund", refund);
}

struct Sent {
    url: String,
    headers: Vec<(&'static str, String)>,
    body: String,
}

impl Sent {
    fn header(&self, name: &str) -> &str {
        self.headers
            .iter()
            .find(|(header, _)| *header == name)
            .map_or("", |(_, value)| value.as_str())
    }
}

#[derive(Default)]
struct Receiver {
    sent: Mutex<Vec<Sent>>,
    answers: Mutex<VecDeque<Result<u16>>>,
}

impl Receiver {
    fn answer_next(&self, answer: Result<u16>) {
        self.answers.lock().unwrap().push_back(answer);
    }

    fn sent(&self) -> Vec<Sent> {
        std::mem::take(&mut *self.sent.lock().unwrap())
    }
}

impl WebhookSender for Receiver {
    fn post<'a>(
        &'a self,
        url: &'a str,
        headers: &'a [(&'static str, String)],
        body: &'a str,
    ) -> BoxFuture<'a, Result<u16>> {
        self.sent.lock().unwrap().push(Sent {
            url: url.to_owned(),
            headers: headers.to_vec(),
            body: body.to_owned(),
        });
        let answer = self.answers.lock().unwrap().pop_front().unwrap_or(Ok(204));
        Box::pin(std::future::ready(answer))
    }
}

async fn started() -> (TestApp, Arc<Receiver>) {
    let app = sofie_testing::start(
        |db: Db| async move {
            db.schema().migrate(&[sofie_webhooks::migrations()]).await?;
            Ok(App::new(db)
                .area::<Member>("sofie_webhooks", sofie_webhooks::routes::<Member>)
                .area::<Member>("orders", orders))
        },
        |db| sofie_webhooks::worker(sofie::worker::Worker::new(db)),
    )
    .await;
    let receiver = Arc::new(Receiver::default());
    sofie::db::provide::<dyn WebhookSender>(
        app.db(),
        Arc::clone(&receiver) as Arc<dyn WebhookSender>,
    );
    (app, receiver)
}

async fn created(caller: &TestCaller<'_>, events: Value) -> Value {
    let answer = caller
        .post("/account/webhooks")
        .accept_json()
        .json(&json!({ "url": "https://hooks.example.com/in", "events": events }))
        .await;
    answer.assert_ok();
    answer.json()
}

fn signature(secret: &str, sent: &Sent) -> String {
    let key = base64::decode(&STANDARD, secret.strip_prefix("whsec_").unwrap()).unwrap();
    let key = hmac::Key::new(hmac::HMAC_SHA256, &key);
    let signed = format!(
        "{}.{}.{}",
        sent.header("webhook-id"),
        sent.header("webhook-timestamp"),
        sent.body
    );
    format!(
        "v1,{}",
        base64::encode_padded(&STANDARD, hmac::sign(&key, signed.as_bytes()).as_ref())
    )
}

#[tokio::test]
async fn an_event_reaches_each_subscribed_endpoint_of_its_owner_signed() {
    let (app, receiver) = started().await;
    let owner = TestPrincipal::<Member>::fresh(&app);
    let other = TestPrincipal::<Member>::fresh(&app);
    let endpoint = created(&app.as_(&owner), json!(["order.paid"])).await;
    created(&app.as_(&other), json!(["*"])).await;

    app.as_(&owner)
        .post("/account/orders/pay")
        .await
        .assert_ok();
    app.as_(&owner)
        .post("/account/orders/refund")
        .await
        .assert_ok();
    app.drain().await;

    let sent = receiver.sent();
    assert_eq!(
        sent.len(),
        1,
        "only the owner's endpoint subscribed to the event hears it"
    );
    let body: Value = serde_json::from_str(&sent[0].body).unwrap();
    assert_eq!(sent[0].url, "https://hooks.example.com/in");
    assert_eq!(body["type"], "order.paid");
    assert_eq!(body["data"], json!({ "order": "o_1" }));
    assert_eq!(
        sent[0].header("webhook-signature"),
        signature(endpoint["secret"].as_str().unwrap(), &sent[0])
    );
}

#[tokio::test]
async fn a_failed_delivery_is_tried_again_with_its_id_and_logged() {
    let (app, receiver) = started().await;
    let owner = TestPrincipal::<Member>::fresh(&app);
    created(&app.as_(&owner), json!(["order.paid"])).await;
    receiver.answer_next(Ok(503));

    app.as_(&owner)
        .post("/account/orders/pay")
        .await
        .assert_ok();
    app.drain().await;
    let first = receiver.sent();
    app.advance(Duration::from_secs(6)).await;
    let again = receiver.sent();

    assert_eq!(first.len(), 1);
    assert_eq!(again.len(), 1);
    assert_eq!(first[0].header("webhook-id"), again[0].header("webhook-id"));
    let deliveries = app
        .as_(&owner)
        .get("/account/webhooks/deliveries")
        .accept_json()
        .await
        .json();
    assert_eq!(deliveries[0]["attempts"], 2);
    assert_eq!(deliveries[0]["status"], 204);
    assert!(deliveries[0]["delivered_at"].is_string(), "{deliveries}");
}

#[tokio::test]
async fn a_refused_address_is_given_up_at_once() {
    let (app, receiver) = started().await;
    let owner = TestPrincipal::<Member>::fresh(&app);
    created(&app.as_(&owner), json!(["order.paid"])).await;
    receiver.answer_next(Err(Error::BadRequest("the host is not public".to_owned())));

    app.as_(&owner)
        .post("/account/orders/pay")
        .await
        .assert_ok();
    app.drain().await;
    app.advance(Duration::from_mins(10)).await;

    assert_eq!(receiver.sent().len(), 1);
    let deliveries = app
        .as_(&owner)
        .get("/account/webhooks/deliveries")
        .accept_json()
        .await
        .json();
    assert_eq!(deliveries[0]["error"], "the host is not public");
    assert!(deliveries[0]["failed_at"].is_string(), "{deliveries}");
}

#[tokio::test]
async fn an_owner_sees_and_changes_only_its_own_endpoints_without_their_secrets() {
    let (app, _receiver) = started().await;
    let owner = TestPrincipal::<Member>::fresh(&app);
    let other = TestPrincipal::<Member>::fresh(&app);
    let endpoint = created(&app.as_(&owner), json!(["order.paid"])).await;
    let path = format!("/account/webhooks/{}", endpoint["id"].as_str().unwrap());

    let listed = app
        .as_(&owner)
        .get("/account/webhooks")
        .accept_json()
        .await
        .json();
    let others = app
        .as_(&other)
        .get("/account/webhooks")
        .accept_json()
        .await
        .json();
    let taken = app
        .as_(&other)
        .request(&sofie::http::RoutePath::new(
            sofie::http::Method::PATCH,
            path.clone(),
        ))
        .accept_json()
        .json(&json!({ "enabled": false }))
        .await;

    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert!(listed[0].get("secret").is_none(), "{listed}");
    assert_eq!(others, json!([]));
    assert_eq!(taken.status, 404);
}

#[tokio::test]
async fn an_endpoint_names_offered_events_and_an_https_url() {
    let (app, _receiver) = started().await;
    let owner = TestPrincipal::<Member>::fresh(&app);
    let caller = app.as_(&owner);

    let unknown = caller
        .post("/account/webhooks")
        .accept_json()
        .json(&json!({ "url": "https://hooks.example.com", "events": ["order.lost"] }))
        .await;
    let plain = caller
        .post("/account/webhooks")
        .accept_json()
        .json(&json!({ "url": "http://hooks.example.com", "events": ["order.paid"] }))
        .await;
    let offered = caller
        .get("/account/webhooks/events")
        .accept_json()
        .await
        .json();

    assert_eq!(unknown.status, 422);
    assert_eq!(plain.status, 422);
    assert_eq!(offered[0]["name"], "order.paid");
}

#[tokio::test]
async fn a_rotated_secret_and_the_previous_one_both_sign_for_a_day() {
    let (app, receiver) = started().await;
    let owner = TestPrincipal::<Member>::fresh(&app);
    let endpoint = created(&app.as_(&owner), json!(["order.paid"])).await;
    let id = endpoint["id"].as_str().unwrap();

    let rotated = app
        .as_(&owner)
        .post(&format!("/account/webhooks/{id}/secret"))
        .accept_json()
        .await
        .json();
    app.as_(&owner)
        .post("/account/orders/pay")
        .await
        .assert_ok();
    app.drain().await;

    let sent = receiver.sent();
    let new = signature(rotated["secret"].as_str().unwrap(), &sent[0]);
    let old = signature(endpoint["secret"].as_str().unwrap(), &sent[0]);
    assert_eq!(sent[0].header("webhook-signature"), format!("{new} {old}"));
}

#[tokio::test]
async fn a_disabled_endpoint_hears_nothing_until_enabled() {
    let (app, receiver) = started().await;
    let owner = TestPrincipal::<Member>::fresh(&app);
    let endpoint = created(&app.as_(&owner), json!(["order.paid"])).await;
    let path = format!("/account/webhooks/{}", endpoint["id"].as_str().unwrap());
    let change = |enabled: bool| {
        app.as_(&owner)
            .request(&sofie::http::RoutePath::new(
                sofie::http::Method::PATCH,
                path.clone(),
            ))
            .accept_json()
            .json(&json!({ "enabled": enabled }))
    };

    change(false).await.assert_ok();
    app.as_(&owner)
        .post("/account/orders/pay")
        .await
        .assert_ok();
    app.drain().await;
    let while_disabled = receiver.sent().len();
    change(true).await.assert_ok();
    app.as_(&owner)
        .post("/account/orders/pay")
        .await
        .assert_ok();
    app.drain().await;

    assert_eq!(while_disabled, 0);
    assert_eq!(receiver.sent().len(), 1);
}
