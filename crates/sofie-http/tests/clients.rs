#![cfg(feature = "clients")]

use serde_json::{Value, json};
use sofie_core::{Error, Instant, Result};
use sofie_db::{Db, Input, Outcome, ReadOnly, Reply, Resolve, Showing, Writes};
use sofie_engine::ClientBundle;
use sofie_http::{App, Routes, Web};
use sofie_testing::{FakeDriver, TestResponse, TestServer};

const SCREEN: &str = "application/vnd.sofie.screen+json";

const ORDERS: &str = r#"<Section title="Orders">
  <Table id="orders" of={data} columns="status" />
  <Form id="note" post="/orders/note"><Field name="text" required /><Submit label="Save" /></Form>
</Section>
"#;

const PLAIN: &str = r#"<Section title="Help"><p>Ask us.</p></Section>"#;

struct Listing(Value);

impl Outcome for Listing {
    fn reply(self) -> Result<Reply> {
        Ok(Reply::Json(self.0))
    }
}

impl ReadOnly for Listing {}

async fn list_orders() -> Result<Listing> {
    Ok(Listing(json!([{ "status": "open" }])))
}

async fn help() -> Result<Listing> {
    Ok(Listing(json!({})))
}

struct Moments(Vec<Instant>);

impl Outcome for Moments {
    fn reply(self) -> Result<Reply> {
        Ok(Reply::Data(Box::new(Showing(self.0))))
    }
}

impl ReadOnly for Moments {}

async fn moments() -> Result<Moments> {
    Ok(Moments(vec![Instant::parse("2026-10-11T22:30:00Z")?]))
}

struct Note;

impl Input for Note {
    fn from_json(value: Value) -> Result<Self> {
        match value.get("text").and_then(Value::as_str) {
            Some(text) if !text.is_empty() => Ok(Self),
            _ => Err(Error::invalid("text", "is required")),
        }
    }
}

impl<C: Writes> Resolve<C> for Note {
    fn resolve(context: &mut C) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(context.input().and_then(Self::from_json))
    }
}

async fn add_note(_note: Note) -> Result<()> {
    Ok(())
}

async fn connect() -> Result<Listing> {
    Ok(Listing(
        json!({ "location": "https://provider.test/authorize?state=1" }),
    ))
}

async fn server() -> TestServer {
    let mut screens = sofie_http::screens_from_sources(&[
        ("shop/web/orders", ORDERS),
        ("shop/web/help", PLAIN),
        ("shop/web/moments", PLAIN),
    ])
    .unwrap();
    screens.clients(vec![ClientBundle::new(
        "web",
        &["shop/web/orders", "shop/web/moments"],
        b"export {};",
        b"",
    )]);
    let app = App::new(Db::new(FakeDriver::default()))
        .screens(screens)
        .area("shop", |routes: &mut Routes<Web>| {
            routes.screen("/orders", "orders", list_orders);
            routes.screen("/help", "help", help);
            routes.screen("/moments", "moments", moments);
            routes
                .post("/orders/note", add_note)
                .then_screen("orders", "orders");
            routes.post("/connect", connect).then_follow();
        });
    TestServer::start(app).await
}

fn envelope_in(page: &TestResponse) -> Value {
    let start = page.body.find("id=\"sofie-screen\">").unwrap() + "id=\"sofie-screen\">".len();
    let end = start + page.body[start..].find("</script>").unwrap();
    serde_json::from_str(&page.body[start..end]).unwrap()
}

#[tokio::test]
async fn a_page_of_a_client_screen_is_the_shell_with_its_first_envelope() {
    let server = server().await;

    let page = server.get("/orders").header("accept", "text/html").await;

    page.assert_ok();
    assert!(
        page.body.contains("<div id=\"sofie-app\"></div>"),
        "{}",
        page.body
    );
    assert!(page.body.contains(".client.js\" nonce=\""), "{}", page.body);
    let envelope = envelope_in(&page);
    assert_eq!(envelope["screen"], "shop/web/orders");
    assert_eq!(envelope["data"], json!([{ "status": "open" }]));
    assert_eq!(
        envelope["view"]["forms"]["note"]["parts"][0]["field"]["name"],
        "text"
    );
    let script = page
        .body
        .split("<script type=\"module\" src=\"")
        .nth(1)
        .unwrap();
    let path = &script[..script.find('"').unwrap()];
    let bundle = server.get(path).await;
    bundle.assert_ok();
    assert_eq!(bundle.body, "export {};");
}

#[tokio::test]
async fn the_runtime_gets_the_envelope_and_a_screen_outside_the_bundle_stays_html() {
    let server = server().await;

    let envelope = server.get("/orders").header("accept", SCREEN).await;
    let help = server.get("/help").header("accept", "text/html").await;

    envelope.assert_ok();
    assert_eq!(envelope.json()["title"], "Orders");
    assert_eq!(envelope.json()["navigation"], json!([]));
    assert!(help.body.contains("<p>Ask us.</p>"), "{}", help.body);
    assert!(!help.body.contains("sofie-app"), "{}", help.body);
}

#[tokio::test]
async fn the_envelope_marks_where_typed_data_holds_instants() {
    let server = server().await;

    let typed = server.get("/moments").header("accept", SCREEN).await;
    let json = server.get("/orders").header("accept", SCREEN).await;

    assert_eq!(typed.json()["data"], json!(["2026-10-11T22:30:00Z"]));
    assert_eq!(typed.json()["instants"], json!(true));
    assert_eq!(json.json()["instants"], Value::Null);
}

#[tokio::test]
async fn a_post_answers_the_refreshed_screen_or_the_field_errors() {
    let server = server().await;

    let saved = server
        .post("/orders/note")
        .form(&[("text", "hello")])
        .header("accept", SCREEN)
        .header("sec-fetch-site", "same-origin")
        .await;
    let refused = server
        .post("/orders/note")
        .form(&[("text", "")])
        .header("accept", SCREEN)
        .header("sec-fetch-site", "same-origin")
        .await;

    saved.assert_ok();
    assert_eq!(saved.json()["screen"], "shop/web/orders");
    refused.assert_status(422);
    assert_eq!(
        refused.json()["errors"],
        json!([{
            "field": "text",
            "key": "validation.invalid",
            "arguments": {},
            "message": "is required",
        }])
    );
}

#[tokio::test]
async fn an_external_follow_is_named_to_the_runtime_which_cannot_fetch_across_sites() {
    let server = server().await;

    let followed = server
        .post("/connect")
        .header("accept", SCREEN)
        .header("sec-fetch-site", "same-origin")
        .await;

    followed.assert_ok();
    assert_eq!(
        followed.json(),
        json!({ "location": "https://provider.test/authorize?state=1" })
    );
}
