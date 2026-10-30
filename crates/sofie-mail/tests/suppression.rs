use std::time::Duration;

use ring::hmac;
use serde_json::json;
use sofie_core::encoding::base64::{self, STANDARD};
use sofie_core::encoding::hex;
use sofie_core::{Result, SqlValue};
use sofie_db::worker::Worker;
use sofie_db::{Db, Queued};
use sofie_http::{App, Routes, Web};
use sofie_mail::testing::{BASE_URL, Outbox};
use sofie_mail::web::Events;
use sofie_mail::{Address, Body, Mail, Mailer, Scope, scopes};
use sofie_testing::{TestApp, sent_mails};
use sofie_values::{Email, Secret};

fn ada() -> Address {
    Address::new(&Email::parse("ada@example.com").unwrap()).unwrap()
}

fn body() -> Body {
    sofie_mail::testing::body("<p>Hello</p>")
}

async fn news(mailer: Mailer) -> Result<Queued> {
    mailer.send(Mail::new(ada(), "News", body())?.bulk("news")?)
}

fn bob() -> Address {
    Address::new(&Email::parse("bob@example.com").unwrap()).unwrap()
}

async fn news_to_two(mailer: Mailer) -> Result<Queued> {
    mailer.send(
        Mail::new(ada(), "News", body())?
            .bulk("news")?
            .also_to(bob()),
    )
}

async fn receipt_to_two(mailer: Mailer) -> Result<Queued> {
    let mail = Mail::new(ada(), "Your receipt", body())?;
    mailer.send(mail.also_to(bob()))
}

async fn receipt(mailer: Mailer) -> Result<Queued> {
    mailer.send(Mail::new(ada(), "Your receipt", body())?)
}

fn events() -> Events {
    Events {
        postmark: Some(Secret::new("hook:secret".to_owned())),
        mailgun: None,
        signed: Some(Secret::new("signing-key".to_owned())),
    }
}

async fn started() -> TestApp {
    sofie_testing::start(
        |db: Db| async move {
            db.schema().migrate(&[]).await?;
            Ok(
                App::new(db).area::<Web>("shop", |routes: &mut Routes<Web>| {
                    routes.post("/news", news);
                    routes.post("/news-to-two", news_to_two);
                    routes.post("/receipts", receipt);
                    routes.post("/receipts-to-two", receipt_to_two);
                    sofie_mail::web::routes(routes, &events());
                }),
            )
        },
        Worker::new,
    )
    .await
}

async fn sent_after(app: &TestApp, path: &str) -> usize {
    let before = sent_mails(app).len();
    app.post(path).await.assert_ok();
    app.drain().await;
    sent_mails(app).len() - before
}

#[tokio::test]
async fn a_bulk_mail_unsubscribes_in_one_click_and_transactional_mail_still_goes() {
    let app = started().await;
    assert_eq!(sent_after(&app, "/news").await, 1);
    let link = sent_mails(&app)[0].unsubscribe().unwrap().to_owned();
    let path = link.strip_prefix(BASE_URL).unwrap().to_owned();
    assert!(path.starts_with("/mail/unsubscribe?token="), "{path}");

    let page = app.get(&path).accept_json().await.json();
    assert_eq!(
        (page["asking"].clone(), page["list"].clone()),
        (json!(true), json!("news"))
    );
    assert!(
        page["action"]
            .as_str()
            .unwrap()
            .starts_with("/mail/unsubscribe/confirm?token=")
    );
    app.post(&path)
        .form(&[("List-Unsubscribe", "One-Click")])
        .await
        .assert_ok();

    assert_eq!(app.get(&path).accept_json().await.json()["done"], true);
    assert_eq!(sent_after(&app, "/news").await, 0);
    assert_eq!(sent_after(&app, "/receipts").await, 1);
    let forged = format!("{path}x");
    assert_eq!(app.get(&forged).accept_json().await.json()["invalid"], true);
}

#[tokio::test]
async fn a_verified_bounce_stops_all_mail_and_a_forged_one_nothing() {
    let app = started().await;
    let report = json!({ "RecordType": "Bounce", "Type": "HardBounce", "Email": "ada@example.com", "ID": 42 });
    let authorization = format!("Basic {}", base64::encode(&STANDARD, b"hook:secret"));

    let forged = app
        .post("/mail/events/postmark")
        .header("authorization", "Basic aG9vazp3cm9uZw==")
        .json(&report)
        .await;
    assert_eq!(forged.status, 401, "{}", forged.body);
    assert_eq!(sent_after(&app, "/receipts").await, 1);

    app.post("/mail/events/postmark")
        .header("authorization", &authorization)
        .json(&report)
        .await
        .assert_ok();

    assert_eq!(sent_after(&app, "/receipts").await, 0);
}

async fn report_signed(app: &TestApp, report: &serde_json::Value) {
    let report = report.to_string();
    let key = hmac::Key::new(hmac::HMAC_SHA256, b"signing-key");
    let signature = hex::encode(hmac::sign(&key, report.as_bytes()).as_ref());
    app.post("/mail/events/signed")
        .header("x-sofie-signature", &signature)
        .header("content-type", "application/json")
        .body(report)
        .await
        .assert_ok();
}

#[tokio::test]
async fn a_signed_complaint_stops_bulk_mail_only() {
    let app = started().await;

    report_signed(
        &app,
        &json!({ "id": "c-1", "event": "complaint", "email": "ada@example.com" }),
    )
    .await;

    assert_eq!(sent_after(&app, "/news").await, 0);
    assert_eq!(sent_after(&app, "/receipts").await, 1);
}

#[tokio::test]
async fn a_bulk_mail_to_two_recipients_is_refused_before_it_is_queued() {
    let app = started().await;

    let answer = app.post("/news-to-two").await;

    assert_eq!(answer.status, 422, "{}", answer.body);
    assert_eq!(app.drain().await, 0);
    assert!(sent_mails(&app).is_empty());
}

#[tokio::test]
async fn a_mail_to_a_bounced_and_a_reachable_address_goes_to_the_reachable_one() {
    let app = started().await;
    report_signed(
        &app,
        &json!({ "id": "b-1", "event": "bounce", "email": "ada@example.com" }),
    )
    .await;

    assert_eq!(sent_after(&app, "/receipts-to-two").await, 1);

    assert_eq!(sent_mails(&app)[0].to(), [bob()]);
}

async fn refused_by_the_server(app: &TestApp, path: &str, email: &str, code: u16) -> Outbox {
    let outbox = sofie_mail::testing::fake(app.db());
    outbox.refuse(email, code);
    app.post(path).await.assert_ok();
    sofie_testing::drain_failing(app).await;
    outbox
}

async fn deliveries_left(app: &TestApp) -> Vec<Vec<SqlValue>> {
    app.query(
        "SELECT attempts, dead_at IS NOT NULL, last_error FROM sofie.messages \
         WHERE name = 'sofie_mail.deliver'",
        &[],
    )
    .await
}

#[tokio::test]
async fn a_mail_refused_for_good_is_buried_at_once_and_its_address_suppressed() {
    let app = started().await;

    let outbox = refused_by_the_server(&app, "/receipts", "ada@example.com", 550).await;

    let left = deliveries_left(&app).await;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0][..2], [SqlValue::Int(1), SqlValue::Bool(true)]);
    let SqlValue::Text(error) = &left[0][2] else {
        panic!("a buried message keeps its error");
    };
    assert!(error.contains("refused every recipient: 550"), "{error}");
    assert!(!error.contains("ada@"), "{error}");
    assert_eq!(held(&app, "ada@example.com").await, [Scope::All]);
    assert!(outbox.mails().is_empty());
}

#[tokio::test]
async fn a_mail_every_recipient_refused_is_traced_as_refused() {
    let app = started().await;
    let telemetry = sofie_telemetry::capture();

    refused_by_the_server(&app, "/receipts", "ada@example.com", 554).await;

    let spans = telemetry.spans();
    let sent = spans
        .iter()
        .find(|span| span.name == "mail")
        .expect("a span per send");
    assert_eq!(sent.fields["outcome"], "refused");
}

async fn waits_for_greylisting(app: &TestApp) -> bool {
    let due = app
        .query(
            "SELECT available_at > now() + interval '4 minutes' FROM sofie.messages \
             WHERE name = 'sofie_mail.deliver'",
            &[],
        )
        .await;
    due[0][0] == SqlValue::Bool(true)
}

#[tokio::test]
async fn a_mail_refused_for_now_waits_for_greylisting_and_suppresses_nothing() {
    for code in [421, 451] {
        let app = started().await;

        refused_by_the_server(&app, "/receipts", "ada@example.com", code).await;

        let left = deliveries_left(&app).await;
        assert_eq!(
            left[0][..2],
            [SqlValue::Int(0), SqlValue::Bool(false)],
            "{code}"
        );
        assert!(waits_for_greylisting(&app).await, "{code}");
        assert!(held(&app, "ada@example.com").await.is_empty(), "{code}");
    }
}

#[tokio::test]
async fn a_mail_a_server_defers_for_hours_is_buried_as_refused() {
    let app = started().await;
    refused_by_the_server(&app, "/receipts", "ada@example.com", 451).await;

    for _ in 0..30 {
        app.clock().advance(Duration::from_mins(5)).unwrap();
        sofie_testing::drain_failing(&app).await;
    }

    let left = deliveries_left(&app).await;
    assert_eq!(left.len(), 1, "{left:?}");
    assert_eq!(left[0][1], SqlValue::Bool(true));
    let SqlValue::Text(error) = &left[0][2] else {
        panic!("a buried message keeps its error");
    };
    assert!(error.contains("refused every recipient: 451"), "{error}");
}

#[tokio::test]
async fn a_mail_one_recipient_refused_goes_to_the_other_and_suppresses_the_refused_one() {
    let app = started().await;

    let outbox = refused_by_the_server(&app, "/receipts-to-two", "bob@example.com", 550).await;

    assert_eq!(deliveries_left(&app).await, Vec::<Vec<SqlValue>>::new());
    assert_eq!(outbox.mails()[0].to(), [ada()]);
    assert_eq!(held(&app, "bob@example.com").await, [Scope::All]);
}

#[tokio::test]
async fn a_recipient_refused_for_now_holds_the_mail_from_nobody_else() {
    let app = started().await;

    let outbox = refused_by_the_server(&app, "/receipts-to-two", "bob@example.com", 452).await;

    assert_eq!(outbox.mails()[0].to(), [ada()]);
    let left = deliveries_left(&app).await;
    assert_eq!(left.len(), 1, "{left:?}");
    assert_eq!(
        left[0][..2],
        [SqlValue::Int(0), SqlValue::Bool(false)],
        "bob's mail waits for a retry"
    );
    assert!(
        waits_for_greylisting(&app).await,
        "a greylisting server is not asked again at once"
    );
    assert_eq!(held(&app, "bob@example.com").await, Vec::<Scope>::new());
}

#[tokio::test]
async fn a_mail_every_recipient_refused_for_good_suppresses_each_of_them() {
    let app = started().await;
    let outbox = sofie_mail::testing::fake(app.db());
    outbox.refuse("ada@example.com", 550);
    outbox.refuse("bob@example.com", 551);

    app.post("/receipts-to-two").await.assert_ok();
    sofie_testing::drain_failing(&app).await;

    assert_eq!(deliveries_left(&app).await[0][1], SqlValue::Bool(true));
    for email in ["ada@example.com", "bob@example.com"] {
        assert_eq!(held(&app, email).await, [Scope::All], "{email}");
    }
}

#[tokio::test]
async fn a_refusal_other_than_a_missing_mailbox_is_buried_but_suppresses_nothing() {
    let app = started().await;

    refused_by_the_server(&app, "/receipts", "ada@example.com", 554).await;

    assert_eq!(deliveries_left(&app).await[0][1], SqlValue::Bool(true));
    assert_eq!(held(&app, "ada@example.com").await, Vec::<Scope>::new());
}

async fn fail_suppressions(app: &TestApp, failing: &str) {
    app.query("CREATE SEQUENCE sofie.suppression_tries", &[])
        .await;
    let function = format!(
        "CREATE FUNCTION sofie.refuse_suppression() RETURNS trigger AS $$ BEGIN \
         IF {failing} THEN RAISE EXCEPTION 'suppressions are down' USING ERRCODE = '23505'; END IF; \
         RETURN NEW; END $$ LANGUAGE plpgsql"
    );
    app.query(&function, &[]).await;
    app.query(
        "CREATE TRIGGER refuse BEFORE INSERT ON sofie.mail_suppressions \
         FOR EACH ROW EXECUTE FUNCTION sofie.refuse_suppression()",
        &[],
    )
    .await;
}

async fn held(app: &TestApp, email: &str) -> Vec<Scope> {
    scopes(&app.db().read(), email).await.unwrap()
}

async fn answered_keys(app: &TestApp) -> SqlValue {
    let answered = app
        .query(
            "SELECT count(*) FROM sofie.idempotency WHERE answered_at IS NOT NULL",
            &[],
        )
        .await;
    answered[0][0].clone()
}

#[tokio::test]
async fn a_sent_mail_is_recorded_as_sent_even_when_suppressing_its_bounce_fails() {
    let app = started().await;
    fail_suppressions(&app, "true").await;

    let outbox = refused_by_the_server(&app, "/receipts-to-two", "bob@example.com", 550).await;

    assert_eq!(outbox.mails().len(), 1);
    assert_eq!(answered_keys(&app).await, SqlValue::Int(1));
}

#[tokio::test]
async fn a_bounce_another_delivery_suppressed_first_does_not_fail_the_delivery() {
    let app = started().await;
    fail_suppressions(&app, "nextval('sofie.suppression_tries') = 1").await;

    let outbox = refused_by_the_server(&app, "/receipts-to-two", "bob@example.com", 550).await;

    assert_eq!(outbox.mails().len(), 1);
    assert_eq!(deliveries_left(&app).await, Vec::<Vec<SqlValue>>::new());
    assert_eq!(held(&app, "bob@example.com").await, [Scope::All]);
}
