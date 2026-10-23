use sofie::core::{Error, Result};
use sofie::db::{Db, File, Input, Outcome, ReadOnly, Reply, Resolve, Writes};
use sofie::engine::Tone;
use sofie::http::{App, Routes, Web};
use sofie::serde_json::{Value, json};
use sofie_testing::{FakeDriver, TestResponse, TestServer};

const ORDERS: &str = r#"<Section title="Orders">
  <p>{t("orders.intro", count = data.count)}</p>
  <Badge value={data.state} enum="order_status" />
  <Form id="new-order" post="/orders" target="orders">
    <Field name="status" required />
    <Submit label="Add" />
  </Form>
  <Table id="orders" of={data.orders} columns="status" />
</Section>
"#;

const ENGLISH: &str = "[orders]\nintro = \"{count} orders\"\n";
const TURKISH: &str = "[orders]\nintro = \"{count} sipariş\"\n\n[label]\nstatus = \"Durum\"\n\n\
                       [enum.order_status]\npaid = \"Ödendi\"\n";

struct Listing;

impl Outcome for Listing {
    fn reply(self) -> Result<Reply> {
        Ok(Reply::Json(
            json!({ "count": 2, "state": "PAID", "orders": [{ "status": "open" }] }),
        ))
    }
}

impl ReadOnly for Listing {}

struct NewOrder;

impl Input for NewOrder {
    fn from_json(value: Value) -> Result<Self> {
        match value.get("status").and_then(Value::as_str) {
            Some(status) if !status.is_empty() => Ok(Self),
            _ => Err(Error::invalid("status", "validation.required")),
        }
    }
}

impl<C: Writes> Resolve<C> for NewOrder {
    fn resolve(context: &mut C) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(context.input().and_then(Self::from_json))
    }
}

async fn list_orders() -> Result<Listing> {
    Ok(Listing)
}

async fn place_order(_order: NewOrder) -> Result<()> {
    Ok(())
}

fn app() -> App {
    let mut screens = sofie::http::screens_from_sources(&[("shop/web/orders", ORDERS)]).unwrap();
    let translations = sofie::http::translations(
        &["en", "tr"],
        &[
            ("shop", File::new("lang/en.toml", ENGLISH.as_bytes())),
            ("shop", File::new("lang/tr.toml", TURKISH.as_bytes())),
        ],
        &[],
        &[(
            "order_status",
            &[("OPEN", Tone::Neutral), ("PAID", Tone::Success)],
        )],
    )
    .unwrap();
    screens.translate(translations);
    App::new(Db::new(FakeDriver::default()))
        .screens(screens)
        .area("shop", |routes: &mut Routes<Web>| {
            routes.screen("/orders", "orders", list_orders);
            routes
                .post("/orders", place_order)
                .then_screen("orders", "orders");
        })
}

async fn post_empty_status(server: &TestServer, headers: &[(&str, &str)]) -> TestResponse {
    let mut request = server
        .post("/orders")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("accept-language", "tr-TR, en;q=0.5")
        .body("status=");
    for (name, value) in headers {
        request = request.header(name, value);
    }
    request.await
}

#[tokio::test]
async fn a_page_is_answered_in_the_language_the_browser_asks_for() {
    let server = TestServer::start(app()).await;

    let turkish = server
        .get("/orders")
        .header("accept-language", "tr-TR, en;q=0.5")
        .await;
    let english = server.get("/orders").await;

    assert!(
        turkish
            .body
            .starts_with("<!doctype html><html lang=\"tr\">")
    );
    assert!(
        turkish.body.contains("<p>2 sipariş</p>"),
        "{}",
        turkish.body
    );
    assert!(turkish.body.contains(">Durum</label>"), "{}", turkish.body);
    assert!(
        turkish
            .body
            .contains("<span class=\"sofie-badge sofie-tone-success\">Ödendi</span>"),
        "{}",
        turkish.body
    );
    assert!(english.body.contains(">Paid</span>"), "{}", english.body);
    assert!(
        turkish.headers["vary"]
            .to_str()
            .unwrap()
            .contains("Accept-Language")
    );
    assert!(
        english
            .body
            .starts_with("<!doctype html><html lang=\"en\">")
    );
    assert!(english.body.contains("<p>2 orders</p>"), "{}", english.body);
}

#[tokio::test]
async fn a_refused_form_is_explained_in_the_viewer_s_language() {
    let server = TestServer::start(app()).await;
    let from_the_form = [
        ("hx-request", "true"),
        ("hx-target", "orders"),
        ("hx-trigger", "new-order"),
    ];

    let htmx = post_empty_status(&server, &from_the_form).await;
    let without_javascript = post_empty_status(&server, &[]).await;

    assert!(
        htmx.body
            .starts_with("<p>Lütfen işaretli alanı düzeltin.</p>"),
        "{}",
        htmx.body
    );
    assert!(
        htmx.body
            .ends_with("#new-order-status-error\">zorunludur</div>"),
        "{}",
        htmx.body
    );
    let page = &without_javascript.body;
    assert!(page.contains("<p>Durum: zorunludur</p>"), "{page}");
    assert!(page.contains(">zorunludur</p>"), "{page}");
}
