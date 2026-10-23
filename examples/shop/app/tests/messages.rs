use std::num::NonZeroU16;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use order::Order;
use order::actions::PlaceOrder;
use order_contract::routes;
use order_contract::{Customer, OrderCancelled};
use serde_json::json;
use sofie::core::{Error, Result, SqlValue};
use sofie::db::{Id, Messages, Reach};
use sofie::worker::{Worker, publish_heard};
use sofie_identity::testing::TestMember as _;
use sofie_testing::{TestApp, TestPrincipal, TestSession, fixture_input, test_app};
use tokio::sync::oneshot;

const HOUR: Duration = Duration::from_hours(1);

mod support;

async fn signed_in_customer(app: &TestApp) -> TestSession {
    let customer = support::customer(app, "ada@example.com").await;
    // A workspace build turns on sofie-identity's mailed links, whose sign-up mail is not the order flow under test.
    app.query(
        "DELETE FROM sofie.messages WHERE name LIKE 'sofie_identity.%'",
        &[],
    )
    .await;
    customer
}

async fn place_order(app: &TestApp, customer: &TestSession) -> String {
    let total = json!({ "minor": 1_000, "currency": "EUR" });
    let order = json!({ "total": total });
    let placed = app
        .request(&routes::customer::place_order())
        .session(customer)
        .json(&order)
        .await;
    assert_eq!(placed.status, 200, "{}", placed.body);
    placed.json()["id"].as_str().unwrap().to_owned()
}

async fn cancel_order(app: &TestApp, customer: &TestSession, id: &str) {
    let cancelled = app
        .request(&routes::customer::cancel_order(Id::<Order>::from_uuid(
            id.parse().unwrap(),
        )))
        .session(customer)
        .accept_json()
        .await;
    assert_eq!(cancelled.status, 200, "{}", cancelled.body);
}

fn text(value: &SqlValue) -> String {
    match value {
        SqlValue::Text(text) => text.clone(),
        other => panic!("expected text, got {other:?}"),
    }
}

async fn audit(app: &TestApp, order: &str) -> Vec<String> {
    let sql = "SELECT action FROM \"order\".audit_entry WHERE \"order\"::text = $1 ORDER BY action";
    let rows = app.query(sql, &[SqlValue::Text(order.to_owned())]).await;
    rows.iter().map(|row| text(&row[0])).collect()
}

async fn order_status(app: &TestApp, order: &str) -> String {
    let sql = "SELECT status FROM \"order\".\"order\" WHERE id::text = $1";
    let rows = app.query(sql, &[SqlValue::Text(order.to_owned())]).await;
    text(&rows[0][0])
}

async fn messages(app: &TestApp) -> Vec<Vec<SqlValue>> {
    let sql = "SELECT name, attempts, last_error FROM sofie.messages ORDER BY name";
    app.query(sql, &[]).await
}

fn advance(app: &TestApp, by: Duration) {
    app.clock().advance(by).unwrap();
}

#[test_app]
async fn a_cancellation_is_audited_once_the_worker_drains(app: TestApp) {
    let customer = signed_in_customer(&app).await;
    let order = place_order(&app, &customer).await;
    cancel_order(&app, &customer, &order).await;

    assert_eq!(audit(&app, &order).await, Vec::<String>::new());

    assert_eq!(app.drain().await, 4);
    assert_eq!(audit(&app, &order).await, ["CANCELLED", "PLACED"]);
    assert_eq!(messages(&app).await, Vec::<Vec<SqlValue>>::new());
    assert_eq!(app.drain().await, 0);
}

#[test]
fn an_event_declared_in_the_contract_is_named_after_its_module() {
    assert_eq!(
        <OrderCancelled as sofie::db::Event>::NAME,
        "order.OrderCancelled"
    );
}

async fn refuse(_event: OrderCancelled) -> Result<()> {
    Err(Error::Unavailable("the warehouse is closed".to_owned()))
}

async fn attempts(app: &TestApp) -> Vec<SqlValue> {
    let sql = "SELECT attempts FROM sofie.messages WHERE name = 'test.refuse'";
    app.query(sql, &[]).await.into_iter().flatten().collect()
}

#[test_app]
async fn a_failing_listener_backs_off_and_is_left_dead_after_its_attempts(app: TestApp) {
    let customer = signed_in_customer(&app).await;
    let worker = Worker::new(app.db().clone()).on::<OrderCancelled, _>("test.refuse", refuse);
    publish_heard(&worker);
    let order = place_order(&app, &customer).await;
    cancel_order(&app, &customer, &order).await;

    assert_eq!(
        worker.drain().await.unwrap(),
        0,
        "a failed job is not counted"
    );
    assert_eq!(attempts(&app).await, [SqlValue::Int(1)]);
    for attempt in 1..10_i64 {
        let backoff = Duration::from_secs(1 << (attempt - 1));
        advance(&app, backoff.checked_sub(Duration::from_millis(1)).unwrap());
        worker.drain().await.unwrap();
        assert_eq!(
            attempts(&app).await,
            [SqlValue::Int(attempt)],
            "attempt {attempt} waits"
        );
        advance(&app, Duration::from_millis(1));
        worker.drain().await.unwrap();
        assert_eq!(
            attempts(&app).await,
            [SqlValue::Int(attempt + 1)],
            "attempt {attempt} retries"
        );
    }
    advance(&app, HOUR * 24);

    worker.drain().await.unwrap();
    let refused = "SELECT attempts, last_error, dead_at IS NOT NULL FROM sofie.messages \
                   WHERE name = 'test.refuse'";
    assert_eq!(
        app.query(refused, &[]).await,
        [[
            SqlValue::Int(10),
            SqlValue::Text("unavailable: the warehouse is closed".to_owned()),
            SqlValue::Bool(true),
        ]]
    );
}

static NOTED: AtomicUsize = AtomicUsize::new(0);

async fn note(_event: OrderCancelled) -> Result<()> {
    NOTED.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

#[test_app]
async fn listeners_of_one_event_retry_apart_so_a_failing_one_reruns_no_other(app: TestApp) {
    let customer = signed_in_customer(&app).await;
    let worker = Worker::new(app.db().clone())
        .on::<OrderCancelled, _>("test.note", note)
        .on::<OrderCancelled, _>("test.refuse", refuse);
    publish_heard(&worker);
    let order = place_order(&app, &customer).await;
    cancel_order(&app, &customer, &order).await;

    worker.drain().await.unwrap();
    advance(&app, Duration::from_secs(1));
    worker.drain().await.unwrap();

    assert_eq!(NOTED.load(Ordering::Relaxed), 1);
    assert_eq!(attempts(&app).await, [SqlValue::Int(2)]);
    let names: Vec<SqlValue> = messages(&app)
        .await
        .into_iter()
        .map(|row| row[0].clone())
        .collect();
    assert_eq!(
        names,
        [
            SqlValue::Text("order.listeners.audit_cancellation".to_owned()),
            SqlValue::Text("order.listeners.audit_placement".to_owned()),
            SqlValue::Text("test.refuse".to_owned())
        ]
    );
}

#[test_app]
async fn a_lost_lease_hands_the_work_on_and_refuses_the_late_finish(app: TestApp) {
    let customer = signed_in_customer(&app).await;
    let order = place_order(&app, &customer).await;
    let messages = Messages::new(app.db().clone());
    let lease = Duration::from_millis(200);
    let crashed = messages.claim(lease, 10).await.unwrap();
    let worker = Worker::new(app.db().clone()).on::<order_contract::OrderPlaced, _>(
        "order.listeners.audit_placement",
        order::listeners::audit_placement,
    );

    assert_eq!(crashed.len(), 1);
    assert_eq!(worker.drain().await.unwrap(), 0);
    // Leases run on the database's clock, which the test clock does not move.
    tokio::time::sleep(lease).await;
    assert_eq!(worker.drain().await.unwrap(), 1);

    let late = app
        .db()
        .commit_with(&mut (), vec![messages.completion(&crashed[0])], Reach::All)
        .await;
    assert!(matches!(late, Err(Error::Conflict)), "{late:?}");
    assert_eq!(audit(&app, &order).await, ["PLACED"]);
}

// `#[test_app]` runs on one thread, where the two workers would take turns instead of racing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_workers_run_each_job_once() {
    let app = support::shop().await;
    let customer = signed_in_customer(&app).await;
    let mut orders = Vec::new();
    for _ in 0..20 {
        orders.push(place_order(&app, &customer).await);
    }
    let first = shop::worker(app.db().clone());
    let second = shop::worker(app.db().clone());

    let (ran_first, ran_second) = tokio::join!(first.drain(), second.drain());

    assert_eq!(ran_first.unwrap() + ran_second.unwrap(), 22);
    for order in &orders {
        assert_eq!(audit(&app, order).await, ["PLACED"]);
    }
    assert_eq!(messages(&app).await, Vec::<Vec<SqlValue>>::new());
}

fn expiry_worker(app: &TestApp) -> Worker {
    Worker::new(app.db().clone()).each(
        "order.jobs.unpaid_orders",
        HOUR,
        order::jobs::unpaid_orders,
        "order.jobs.expire_unpaid",
        order::jobs::expire_unpaid,
    )
}

#[test_app]
async fn the_expiry_runs_hourly_and_expires_each_order_left_pending_a_day(app: TestApp) {
    let customer = signed_in_customer(&app).await;
    let worker = expiry_worker(&app);
    let order = place_order(&app, &customer).await;

    assert_eq!(worker.drain().await.unwrap(), 1);
    assert_eq!(worker.drain().await.unwrap(), 0);
    advance(&app, HOUR.checked_sub(Duration::from_secs(1)).unwrap());
    assert_eq!(worker.drain().await.unwrap(), 0);
    advance(&app, Duration::from_secs(1));
    assert_eq!(worker.drain().await.unwrap(), 1);
    assert_eq!(order_status(&app, &order).await, "PENDING");

    advance(&app, HOUR * 24);
    assert_eq!(worker.drain().await.unwrap(), 2, "the query and one expiry");
    assert_eq!(order_status(&app, &order).await, "EXPIRED");
}

#[test_app]
async fn an_order_left_unpaid_expires_once_its_payment_window_passes(app: TestApp) {
    let customer = TestPrincipal::<Customer>::member(&app).await;
    let order = app
        .as_(&customer)
        .request(&routes::customer::place_order())
        .json(&fixture_input::<PlaceOrder>(&app))
        .await
        .record_id()
        .to_string();

    app.advance(HOUR * 23).await;
    assert_eq!(order_status(&app, &order).await, "PENDING");
    app.advance(HOUR * 2).await;
    assert_eq!(order_status(&app, &order).await, "EXPIRED");
}

#[test_app]
async fn a_worker_asks_about_a_schedule_again_only_once_it_is_due(app: TestApp) {
    signed_in_customer(&app).await;
    let worker = expiry_worker(&app);
    let other = expiry_worker(&app);
    assert_eq!(worker.drain().await.unwrap(), 1);

    let now = app.clock().clock().now().timestamp();
    app.query(
        "UPDATE sofie.schedules SET next_at = $1",
        &[SqlValue::Timestamp(now)],
    )
    .await;
    assert_eq!(
        worker.drain().await.unwrap(),
        0,
        "not due by the time it read"
    );
    assert_eq!(
        other.drain().await.unwrap(),
        1,
        "a worker that never read it asks"
    );

    advance(&app, HOUR);
    assert_eq!(worker.drain().await.unwrap(), 1);
    assert_eq!(
        other.drain().await.unwrap(),
        0,
        "it asks and learns it was fired"
    );
}

async fn audited_within(app: &TestApp, order: &str, limit: Duration) -> Vec<String> {
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        let audit = audit(app, order).await;
        if !audit.is_empty() || tokio::time::Instant::now() >= deadline {
            return audit;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[test_app]
async fn a_running_worker_is_woken_by_a_commit_and_stops_when_told(app: TestApp) {
    let customer = signed_in_customer(&app).await;
    let worker = shop::worker(app.db().clone());
    let (stop_worker, worker_stopped) = oneshot::channel::<()>();
    let running = tokio::spawn(worker.run(async {
        let _ = worker_stopped.await;
    }));
    let first = place_order(&app, &customer).await;
    assert_eq!(
        audited_within(&app, &first, Duration::from_secs(3)).await,
        ["PLACED"]
    );

    // The commit wakes whoever listens, the idle worker too, instead of its next poll.
    let messages = Messages::new(app.db().clone());
    let mut wakeups = messages
        .wakeups()
        .expect("Postgres sends wake-ups")
        .await
        .unwrap();
    let second = place_order(&app, &customer).await;

    tokio::time::timeout(Duration::from_secs(10), wakeups.next())
        .await
        .expect("the commit sends a wake-up")
        .unwrap();
    assert_eq!(
        audited_within(&app, &second, Duration::from_secs(10)).await,
        ["PLACED"]
    );
    stop_worker.send(()).unwrap();
    running.await.unwrap().unwrap();
}

#[test_app]
async fn a_message_whose_worker_stopped_during_its_last_attempt_is_left_dead(app: TestApp) {
    let customer = signed_in_customer(&app).await;
    place_order(&app, &customer).await;
    let lease = Duration::from_millis(200);
    let crashed = Messages::new(app.db().clone())
        .claim(lease, 10)
        .await
        .unwrap();
    let worker = Worker::new(app.db().clone())
        .on::<order_contract::OrderPlaced, _>(
            "order.listeners.audit_placement",
            order::listeners::audit_placement,
        )
        .max_attempts(NonZeroU16::MIN);

    assert_eq!(crashed.len(), 1);
    // Leases run on the database's clock, which the test clock does not move.
    tokio::time::sleep(lease).await;
    assert_eq!(worker.drain().await.unwrap(), 0);

    let dead = "SELECT last_error FROM sofie.messages WHERE dead_at IS NOT NULL";
    assert_eq!(
        app.query(dead, &[]).await,
        [[SqlValue::Text(
            "its worker stopped during its last attempt".to_owned()
        )]]
    );
}
