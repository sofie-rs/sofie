#![cfg(feature = "parse")]

use serde_json::{Value, json};
use sofie_core::Result;
use sofie_db::{Db, Outcome, ReadOnly, Reply};
use sofie_http::{App, Area, ContentSecurity, Routes, Visitor, Web};
use sofie_testing::{FakeDriver, TestServer};
use tokio::net::TcpListener;

struct Embeds;

impl Area for Embeds {
    const NAME: &'static str = "embeds";
    const PREFIX: &'static str = "/embeds";
    const SIGN_IN: &'static str = "/";
    type Principal = Visitor;
}

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

const ORDERS_SCREEN: &str = r#"<Section title="Orders">
  <Table id="orders" of={data} columns="status" />
</Section>
"#;

fn app() -> App {
    let screens = [
        ("shop/web/orders", ORDERS_SCREEN),
        ("shop/embeds/orders", ORDERS_SCREEN),
    ];
    App::new(Db::new(FakeDriver::default()))
        .screens(sofie_http::screens_from_sources(&screens).unwrap())
        .area("shop", |routes: &mut Routes<Web>| {
            routes.screen("/orders", "orders", list_orders);
        })
        .area("shop", |routes: &mut Routes<Embeds>| {
            routes.screen("/orders", "orders", list_orders);
        })
}

fn provider() -> ContentSecurity {
    ContentSecurity::new()
        .script("https://js.provider.example")
        .frame("https://js.provider.example")
        .connect("https://api.provider.example")
        .form_action("https://pay.provider.example")
}

async fn policy(server: &TestServer, path: &str) -> String {
    let answer = server.get(path).header("accept", "text/html").await;
    let policy = answer.headers["content-security-policy"].to_str().unwrap();
    policy.to_owned()
}

#[tokio::test]
async fn an_areas_sources_reach_only_its_own_pages() {
    let server =
        TestServer::start(app().serving(|serving| serving.content_security::<Embeds>(provider())))
            .await;

    let embeds = policy(&server, "/embeds/orders").await;
    let web = policy(&server, "/orders").await;

    assert!(
        embeds.contains("' https://js.provider.example; style-src 'self';"),
        "{embeds}"
    );
    assert!(
        embeds.contains("connect-src 'self' https://api.provider.example;"),
        "{embeds}"
    );
    assert!(
        embeds.contains("frame-src https://js.provider.example;"),
        "{embeds}"
    );
    assert!(
        embeds.contains("form-action 'self' https://pay.provider.example;"),
        "{embeds}"
    );
    assert!(embeds.ends_with("frame-ancestors 'none'; base-uri 'none'"));
    assert!(!web.contains("provider"), "{web}");
    assert!(!web.contains("frame-src"), "{web}");
}

#[tokio::test]
async fn sources_for_every_area_reach_each_beside_its_own() {
    let everywhere = ContentSecurity::new().image("https://images.example");
    let server = TestServer::start(app().serving(|serving| {
        serving
            .content_security_all(everywhere)
            .content_security::<Embeds>(provider())
    }))
    .await;

    let embeds = policy(&server, "/embeds/orders").await;
    let web = policy(&server, "/orders").await;

    assert!(
        web.contains("img-src 'self' data: https://images.example;"),
        "{web}"
    );
    assert!(
        embeds.contains("img-src 'self' data: https://images.example;"),
        "{embeds}"
    );
    assert!(embeds.contains("https://js.provider.example"), "{embeds}");
}

#[tokio::test]
async fn a_source_that_would_undo_the_policy_refuses_to_start() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let unsafe_inline = ContentSecurity::new().script("'unsafe-inline'");

    let served = app()
        .serving(|serving| serving.content_security::<Embeds>(unsafe_inline))
        .serve(listener, std::future::ready(()))
        .await;

    let error = served.unwrap_err().to_string();
    assert!(error.contains("area `embeds`"), "{error}");
}
