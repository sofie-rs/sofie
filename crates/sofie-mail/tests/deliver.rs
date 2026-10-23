use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use sofie_core::{Error, Result, SqlValue};
use sofie_db::worker::Worker;
use sofie_db::{Db, File, Queued};
use sofie_http::{App, Routes, Web};
use sofie_mail::{Address, Mail, Mailer, Template};
use sofie_testing::{TestApp, sent_mails};
use sofie_values::Email;

const RECEIPT: &str = "<p>Thanks, {data.name}.</p><p>Your total is {data.total}.</p>\
                       <p><a href={data.link}>Your receipt</a></p>";
const BUILT_IN: &str = "<p>The crate's own words for {data.name}.</p>";
const REPLACEMENT: &str = "<p>The app's words for {data.name}.</p>";

static WRITTEN: AtomicUsize = AtomicUsize::new(0);

/// Written to disk too, as a development build rereads a mail from its source.
fn compiled(key: &'static str, source: &str) -> Template {
    let bytes = sofie_engine::compile_template(key, source).expect("the template compiles");
    let number = WRITTEN.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "sofie-mail-deliver-{}-{number}.html",
        std::process::id()
    ));
    std::fs::write(&path, source).expect("the template is written");
    let path: &'static str = String::leak(path.to_string_lossy().into_owned());
    Template::new(key, File::new(path, Vec::leak(bytes)))
}

fn registered() -> &'static [Template] {
    static REGISTERED: OnceLock<Vec<Template>> = OnceLock::new();
    REGISTERED.get_or_init(|| {
        vec![
            compiled("shop/receipt", RECEIPT),
            compiled("library/greeting", REPLACEMENT),
        ]
    })
}

fn built_in() -> &'static Template {
    static BUILT_IN_TEMPLATE: OnceLock<Template> = OnceLock::new();
    BUILT_IN_TEMPLATE.get_or_init(|| compiled("library/greeting", BUILT_IN))
}

fn ada() -> Address {
    Address::new(&Email::parse("ada@example.com").unwrap()).unwrap()
}

async fn place_order(mailer: Mailer) -> Result<Queued> {
    let data = json!({
        "name": "Ada <3",
        "total": "€12.00",
        "link": mailer.link("/orders/7")?,
    });
    let body = mailer.body("shop/receipt", &data)?;
    mailer.send(Mail::new(ada(), "Your receipt", body)?)
}

async fn refuse_order(mailer: Mailer) -> Result<Queued> {
    let body = mailer.body("shop/receipt", &json!({ "name": "Ada" }))?;
    mailer.send(Mail::new(ada(), "Your receipt", body)?)?;
    Err(Error::Rule("out of stock".to_owned()))
}

async fn greet(mailer: Mailer) -> Result<Queued> {
    let body = mailer.body_from(built_in(), &json!({ "name": "Ada" }))?;
    mailer.send(Mail::new(ada(), "Hello", body)?)
}

fn routes(routes: &mut Routes<Web>) {
    routes.post("/orders", place_order);
    routes.post("/refused", refuse_order);
    routes.post("/greetings", greet);
}

async fn started() -> TestApp {
    sofie_testing::start(
        |db: Db| async move {
            db.schema().migrate(&[]).await?;
            sofie_mail::templates(&db, registered());
            Ok(App::new(db).area::<Web>("shop", routes))
        },
        Worker::new,
    )
    .await
}

#[tokio::test]
async fn a_mail_waits_for_the_worker_and_is_sent_as_rendered() {
    let app = started().await;

    let answer = app.post("/orders").await;

    assert_eq!(answer.status, 204, "{}", answer.body);
    assert!(sent_mails(&app).is_empty(), "the request did not send it");
    assert_eq!(app.drain().await, 1);
    let mails = sent_mails(&app);
    assert_eq!(mails.len(), 1);
    let mail = &mails[0];
    assert_eq!(mail.subject(), "Your receipt");
    assert_eq!(mail.to(), [ada()]);
    assert_eq!(
        mail.from().map(|from| from.email().as_str()),
        Some(sofie_mail::testing::FROM)
    );
    assert_eq!(
        mail.body().html(),
        "<p>Thanks, Ada &lt;3.</p><p>Your total is €12.00.</p>\
         <p><a href=\"http://localhost/orders/7\">Your receipt</a></p>"
    );
    assert_eq!(
        mail.body().text(),
        "Thanks, Ada <3.\n\nYour total is €12.00.\n\n\
         Your receipt (http://localhost/orders/7)"
    );
}

#[tokio::test]
async fn a_refused_handler_queues_no_mail() {
    let app = started().await;

    let answer = app.post("/refused").await;

    assert_eq!(answer.status, 422, "{}", answer.body);
    assert_eq!(app.drain().await, 0);
    assert!(sent_mails(&app).is_empty());
}

#[tokio::test]
async fn a_job_that_already_sent_its_mail_sends_nothing_when_retried() {
    let app = started().await;
    assert_eq!(app.post("/orders").await.status, 204);
    let queued = app
        .query(
            "SELECT id FROM sofie.messages WHERE name = 'sofie_mail.deliver'",
            &[],
        )
        .await;
    let SqlValue::Uuid(message) = queued[0][0] else {
        panic!("a message id is a uuid");
    };
    // What a try leaves behind when it sent the mail and then failed to finish.
    app.query(
        "INSERT INTO sofie.idempotency (key, fingerprint, created_at, answered_at) \
         VALUES ($1, 'sofie_mail.deliver', now(), now())",
        &[SqlValue::Text(message.to_string())],
    )
    .await;

    assert_eq!(app.drain().await, 1);

    assert!(sent_mails(&app).is_empty());
    let left = app.query("SELECT count(*) FROM sofie.messages", &[]).await;
    assert_eq!(left[0][0], SqlValue::Int(0));
}

#[tokio::test]
async fn an_app_template_replaces_a_crates_own_of_the_same_key() {
    let app = started().await;

    assert_eq!(app.post("/greetings").await.status, 204);
    app.drain().await;

    let mails = sent_mails(&app);
    assert_eq!(mails[0].body().text(), "The app's words for Ada.");
}

#[tokio::test]
async fn each_send_is_a_span_naming_its_transport_and_outcome_but_no_address() {
    let app = started().await;
    assert_eq!(app.post("/orders").await.status, 204);
    let telemetry = sofie_telemetry::capture();

    app.drain().await;

    let spans = telemetry.spans();
    let sent = spans
        .iter()
        .find(|span| span.name == "mail")
        .expect("a span per send");
    assert_eq!(sent.fields["mail.transport"], "fake");
    assert_eq!(sent.fields["mail.recipients"], "1");
    assert_eq!(sent.fields["outcome"], "ok");
    let recorded = format!("{spans:?} {:?}", telemetry.events());
    assert!(!recorded.contains("ada@"), "{recorded}");
}
