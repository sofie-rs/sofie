use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use ring::hmac;
use serde_json::{Value, json};
use sofie_billing::{
    Billing, Consumed, Entitlement, Entitlements, Limit, Link, Period, Plan, Plans, Product,
    ProviderKeys, PurchaseCompleted, Quota, Seats, Setup, Standing, Status, Subscriber,
    SubscriptionChanged, TrialStarted, UNLIMITED, Vendor,
};
use sofie_core::encoding::hex;
use sofie_core::{Result, SqlValue, Uuid, unpoisoned};
use sofie_db::{Db, Id};
use sofie_http::{App, Area, Gates, Principal, Routes, Web};
use sofie_testing::{TestApp, TestPrincipal};
use tokio::net::TcpListener;

const EXPORT: Entitlement = Entitlement::new("project.export");
const PROJECTS: Limit = Limit::new("project.projects");
const CALLS: Quota = Quota::new("project.api_calls", Period::Month);
const CREDITS: Product = Product::new("credits");
const FREE: Plan = Plan::new("free")
    .limits(&[(PROJECTS, 1)])
    .quotas(&[(CALLS, 2)]);
const PRO: Plan = Plan::new("pro")
    .grants(&[EXPORT])
    .limits(&[(PROJECTS, UNLIMITED)])
    .quotas(&[(CALLS, 3)])
    .trial_days(14)
    .per_seat();
const PLANS: Plans = Plans::new(FREE, &[PRO]).products(&[CREDITS]);
const PRICES: &[(&str, &str)] = &[("pro", "price_pro"), ("credits", "price_credits")];
const SECRET: &str = "whsec_test";
const PLAN_SCREEN: &str =
    r#"{#if entitled("project.export")}<p id="export">export</p>{:else}<p>upgrade</p>{/if}"#;

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

async fn start_trial(me: Principal<Members>, billing: Billing) -> Result<TrialStarted> {
    billing.start_trial(me.id().into(), PRO).await
}

async fn export() -> Result<()> {
    Ok(())
}

async fn first_project(entitlements: Entitlements) -> Result<()> {
    entitlements.allow_another(PROJECTS, 0)
}

async fn second_project(entitlements: Entitlements) -> Result<()> {
    entitlements.allow_another(PROJECTS, 1)
}

async fn call(entitlements: Entitlements) -> Result<Consumed> {
    entitlements.consume(CALLS, 1).await
}

async fn plan_page() -> Result<()> {
    Ok(())
}

async fn upgrade(me: Principal<Members>, billing: Billing) -> Result<Link> {
    billing
        .subscribe(me.id().into(), PRO, 2, "/members/plan")
        .await
}

async fn buy_credits(me: Principal<Members>, billing: Billing) -> Result<Link> {
    billing
        .buy(me.id().into(), CREDITS, 3, "/members/plan")
        .await
}

async fn manage(me: Principal<Members>, billing: Billing) -> Result<Link> {
    billing.portal(me.id().into(), "/members/plan").await
}

fn member_routes(routes: &mut Routes<Members>) {
    routes.post("/trial", start_trial);
    routes.get("/export", export).entitled(EXPORT);
    routes.post("/projects/first", first_project);
    routes.post("/projects/second", second_project);
    routes.post("/calls", call);
    routes.screen("/plan", "plan", plan_page);
    routes.post("/upgrade", upgrade).then_follow();
    routes.post("/portal", manage).then_follow();
    routes.post("/credits", buy_credits).then_follow();
}

/// What the fake provider was asked, as path, body and idempotency key.
type Asked = Arc<Mutex<Vec<(String, String, Option<String>)>>>;

fn failed(status: http::StatusCode) -> http::Response<Full<Bytes>> {
    let mut failed = http::Response::new(Full::new(Bytes::new()));
    *failed.status_mut() = status;
    failed
}

/// A request of `body` waits for `release` before it lands, as a slow one would.
#[derive(Default)]
struct Hold {
    body: Mutex<Option<String>>,
    arrived: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

impl Hold {
    async fn wait(&self, body: &[u8]) {
        let held = {
            let mut held = unpoisoned::lock(&self.body);
            held.take_if(|held| held.as_bytes() == body).is_some()
        };
        if !held {
            return;
        }
        self.arrived.notify_one();
        self.release.notified().await;
    }
}

async fn fake_provider() -> (String, Asked) {
    fake_provider_holding(Arc::default()).await
}

/// Stripe's subscription item update fails once, as a provider's outage would.
async fn fake_provider_holding(hold: Arc<Hold>) -> (String, Asked) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let asked = Asked::default();
    let recorded = Arc::clone(&asked);
    let item_failed = Arc::new(AtomicBool::new(false));
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let recorded = Arc::clone(&recorded);
            let item_failed = Arc::clone(&item_failed);
            let hold = Arc::clone(&hold);
            let service = service_fn(move |request: http::Request<Incoming>| {
                let recorded = Arc::clone(&recorded);
                let item_failed = Arc::clone(&item_failed);
                let hold = Arc::clone(&hold);
                async move {
                    let path = request.uri().path().to_owned();
                    let method = request.method().clone();
                    let key = request
                        .headers()
                        .get("idempotency-key")
                        .map(|key| key.to_str().unwrap().to_owned());
                    let body = request.into_body().collect().await.unwrap().to_bytes();
                    hold.wait(&body).await;
                    unpoisoned::lock(&recorded).push((
                        path.clone(),
                        String::from_utf8(body.to_vec()).unwrap(),
                        key,
                    ));
                    if path == "/transactions" {
                        return Ok::<_, Infallible>(failed(http::StatusCode::SERVICE_UNAVAILABLE));
                    }
                    if path == "/v1/subscription_items/si_1"
                        && !item_failed.swap(true, Ordering::SeqCst)
                    {
                        return Ok(failed(http::StatusCode::INTERNAL_SERVER_ERROR));
                    }
                    let answer = match path.as_str() {
                        "/v1/subscriptions/sub_1" => {
                            json!({"id": "sub_1", "items": {"data": [{"id": "si_1"}]}})
                        }
                        "/subscriptions/sub_01" if method == http::Method::GET => {
                            json!({"data": {"id": "sub_01", "items": [{"price": {"id": "price_pro"}}]}})
                        }
                        "/v1/checkout/sessions" => {
                            json!({"id": "cs_1", "url": "https://checkout.stripe.test/c/cs_1"})
                        }
                        "/v1/billing_portal/sessions" => {
                            json!({"url": "https://billing.stripe.test/p/1"})
                        }
                        _ => json!({}),
                    };
                    Ok(http::Response::new(Full::new(Bytes::from(
                        answer.to_string(),
                    ))))
                }
            });
            tokio::spawn(async move {
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    (format!("http://{address}"), asked)
}

fn setup(vendor: Vendor, api_url: &str) -> Setup {
    let settings = sofie_client::Vendor {
        base_url: Some(api_url.to_owned()),
        private: Some(true),
        ..sofie_client::Vendor::default()
    };
    let http = sofie_client::for_vendor(vendor.name(), settings).unwrap();
    let keys = ProviderKeys {
        api_key: "sk_test".to_owned(),
        webhook_secret: SECRET.to_owned(),
    };
    Setup::with_provider(vendor, http, keys, PRICES, "https://app.test").unwrap()
}

async fn start(setup: Setup) -> TestApp {
    sofie_testing::start(
        move |db: Db| async move {
            db.schema().migrate(&[sofie_billing::migrations()]).await?;
            sofie_billing::configure(&db, setup);
            let billing = sofie_billing::install(&db, &PLANS).checked()?;
            let screens =
                sofie_http::screens_from_sources(&[("billing/members/plan", PLAN_SCREEN)])?;
            Ok(App::new(db)
                .screens(screens)
                .gates(Gates::default().entitlements(billing.clone()))
                .area::<Web>("billing", |routes| billing.routes(routes))
                .area::<Members>("billing", member_routes))
        },
        |db| sofie_billing::worker(sofie_db::worker::Worker::new(db)),
    )
    .await
}

async fn started() -> (TestApp, Asked) {
    let (api_url, asked) = fake_provider().await;
    (start(setup(Vendor::Stripe, &api_url)).await, asked)
}

fn member(uuid: u128) -> TestPrincipal<Members> {
    TestPrincipal::new(Id::from_uuid(Uuid::from_u128(uuid)))
}

fn now(app: &TestApp) -> i64 {
    app.db().clock().now().timestamp().as_second()
}

fn sign(secret: &str, timestamp: &str, separator: &str, body: &str) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    let tag = hmac::sign(&key, format!("{timestamp}{separator}{body}").as_bytes());
    hex::encode(tag.as_ref())
}

async fn stripe_event(app: &TestApp, event: &Value) -> sofie_testing::TestResponse {
    let body = event.to_string();
    let timestamp = now(app).to_string();
    let signature = sign(SECRET, &timestamp, ".", &body);
    app.post(sofie_billing::WEBHOOK_PATH)
        .header("stripe-signature", &format!("t={timestamp},v1={signature}"))
        .header("content-type", "application/json")
        .body(body)
        .await
}

fn subscription_event(id: &str, subscriber: Uuid, status: &str, created: i64) -> Value {
    json!({
        "id": id, "type": "customer.subscription.updated", "created": created,
        "data": {"object": {
            "id": "sub_1", "customer": "cus_1", "status": status,
            "cancel_at_period_end": false, "trial_end": null,
            "metadata": {"subscriber": subscriber.to_string()},
            "items": {"data": [{
                "price": {"id": "price_pro"}, "quantity": 4,
                "current_period_start": created, "current_period_end": created + 2_592_000
            }]}
        }}
    })
}

async fn entitlements(app: &TestApp, uuid: u128) -> Entitlements {
    let billing = sofie_billing::install(app.db(), &PLANS);
    billing
        .entitlements(Subscriber::new(Uuid::from_u128(uuid)))
        .await
        .unwrap()
}

#[tokio::test]
async fn a_trial_without_a_card_grants_the_plan_until_it_ends_once() {
    let (app, _) = started().await;
    let me = app.as_(&member(1));
    assert_eq!(me.get("/members/export").accept_json().await.status, 403);

    let started = me.post("/members/trial").accept_json().await;
    let again = me.post("/members/trial").accept_json().await;

    assert_eq!(started.status, 200, "{}", started.body);
    assert_eq!(me.get("/members/export").accept_json().await.status, 204);
    assert_eq!(again.status, 422);
    assert_eq!(again.json()["error"], "A subscription is already running.");
    app.clock().advance(Duration::from_hours(14 * 24)).unwrap();
    let me = app.as_(&member(1));
    assert_eq!(me.get("/members/export").accept_json().await.status, 403);
    assert_eq!(
        entitlements(&app, 1).await.standing().map(Standing::status),
        Some(Status::Canceled)
    );
    let after = me.post("/members/trial").accept_json().await;
    assert_eq!(after.json()["error"], "The trial was already used.");
}

#[tokio::test]
async fn a_screen_shows_what_the_viewers_plan_holds() {
    let (app, _) = started().await;
    let me = app.as_(&member(2));

    let before = me.get("/members/plan").await;
    me.post("/members/trial").accept_json().await;
    let after = me.get("/members/plan").await;

    assert!(before.body.contains("upgrade"), "{}", before.body);
    assert!(
        after.body.contains(r#"<p id="export">export</p>"#),
        "{}",
        after.body
    );
}

#[tokio::test]
async fn a_limit_refuses_one_more_and_a_quota_is_given_back_each_period() {
    let (app, _) = started().await;
    let me = app.as_(&member(3));
    let calls = || me.post("/members/calls").accept_json();

    assert_eq!(
        me.post("/members/projects/first")
            .accept_json()
            .await
            .status,
        204
    );
    let second = me.post("/members/projects/second").accept_json().await;
    let used: Vec<u16> = [calls().await, calls().await, calls().await]
        .iter()
        .map(|answer| answer.status.as_u16())
        .collect();
    let refused = calls().await;
    app.clock().advance(Duration::from_hours(31 * 24)).unwrap();
    let next_month = app
        .as_(&member(3))
        .post("/members/calls")
        .accept_json()
        .await;

    assert_eq!(second.status, 422);
    assert_eq!(
        second.json()["error"],
        "Your plan allows at most 1 of `project.projects`."
    );
    assert_eq!(used, [204, 204, 422]);
    assert_eq!(
        refused.json()["error"],
        "Your plan's `project.api_calls` for this period is used up."
    );
    assert_eq!(next_month.status, 204);
    assert_eq!(entitlements(&app, 3).await.used(CALLS).await.unwrap(), 1);
}

#[tokio::test]
async fn a_signed_stripe_event_sets_the_subscription_once_and_late_ones_are_dropped() {
    let (app, _) = started().await;
    let owner = Uuid::from_u128(4);
    let created = now(&app);

    let first = stripe_event(&app, &subscription_event("evt_1", owner, "active", created)).await;
    let redelivered =
        stripe_event(&app, &subscription_event("evt_1", owner, "active", created)).await;
    let forged = app
        .post(sofie_billing::WEBHOOK_PATH)
        .header("stripe-signature", &format!("t={created},v1=00"))
        .json(&subscription_event("evt_2", owner, "canceled", created))
        .await;
    let late = subscription_event("evt_0", owner, "past_due", created - 60);
    stripe_event(&app, &late).await;

    assert_eq!(first.status, 200, "{}", first.body);
    assert_eq!(redelivered.json(), first.json());
    assert_eq!(forged.status, 401);
    let held = entitlements(&app, 4).await;
    assert_eq!(held.plan().name(), "pro");
    let standing = held.standing().unwrap();
    assert_eq!(standing.status(), Status::Active);
    assert_eq!(
        standing.renews_at().map(sofie_core::Timestamp::as_second),
        Some(created + 2_592_000)
    );
    assert!(!standing.cancels_at_period_end());
    assert_eq!(held.seats(), Seats::AtMost(4));
    assert_eq!(
        app.query("SELECT id FROM sofie_billing.subscription", &[])
            .await
            .len(),
        1
    );

    let canceled = subscription_event("evt_3", owner, "canceled", created + 10);
    stripe_event(&app, &canceled).await;
    assert_eq!(entitlements(&app, 4).await.plan().name(), "free");
    let changed = sofie_testing::assert_raised::<SubscriptionChanged>(&app);
    let statuses: Vec<Status> = changed.iter().map(|changed| changed.status).collect();
    assert_eq!(statuses, [Status::Active, Status::Canceled]);
}

#[tokio::test]
async fn a_price_the_app_does_not_name_keeps_the_held_plan_and_still_cancels() {
    let (app, _) = started().await;
    let owner = Uuid::from_u128(7);
    let created = now(&app);
    let unnamed = |id: &str, status: &str, at: i64| {
        let mut event = subscription_event(id, owner, status, at);
        event["data"]["object"]["items"]["data"][0]["price"]["id"] = json!("price_unnamed");
        event
    };
    stripe_event(&app, &subscription_event("evt_1", owner, "active", created)).await;

    let switched = stripe_event(&app, &unnamed("evt_2", "past_due", created + 10)).await;
    let held = entitlements(&app, 7).await;
    let canceled = stripe_event(&app, &unnamed("evt_3", "canceled", created + 20)).await;

    assert_eq!(switched.status, 200, "{}", switched.body);
    assert_eq!(held.plan().name(), "pro");
    assert_eq!(held.standing().map(Standing::status), Some(Status::PastDue));
    assert_eq!(canceled.status, 200, "{}", canceled.body);
    assert_eq!(entitlements(&app, 7).await.plan().name(), "free");
}

#[tokio::test]
async fn a_new_subscription_to_a_price_the_app_does_not_name_is_acknowledged_and_grants_nothing() {
    let (app, _) = started().await;
    let owner = Uuid::from_u128(8);
    let mut unnamed = subscription_event("evt_1", owner, "active", now(&app));
    unnamed["data"]["object"]["items"]["data"][0]["price"]["id"] = json!("price_unnamed");
    let mut product = subscription_event("evt_2", owner, "active", now(&app));
    product["data"]["object"]["items"]["data"][0]["price"]["id"] = json!("price_credits");

    let received = stripe_event(&app, &unnamed).await;
    let a_product = stripe_event(&app, &product).await;

    assert_eq!(received.status, 200, "{}", received.body);
    assert_eq!(a_product.status, 200, "{}", a_product.body);
    assert_eq!(entitlements(&app, 8).await.plan().name(), "free");
    let subscriptions = app
        .query("SELECT plan FROM sofie_billing.subscription", &[])
        .await;
    assert!(subscriptions.is_empty(), "{subscriptions:?}");
}

#[tokio::test]
async fn a_new_subscription_of_an_unnamed_price_after_a_cancellation_grants_nothing() {
    let (app, _) = started().await;
    let owner = Uuid::from_u128(10);
    let created = now(&app);
    stripe_event(&app, &subscription_event("evt_1", owner, "active", created)).await;
    stripe_event(
        &app,
        &subscription_event("evt_2", owner, "canceled", created + 10),
    )
    .await;
    let mut unnamed = subscription_event("evt_3", owner, "active", created + 20);
    unnamed["data"]["object"]["id"] = json!("sub_2");
    unnamed["data"]["object"]["items"]["data"][0]["price"]["id"] = json!("price_unnamed");

    let received = stripe_event(&app, &unnamed).await;

    assert_eq!(received.status, 200, "{}", received.body);
    assert_eq!(entitlements(&app, 10).await.plan().name(), "free");
}

#[tokio::test]
async fn a_checkout_and_the_portal_send_the_browser_to_the_provider() {
    let (app, asked) = started().await;
    let owner = Uuid::from_u128(5);
    let me = app.as_(&member(5));

    let refused = me.post("/members/portal").form(&[]).await;
    let checkout = me.post("/members/upgrade").form(&[]).await;
    stripe_event(
        &app,
        &subscription_event("evt_5", owner, "active", now(&app)),
    )
    .await;
    let portal = me.post("/members/portal").form(&[]).await;
    let credits = me.post("/members/credits").accept_json().await;

    assert_eq!(refused.status, 422);
    assert_eq!(checkout.status, 303, "{}", checkout.body);
    assert_eq!(
        checkout.header("location"),
        "https://checkout.stripe.test/c/cs_1"
    );
    assert_eq!(portal.header("location"), "https://billing.stripe.test/p/1");
    let asked = unpoisoned::lock(&asked).clone();
    let (path, form, checkout_key) = &asked[0];
    assert_eq!(path, "/v1/checkout/sessions");
    for field in [
        "mode=subscription",
        "line_items%5B0%5D%5Bprice%5D=price_pro",
        "line_items%5B0%5D%5Bquantity%5D=2",
        "success_url=https%3A%2F%2Fapp.test%2Fmembers%2Fplan",
        &format!("client_reference_id={owner}"),
    ] {
        assert!(form.contains(field), "{field} in {form}");
    }
    assert_eq!(
        credits.json()["location"],
        "https://checkout.stripe.test/c/cs_1"
    );
    let (_, bought, bought_key) = &asked[2];
    for field in [
        "mode=payment",
        "line_items%5B0%5D%5Bprice%5D=price_credits",
        "metadata%5Bquantity%5D=3",
    ] {
        assert!(bought.contains(field), "{field} in {bought}");
    }
    let (portal_path, portal_form, portal_key) = &asked[1];
    assert_eq!(
        (portal_path.as_str(), portal_form.as_str()),
        (
            "/v1/billing_portal/sessions",
            "customer=cus_1&return_url=https%3A%2F%2Fapp.test%2Fmembers%2Fplan"
        )
    );
    assert!(checkout_key.is_some() && portal_key.is_some());
    assert_ne!(checkout_key, bought_key, "each checkout is its own call");
}

#[tokio::test]
async fn a_second_checkout_is_refused_while_the_plan_renews_and_allowed_once_canceled() {
    let (app, asked) = started().await;
    let owner = Uuid::from_u128(15);
    let me = app.as_(&member(15));
    stripe_event(
        &app,
        &subscription_event("evt_15", owner, "active", now(&app)),
    )
    .await;

    let refused = me.post("/members/upgrade").accept_json().await;
    stripe_event(
        &app,
        &subscription_event("evt_16", owner, "canceled", now(&app) + 1),
    )
    .await;
    let allowed = me.post("/members/upgrade").form(&[]).await;

    assert_eq!(refused.status, 422, "{}", refused.body);
    assert_eq!(allowed.status, 303, "{}", allowed.body);
    assert_eq!(
        unpoisoned::lock(&asked).len(),
        1,
        "only the second reached Stripe"
    );
}

#[tokio::test]
async fn a_running_subscription_is_canceled_at_stripe_once_and_a_canceled_one_is_left() {
    let (app, asked) = started().await;
    let owner = Uuid::from_u128(17);
    stripe_event(
        &app,
        &subscription_event("evt_17", owner, "active", now(&app)),
    )
    .await;
    let billing = sofie_billing::install(app.db(), &PLANS);
    let key = sofie_db::IdempotencyKey::generate().unwrap();

    billing
        .cancel_at_provider(Subscriber::new(owner), &key)
        .await
        .unwrap();
    stripe_event(
        &app,
        &subscription_event("evt_18", owner, "canceled", now(&app) + 1),
    )
    .await;
    billing
        .cancel_at_provider(Subscriber::new(owner), &key)
        .await
        .unwrap();

    let asked = unpoisoned::lock(&asked).clone();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(asked[0].0, "/v1/subscriptions/sub_1");
    assert!(asked[0].2.is_some(), "sent with its idempotency key");
}

/// Paddle takes no idempotency key, so a POST it may have applied is never sent twice.
#[tokio::test]
async fn a_paddle_checkout_is_sent_once_without_an_idempotency_key() {
    let (api_url, asked) = fake_provider().await;
    let app = start(setup(Vendor::Paddle, &api_url)).await;

    let checkout = app.as_(&member(7)).post("/members/upgrade").form(&[]).await;

    assert!(checkout.status.is_server_error(), "{}", checkout.body);
    let asked = unpoisoned::lock(&asked).clone();
    let paths: Vec<(&str, Option<&str>)> = asked
        .iter()
        .map(|(path, _, key)| (path.as_str(), key.as_deref()))
        .collect();
    assert_eq!(paths, [("/transactions", None)]);
}

#[tokio::test]
async fn a_paddle_one_time_purchase_is_kept_once_and_announced() {
    let (api_url, _) = fake_provider().await;
    let app = start(setup(Vendor::Paddle, &api_url)).await;
    let owner = Uuid::from_u128(6);
    let body = json!({
        "event_id": "evt_01", "event_type": "transaction.completed",
        "occurred_at": "2026-10-14T10:00:00Z",
        "data": {
            "id": "txn_01", "subscription_id": null,
            "custom_data": {"subscriber": owner.to_string()},
            "items": [{"price": {"id": "price_credits"}, "quantity": 5}]
        }
    })
    .to_string();
    let deliver = |id: &str| {
        let body = body.replace("evt_01", id);
        let timestamp = now(&app).to_string();
        let signature = sign(SECRET, &timestamp, ":", &body);
        app.post(sofie_billing::WEBHOOK_PATH)
            .header(
                "paddle-signature",
                &format!("ts={timestamp};h1={signature}"),
            )
            .header("content-type", "application/json")
            .body(body)
    };

    let first = deliver("evt_01").await;
    let same_purchase_again = deliver("evt_02").await;

    assert_eq!(first.status, 200, "{}", first.body);
    assert_eq!(same_purchase_again.status, 200);
    let purchases = app
        .query("SELECT product, quantity FROM sofie_billing.purchase", &[])
        .await;
    assert_eq!(
        purchases,
        [vec![SqlValue::Text("credits".to_owned()), SqlValue::Int(5)]]
    );
    let announced = sofie_testing::assert_raised::<PurchaseCompleted>(&app);
    assert_eq!(
        announced,
        [PurchaseCompleted {
            subscriber: owner,
            product: "credits".to_owned(),
            quantity: 5
        }]
    );
}

#[tokio::test]
async fn a_purchase_of_a_price_the_app_does_not_name_is_acknowledged_and_kept_nowhere() {
    let (api_url, _) = fake_provider().await;
    let app = start(setup(Vendor::Paddle, &api_url)).await;
    for (event, price) in [("evt_01", "price_unnamed"), ("evt_02", "price_pro")] {
        let body = json!({
            "event_id": event, "event_type": "transaction.completed",
            "occurred_at": "2026-10-14T10:00:00Z",
            "data": {
                "id": format!("txn_{event}"), "subscription_id": null,
                "custom_data": {"subscriber": Uuid::from_u128(9).to_string()},
                "items": [{"price": {"id": price}, "quantity": 1}]
            }
        })
        .to_string();
        let timestamp = now(&app).to_string();
        let signature = sign(SECRET, &timestamp, ":", &body);

        let received = app
            .post(sofie_billing::WEBHOOK_PATH)
            .header(
                "paddle-signature",
                &format!("ts={timestamp};h1={signature}"),
            )
            .header("content-type", "application/json")
            .body(body)
            .await;

        assert_eq!(received.status, 200, "{price}: {}", received.body);
    }
    let purchases = app
        .query("SELECT product FROM sofie_billing.purchase", &[])
        .await;
    assert!(purchases.is_empty(), "{purchases:?}");
}

async fn ask_seats(app: &TestApp, owner: Uuid, seats: u32) -> sofie_core::Result<()> {
    let billing = sofie_billing::install(app.db(), &PLANS);
    let mut asked = billing.change_seats(Subscriber::new(owner), seats).await?;
    app.db().commit(&mut asked).await
}

fn sent(asked: &Asked, path: &str) -> Vec<(String, Option<String>)> {
    unpoisoned::lock(asked)
        .iter()
        .filter(|(sent, _, _)| sent == path)
        .map(|(_, body, key)| (body.clone(), key.clone()))
        .collect()
}

#[tokio::test]
async fn seats_change_at_stripe_after_the_commit_with_one_key_across_retries() {
    let (app, asked) = started().await;
    let owner = Uuid::from_u128(10);
    let unpaid = ask_seats(&app, owner, 6).await.unwrap_err();
    stripe_event(
        &app,
        &subscription_event("evt_10", owner, "active", now(&app)),
    )
    .await;

    ask_seats(&app, owner, 5).await.unwrap();
    ask_seats(&app, owner, 6).await.unwrap();
    let before_the_job = sent(&asked, "/v1/subscription_items/si_1");
    let failures = sofie_testing::drain_failing(&app).await;
    app.advance(Duration::from_mins(1)).await;

    assert_eq!(
        unpaid.to_string(),
        "refused: Nothing has been paid for yet."
    );
    assert!(before_the_job.is_empty(), "{before_the_job:?}");
    assert_eq!(failures.len(), 1, "{failures:?}");
    let updates = sent(&asked, "/v1/subscription_items/si_1");
    assert_eq!(updates.len(), 2, "{updates:?}");
    assert_eq!(
        updates[0], updates[1],
        "a retry sends the same change and key"
    );
    assert_eq!(
        updates[0].0,
        "quantity=6&proration_behavior=create_prorations"
    );
    assert!(updates[0].1.is_some());
    assert_eq!(entitlements(&app, 10).await.seats(), Seats::AtMost(4));
}

const FIVE_SEATS: &str = "quantity=5&proration_behavior=create_prorations";
const SEVEN_SEATS: &str = "quantity=7&proration_behavior=create_prorations";

#[tokio::test]
async fn a_seat_count_landing_after_a_later_one_is_followed_by_the_later_one() {
    let hold = Arc::new(Hold::default());
    let (api_url, asked) = fake_provider_holding(Arc::clone(&hold)).await;
    let app = start(setup(Vendor::Stripe, &api_url)).await;
    let owner = Uuid::from_u128(14);
    stripe_event(
        &app,
        &subscription_event("evt_14", owner, "active", now(&app)),
    )
    .await;
    ask_seats(&app, owner, 4).await.unwrap();
    sofie_testing::drain_failing(&app).await;
    app.advance(Duration::from_mins(1)).await;
    *unpoisoned::lock(&hold.body) = Some(FIVE_SEATS.to_owned());
    ask_seats(&app, owner, 5).await.unwrap();

    let later = async {
        hold.arrived.notified().await;
        ask_seats(&app, owner, 7).await.unwrap();
        while sent(&asked, "/v1/subscription_items/si_1")
            .iter()
            .all(|(body, _)| body != SEVEN_SEATS)
        {
            app.drain().await;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        hold.release.notify_one();
    };
    tokio::join!(app.drain(), later);
    app.drain().await;

    let updates = sent(&asked, "/v1/subscription_items/si_1");
    let bodies: Vec<&str> = updates.iter().map(|(body, _)| body.as_str()).collect();
    assert!(bodies.contains(&FIVE_SEATS), "{bodies:?}");
    assert_eq!(bodies.last(), Some(&SEVEN_SEATS), "{bodies:?}");
}

#[tokio::test]
async fn a_seat_count_asked_of_a_replaced_subscription_is_not_sent_to_the_new_one() {
    let (app, asked) = started().await;
    let owner = Uuid::from_u128(15);
    let created = now(&app);
    stripe_event(
        &app,
        &subscription_event("evt_15", owner, "active", created),
    )
    .await;
    ask_seats(&app, owner, 5).await.unwrap();
    let mut replaced = subscription_event("evt_16", owner, "active", created + 10);
    replaced["data"]["object"]["id"] = json!("sub_2");
    stripe_event(&app, &replaced).await.assert_ok();

    let failures = sofie_testing::drain_failing(&app).await;

    assert!(failures.is_empty(), "{failures:?}");
    assert_eq!(sent(&asked, "/v1/subscriptions/sub_2"), []);
    assert_eq!(sent(&asked, "/v1/subscription_items/si_1"), []);
}

#[tokio::test]
async fn seats_change_at_paddle_by_sending_the_held_price_with_the_new_count() {
    let (api_url, asked) = fake_provider().await;
    let app = start(setup(Vendor::Paddle, &api_url)).await;
    let owner = Uuid::from_u128(11);
    let body = json!({
        "event_id": "evt_11", "event_type": "subscription.created",
        "occurred_at": "2026-10-14T10:00:00Z",
        "data": {
            "id": "sub_01", "status": "active", "customer_id": "ctm_01",
            "custom_data": {"subscriber": owner.to_string()},
            "items": [{"price": {"id": "price_pro"}, "quantity": 2}]
        }
    })
    .to_string();
    let timestamp = now(&app).to_string();
    let signature = sign(SECRET, &timestamp, ":", &body);
    app.post(sofie_billing::WEBHOOK_PATH)
        .header(
            "paddle-signature",
            &format!("ts={timestamp};h1={signature}"),
        )
        .header("content-type", "application/json")
        .body(body)
        .await;

    ask_seats(&app, owner, 3).await.unwrap();
    app.drain().await;

    let calls = sent(&asked, "/subscriptions/sub_01");
    assert_eq!(calls.len(), 2, "{calls:?}");
    let update: Value = serde_json::from_str(&calls[1].0).unwrap();
    assert_eq!(
        update,
        json!({
            "items": [{"price_id": "price_pro", "quantity": 3}],
            "proration_billing_mode": "prorated_next_billing_period"
        })
    );
    assert_eq!(calls[1].1, None);
}

#[tokio::test]
async fn seats_change_only_on_a_plan_billed_per_seat() {
    let (app, _) = started().await;
    let owner = Uuid::from_u128(12);
    let billing = sofie_billing::install(app.db(), &PLANS);
    let mut trial = billing
        .start_trial(Subscriber::new(owner), PRO)
        .await
        .unwrap();
    app.db().commit(&mut trial).await.unwrap();

    let none = ask_seats(&app, Uuid::from_u128(13), 0).await.unwrap_err();
    let unpaid_trial = ask_seats(&app, owner, 3).await.unwrap_err();
    let created = now(&app);
    stripe_event(
        &app,
        &subscription_event("evt_12", owner, "active", created),
    )
    .await;
    stripe_event(
        &app,
        &subscription_event("evt_13", owner, "canceled", created + 10),
    )
    .await;
    let canceled = ask_seats(&app, owner, 3).await.unwrap_err();

    assert!(none.to_string().contains("seats"), "{none}");
    assert_eq!(
        unpaid_trial.to_string(),
        "refused: Nothing has been paid for yet."
    );
    assert_eq!(
        canceled.to_string(),
        "refused: The plan held is not billed per seat."
    );
}
