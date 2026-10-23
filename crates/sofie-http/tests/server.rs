use std::time::Duration;

use http::{HeaderMap, Method, StatusCode};
use serde_json::{Value, json};
use sofie_core::{Cast, ColumnReader, Error, Result, SqlValue, Uuid};
use sofie_db::{
    Aggregate, Context, Db, Field, Input, InputFields, Meta, Outcome, ReadOnly, Reply, Resolve,
    Step, Writes,
};
use sofie_http::{App, AppName, Area, Routes, Screens, SigningKeys, Visitor, Web};
use sofie_testing::{FakeDriver, TestResponse, TestServer};
use sofie_values::{FormField, FormFields, Present, Secret};
use tokio::net::TcpListener;

struct Order {
    meta: Meta,
    status: String,
}

impl Aggregate for Order {
    const NAME: &'static str = "order";
    const MODULE: &'static str = "shop";
    const FIELDS: &'static [Field] = &[Field::new("status", String::COLUMNS, false)];

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.status.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            status: String::decode(row)?,
        })
    }
}

impl<C: Context> Resolve<C> for Order {
    fn resolve(context: &mut C) -> impl Future<Output = Result<Self>> + Send {
        sofie_db::bind::<Self, C>(context)
    }
}

struct NewOrder {
    status: String,
}

impl Input for NewOrder {
    fn from_json(value: Value) -> Result<Self> {
        match value.get("status").and_then(Value::as_str) {
            Some(status) if !status.is_empty() => Ok(Self {
                status: status.to_owned(),
            }),
            _ => Err(Error::invalid("status", "is required")),
        }
    }
}

impl<C: Writes> Resolve<C> for NewOrder {
    fn resolve(context: &mut C) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(context.input().and_then(Self::from_json))
    }
}

struct Console;

impl Area for Console {
    const NAME: &'static str = "console";
    const PREFIX: &'static str = "/console";
    const SIGN_IN: &'static str = "/";
    type Principal = Visitor;
}

/// A GET replies the order as an action would, but never commits it.
struct Shown(Order);

impl Outcome for Shown {
    fn reply(self) -> Result<Reply> {
        self.0.reply()
    }
}

impl ReadOnly for Shown {}

async fn show_order(order: Order) -> Result<Shown> {
    Ok(Shown(order))
}

async fn cancel_order(mut order: Order) -> Result<Order> {
    order.status = String::from("cancelled");
    Ok(order)
}

async fn create_order(input: NewOrder) -> Result<Order> {
    Ok(Order {
        meta: Meta::new().unwrap(),
        status: input.status,
    })
}

async fn greet(_visitor: Visitor) -> Result<()> {
    Ok(())
}

async fn refuse() -> Result<()> {
    Err(Error::Rule("orders are <closed>".to_owned()))
}

async fn stale() -> Result<()> {
    Err(Error::Conflict)
}

async fn crash() -> Result<()> {
    Err(Error::internal(
        "charging card 4242",
        std::io::Error::other("secret token abc"),
    ))
}

async fn panic() -> Result<()> {
    panic!("a bug in the handler")
}

async fn slow() -> Result<()> {
    tokio::time::sleep(Duration::from_millis(300)).await;
    Ok(())
}

struct Listing(Value);

impl Outcome for Listing {
    fn reply(self) -> Result<Reply> {
        Ok(Reply::Json(self.0))
    }
}

impl ReadOnly for Listing {}

async fn list_orders() -> Result<Listing> {
    Ok(Listing(
        json!([{ "status": "open" }, { "status": "<script>" }]),
    ))
}

/// Only an app with translations varies by language; these have none.
const SCREEN_VARIES_BY: &str = "Accept, HX-Request, HX-Target";

const ORDERS_SCREEN: &str = r#"<Section title="Orders">
  <Table id="orders" of={data} columns="status" />
</Section>
"#;

fn empty_app() -> App {
    App::new(Db::new(FakeDriver::default()))
}

fn orders_screens(source: &str) -> Screens {
    sofie_http::screens_from_sources(&[("shop/web/orders", source)]).unwrap()
}

fn screen_app(screens: Screens) -> App {
    empty_app()
        .screens(screens)
        .area("shop", |routes: &mut Routes<Web>| {
            routes.screen("/orders", "orders", list_orders);
        })
}

async fn server() -> TestServer {
    TestServer::start(app(&FakeDriver::default())).await
}

async fn orders_server() -> TestServer {
    TestServer::start(screen_app(orders_screens(ORDERS_SCREEN))).await
}

fn refreshing_app(driver: &FakeDriver) -> App {
    App::new(Db::new(driver.clone()))
        .screens(orders_screens(ORDERS_SCREEN))
        .area("shop", |routes: &mut Routes<Web>| {
            routes.screen("/orders", "orders", list_orders);
            routes
                .post("/orders/{order}/cancel", cancel_order)
                .then_screen("orders", "orders");
        })
}

fn order_routes(routes: &mut Routes<Web>) {
    routes.get("/orders/{order}", show_order);
    routes.post("/orders/{order}/cancel", cancel_order);
    routes.post("/orders", create_order);
}

fn misc_routes(routes: &mut Routes<Web>) {
    routes.get("/hello", greet);
    routes.post("/refuse", refuse);
    routes.post("/stale", stale);
    routes.post("/crash", crash);
    routes.post("/panic", panic);
    routes.get("/slow", slow);
}

fn app(driver: &FakeDriver) -> App {
    App::new(Db::new(driver.clone()))
        .area("shop", order_routes)
        .area("shop", misc_routes)
        .area("shop", |routes: &mut Routes<Console>| {
            routes.get("/orders/{order}", show_order);
        })
}

fn open_order() -> (Uuid, FakeDriver) {
    let id = Uuid::now_v7().unwrap();
    let row = vec![
        SqlValue::Uuid(id),
        SqlValue::Int(1),
        SqlValue::Text("open".to_owned()),
    ];
    (id, FakeDriver::returning(vec![row]))
}

async fn assert_fails_to_serve(app: App, expected: &str) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let error = app
        .serving(|serving| serving.signing_keys(SigningKeys::generate().unwrap()))
        .serve(listener, std::future::pending())
        .await
        .unwrap_err()
        .to_string();
    assert_contains(&error, expected);
}

async fn connect(server: &TestServer) -> tokio::net::TcpStream {
    let address = server.origin().trim_start_matches("http://");
    tokio::net::TcpStream::connect(address).await.unwrap()
}

async fn send(
    server: &TestServer,
    method: Method,
    path: &str,
    body: impl Into<bytes::Bytes>,
) -> TestResponse {
    let answer = server
        .request(method, path)
        .header("content-type", "application/json")
        .body(body)
        .await;
    assert_secure(&answer.headers);
    answer
}

async fn get(server: &TestServer, path: &str, headers: &[(&str, &str)]) -> TestResponse {
    with_headers(server.get(path), headers).await
}

async fn post_from(server: &TestServer, path: &str, headers: &[(&str, &str)]) -> TestResponse {
    with_headers(server.post(path), headers).await
}

fn with_headers(
    mut request: sofie_testing::TestRequest,
    headers: &[(&str, &str)],
) -> sofie_testing::TestRequest {
    for (name, value) in headers {
        request = request.header(name, value);
    }
    request
}

fn assert_contains(text: &str, part: &str) {
    assert!(text.contains(part), "{part:?} is not in {text}");
}

fn assert_secure(headers: &HeaderMap) {
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert_eq!(headers["referrer-policy"], "no-referrer");
    assert_eq!(headers["x-frame-options"], "DENY");
    assert_eq!(
        headers["strict-transport-security"],
        "max-age=63072000; includeSubDomains"
    );
    let kept = if headers.contains_key("etag") {
        "private, no-cache"
    } else {
        "no-store"
    };
    assert_eq!(headers["cache-control"], kept);
    let permissions = headers["permissions-policy"].to_str().unwrap();
    assert_contains(permissions, "camera=(), ");
    assert!(!headers.contains_key("server"));
    assert!(headers.contains_key("content-security-policy"));
}

#[tokio::test]
async fn an_answer_without_a_policy_of_its_own_gets_one_that_loads_nothing() {
    let server = server().await;

    let refused = send(&server, Method::POST, "/refuse", "").await;
    let missing = get(&server, "/nowhere", &[]).await;

    for answer in [refused, missing] {
        assert_eq!(
            answer.headers["content-security-policy"],
            "default-src 'none'; frame-ancestors 'none'"
        );
    }
}

#[tokio::test]
async fn a_path_parameter_is_percent_decoded_before_it_binds() {
    let (id, driver) = open_order();
    let server = TestServer::start(app(&driver)).await;
    let text = id.to_string();
    let escaped = format!("%{:02X}{}", text.as_bytes()[0], &text[1..]);

    let answer = send(&server, Method::GET, &format!("/orders/{escaped}"), "").await;
    let broken = send(&server, Method::GET, "/orders/%FF", "").await;

    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.json()["id"], text);
    assert_eq!(broken.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_query_binds_the_path_and_answers_json_without_writing() {
    let (id, driver) = open_order();
    let server = TestServer::start(app(&driver)).await;

    let answer = send(&server, Method::GET, &format!("/orders/{id}"), "").await;

    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.headers["content-type"], "application/json");
    assert_eq!(answer.json(), json!({ "id": id.to_string(), "version": 1 }));
    let queries = driver.queries();
    assert_eq!(queries.len(), 1);
    assert!(queries[0].1.contains(&SqlValue::Uuid(id)));
    assert_eq!(driver.transaction_count(), 0);
}

#[tokio::test]
async fn an_action_writes_its_change_in_a_transaction() {
    let (id, driver) = open_order();
    let server = TestServer::start(app(&driver)).await;

    let answer = send(&server, Method::POST, &format!("/orders/{id}/cancel"), "").await;

    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.json()["version"], 2);
    let transactions = driver.take_transactions();
    let [steps] = transactions.as_slice() else {
        panic!("expected one transaction, saw {}", transactions.len());
    };
    let [Step::Statement { sql, params, .. }] = steps.as_slice() else {
        panic!("expected one statement, saw {steps:?}");
    };
    assert!(sql.starts_with("UPDATE"));
    assert!(params.contains(&SqlValue::Text("cancelled".to_owned())));
}

#[tokio::test]
async fn json_input_creates_an_aggregate() {
    let driver = FakeDriver::default();
    let server = TestServer::start(app(&driver)).await;

    let answer = send(&server, Method::POST, "/orders", r#"{"status":"open"}"#).await;

    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.json()["version"], 1);
    let transactions = driver.take_transactions();
    assert_eq!(transactions.len(), 1);
    assert!(
        matches!(&transactions[0][0], Step::Statement { sql, .. } if sql.starts_with("INSERT"))
    );
}

#[tokio::test]
async fn input_errors_answer_400_or_422() {
    let server = server().await;

    let malformed = send(&server, Method::POST, "/orders", "{").await;
    let invalid = send(&server, Method::POST, "/orders", "{}").await;
    let empty = send(&server, Method::POST, "/orders", "").await;

    assert_eq!(malformed.status, StatusCode::BAD_REQUEST);
    assert_eq!(invalid.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        invalid.json(),
        json!({ "errors": [{
            "field": "status",
            "key": "validation.invalid",
            "arguments": {},
            "message": "is required",
        }] })
    );
    assert_eq!(
        (empty.status, empty.json()),
        (invalid.status, invalid.json())
    );
}

#[tokio::test]
async fn a_refused_rule_answers_422_with_its_message_and_an_internal_error_500_leaking_nothing() {
    let server = server().await;

    let refused = send(&server, Method::POST, "/refuse", "").await;
    let crashed = send(&server, Method::POST, "/crash", "").await;

    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(refused.json(), json!({ "error": "orders are <closed>" }));
    assert_eq!(crashed.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(crashed.body, r#"{"error":"Something went wrong."}"#);
}

/// A test sees why its request failed, which a client never does.
#[tokio::test]
async fn a_test_reads_the_internal_error_behind_a_500() {
    let server = server().await;

    let crashed = send(&server, Method::POST, "/crash", "").await;
    let refused = send(&server, Method::POST, "/refuse", "").await;

    assert!(
        crashed
            .internal_error()
            .is_some_and(|error| error.contains("charging card")),
        "{crashed:?}"
    );
    assert_eq!(refused.internal_error(), None);
}

#[tokio::test]
async fn unbound_or_unknown_records_answer_404() {
    let server = server().await;

    let malformed = send(&server, Method::GET, "/orders/not-a-uuid", "").await;
    let missing = send(
        &server,
        Method::GET,
        &format!("/orders/{}", Uuid::now_v7().unwrap()),
        "",
    )
    .await;
    let unknown_path = send(&server, Method::GET, "/nowhere", "").await;

    assert_eq!(malformed.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(unknown_path.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn another_method_on_a_known_path_answers_405_with_allow_naming_head_beside_get() {
    let server = server().await;

    let answer = send(&server, Method::DELETE, "/orders", "").await;
    let readable = send(&server, Method::DELETE, "/hello", "").await;

    assert_eq!(answer.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(answer.headers["allow"], "POST");
    assert_eq!(readable.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(readable.headers["allow"], "GET, HEAD");
}

#[tokio::test]
async fn a_body_over_the_limit_answers_413() {
    let server = server().await;

    let answer = send(
        &server,
        Method::POST,
        "/orders",
        vec![b'x'; 1024 * 1024 + 1],
    )
    .await;

    assert_eq!(answer.status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn headers_past_32_kb_answer_431() {
    let server = server().await;
    let padding = |bytes| "x".repeat(bytes);

    let fits = get(&server, "/hello", &[("x-padding", &padding(16 * 1024))]).await;
    let too_large = get(&server, "/hello", &[("x-padding", &padding(40 * 1024))]).await;

    assert_ne!(fits.status, StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE);
    assert_eq!(
        too_large.status,
        StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE
    );
}

#[tokio::test]
async fn head_answers_as_get_would_without_the_body() {
    let (id, driver) = open_order();
    let server = TestServer::start(app(&driver)).await;

    let order = server.request(Method::HEAD, &format!("/orders/{id}")).await;
    let live = server.request(Method::HEAD, "/live").await;

    for answer in [&order, &live] {
        assert_secure(&answer.headers);
        assert_eq!(answer.status, StatusCode::OK);
        assert_eq!(answer.body, "");
    }
    assert_eq!(order.headers["content-type"], "application/json");
    let got = server.get(&format!("/orders/{id}")).await;
    assert_eq!(order.headers["content-length"], got.body.len().to_string());
}

#[tokio::test]
async fn the_visitor_resolves_in_the_web_area_and_an_area_prefix_is_prepended_to_its_routes() {
    let (id, driver) = open_order();
    let server = TestServer::start(app(&driver)).await;

    let hello = send(&server, Method::GET, "/hello", "").await;
    let console = send(&server, Method::GET, &format!("/console/orders/{id}"), "").await;

    assert_eq!(hello.status, StatusCode::NO_CONTENT);
    assert_eq!(hello.body, "");
    assert_eq!(console.status, StatusCode::OK);
}

#[tokio::test]
async fn a_duplicate_route_fails_to_serve_naming_it() {
    let hello = |routes: &mut Routes<Web>| {
        routes.get("/hello", greet);
    };

    assert_fails_to_serve(
        empty_app().area("shop", hello).area("shop", hello),
        "GET /hello",
    )
    .await;
}

#[tokio::test]
async fn shutdown_finishes_open_requests_then_stops() {
    let server = server().await;
    let request = server.get("/slow").into_future();
    let request = tokio::spawn(request);
    tokio::time::sleep(Duration::from_millis(100)).await;

    let stopped = server.stop().await;

    assert_eq!(request.await.unwrap().status, StatusCode::NO_CONTENT);
    assert!(stopped.is_ok(), "{stopped:?}");
}

#[tokio::test]
async fn a_screen_without_its_template_fails_to_serve_naming_it() {
    let app = screen_app(sofie_http::screens_from_sources(&[]).unwrap());

    assert_fails_to_serve(app, "GET /orders renders screen shop/web/orders").await;
}

#[tokio::test]
async fn a_screen_answers_json_when_asked_and_a_browser_the_page_under_a_nonce_policy() {
    let server = orders_server().await;

    let json = get(&server, "/orders", &[("accept", "application/json")]).await;
    let first = get(&server, "/orders", &[("accept", "text/html")]).await;
    let second = get(&server, "/orders", &[]).await;

    assert_secure(&json.headers);
    assert_eq!(json.status, StatusCode::OK);
    assert_eq!(json.headers["content-type"], "application/json");
    assert_eq!(json.headers["vary"], SCREEN_VARIES_BY);
    assert_eq!(json.json()[1]["status"], "<script>");
    assert_secure(&first.headers);
    assert_eq!(first.status, StatusCode::OK);
    assert_eq!(first.headers["content-type"], "text/html; charset=utf-8");
    assert_eq!(first.headers["vary"], SCREEN_VARIES_BY);
    let nonce = nonce_of(&first);
    assert_ne!(nonce, nonce_of(&second));
    let page = first.body.clone();
    assert_contains(&page, &format!("nonce=\"{nonce}\""));
    assert_contains(&page, "open");
    assert!(
        page.starts_with("<!doctype html><html lang=\"en\">"),
        "{page}"
    );
    assert_contains(&page, "<title>Orders</title>");
    assert!(
        page.contains("&lt;script&gt;") && !page.contains("<script>"),
        "{page}"
    );
}

#[tokio::test]
async fn an_htmx_request_gets_only_its_target_fragment_and_404_for_an_unknown_one() {
    let server = orders_server().await;

    let answer = server.get("/orders").htmx("orders").await;
    let unknown = server.get("/orders").htmx("nowhere").await;

    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.headers["content-type"], "text/html; charset=utf-8");
    assert!(answer.headers.contains_key("content-security-policy"));
    let fragment = answer.body.clone();
    assert_contains(&fragment, "id=\"orders\"");
    assert!(!fragment.to_lowercase().contains("<html"), "{fragment}");
    assert!(!fragment.contains("Orders</"), "{fragment}");
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    assert_notice(&unknown, "<p>Not found.</p>");
}

fn linked_path(page: &str, prefix: &str) -> String {
    let start = page.find(prefix).expect("the page links the asset");
    page[start..start + page[start..].find('"').unwrap()].to_owned()
}

#[tokio::test]
async fn the_page_assets_are_served_for_good_from_their_hashed_paths() {
    let server = orders_server().await;
    let page = get(&server, "/orders", &[]).await;
    let page = page.body;

    let script = get(&server, &linked_path(&page, "/_sofie/htmx-"), &[]).await;
    let engine_script = get(&server, &linked_path(&page, "/_sofie/engine-"), &[]).await;
    let stylesheet = get(&server, &linked_path(&page, "/_sofie/sofie-"), &[]).await;

    for (asset, content_type) in [
        (&script, "application/javascript; charset=utf-8"),
        (&engine_script, "application/javascript; charset=utf-8"),
        (&stylesheet, "text/css; charset=utf-8"),
    ] {
        assert_eq!(asset.status, StatusCode::OK);
        assert_eq!(
            asset.headers["cache-control"],
            "public, max-age=31536000, immutable"
        );
        assert_eq!(asset.headers["content-type"], content_type);
    }
    assert!(script.body.starts_with("var htmx="));
    let engine_script = engine_script.body;
    assert_contains(&engine_script, "aria-invalid");
    assert_contains(&page, "<link rel=\"stylesheet\" href=\"/_sofie/sofie-");
}

#[tokio::test]
async fn an_htmx_action_answers_with_the_refreshed_fragment_after_committing() {
    let (id, driver) = open_order();
    let server = TestServer::start(refreshing_app(&driver)).await;
    let answer = server
        .post(&format!("/orders/{id}/cancel"))
        .htmx("orders")
        .await;

    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.headers["content-type"], "text/html; charset=utf-8");
    let fragment = answer.body.clone();
    assert!(fragment.starts_with("<table id=\"orders\">"), "{fragment}");
    assert!(!fragment.contains("<section"), "{fragment}");
    assert_eq!(driver.transaction_count(), 1);
}

#[tokio::test]
async fn a_json_caller_of_a_refreshing_action_still_gets_json() {
    let (id, driver) = open_order();
    let server = TestServer::start(refreshing_app(&driver)).await;

    let answer = send(&server, Method::POST, &format!("/orders/{id}/cancel"), "").await;

    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.json()["version"], 2);
}

#[tokio::test]
async fn refreshing_a_screen_no_route_renders_fails_to_serve() {
    let app = empty_app().screens(orders_screens(ORDERS_SCREEN)).area(
        "shop",
        |routes: &mut Routes<Web>| {
            routes
                .post("/orders/{order}/cancel", cancel_order)
                .then_screen("orders", "orders");
        },
    );

    let expected = "POST /orders/{order}/cancel refreshes screen shop/web/orders, but no screen route renders it";
    assert_fails_to_serve(app, expected).await;
}

#[tokio::test]
async fn routes_one_path_apart_but_for_parameter_names_fail_to_serve() {
    let app = empty_app().area("shop", |routes: &mut Routes<Web>| {
        routes.get("/orders/{order}", show_order);
        routes.get("/orders/{id}", show_order);
    });

    let expected = "routes GET /orders/{order} of module `shop` and GET /orders/{id} of module \
                    `shop` are served at one path";
    assert_fails_to_serve(app, expected).await;
}

#[tokio::test]
async fn two_routes_of_one_name_fail_to_serve() {
    let app = empty_app().area("shop", |routes: &mut Routes<Web>| {
        routes
            .post("/orders/{order}/cancel", cancel_order)
            .name("cancel");
        routes
            .post("/orders/{order}/void", cancel_order)
            .name("cancel");
    });

    let expected = "are both named `shop.web.cancel`";
    assert_fails_to_serve(app, expected).await;
}

fn assert_notice(answer: &TestResponse, html: &str) {
    assert_eq!(answer.headers["content-type"], "text/html; charset=utf-8");
    assert_eq!(answer.headers["hx-retarget"], "#sofie-notice");
    assert_eq!(answer.headers["hx-reswap"], "innerHTML");
    assert_eq!(answer.body, html);
}

#[tokio::test]
async fn a_failed_htmx_action_answers_its_status_with_a_notice() {
    let server = server().await;
    let unknown = format!("/orders/{}/cancel", Uuid::now_v7().unwrap());
    let cases = [
        (
            "/refuse",
            "",
            StatusCode::UNPROCESSABLE_ENTITY,
            "<p>orders are &lt;closed&gt;</p>",
        ),
        (
            "/orders",
            "{}",
            StatusCode::UNPROCESSABLE_ENTITY,
            "<p>Status: is required</p>",
        ),
        (
            "/stale",
            "",
            StatusCode::CONFLICT,
            "<p>This record changed since you loaded it; reload and try again.</p>",
        ),
        (&unknown, "", StatusCode::NOT_FOUND, "<p>Not found.</p>"),
        (
            "/crash",
            "",
            StatusCode::INTERNAL_SERVER_ERROR,
            "<p>Something went wrong.</p>",
        ),
    ];

    for (path, body, status, html) in cases {
        let answer = server.post(path).htmx("orders").body(body).await;

        assert_secure(&answer.headers);
        assert_eq!(answer.status, status, "{path}");
        assert_notice(&answer, html);
    }
}

#[tokio::test]
async fn an_htmx_caller_asking_for_json_gets_json_errors() {
    let server = server().await;
    let answer = server.post("/refuse").htmx("orders").accept_json().await;

    assert_eq!(answer.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(!answer.headers.contains_key("hx-retarget"));
    assert_eq!(answer.json(), json!({ "error": "orders are <closed>" }));
}

#[tokio::test]
async fn a_cross_site_write_is_refused_before_its_handler_runs() {
    let (id, driver) = open_order();
    let server = TestServer::start(app(&driver)).await;
    let path = format!("/orders/{id}/cancel");
    let own_origin = server.origin().to_owned();

    let cross_site = post_from(&server, &path, &[("sec-fetch-site", "cross-site")]).await;
    let foreign_origin = post_from(&server, &path, &[("origin", "https://evil.test")]).await;
    let htmx_cross_site = post_from(
        &server,
        &path,
        &[("sec-fetch-site", "cross-site"), ("hx-request", "true")],
    )
    .await;

    for refused in [&cross_site, &foreign_origin] {
        assert_secure(&refused.headers);
        assert_eq!(refused.status, StatusCode::FORBIDDEN);
        assert_eq!(
            refused.json(),
            json!({ "error": "This request came from another site and was refused." })
        );
    }
    assert_eq!(htmx_cross_site.status, StatusCode::FORBIDDEN);
    assert_eq!(htmx_cross_site.headers["hx-retarget"], "#sofie-notice");
    assert_eq!(driver.queries().len(), 0, "no handler ran");
    assert_eq!(driver.transaction_count(), 0, "no handler ran");

    let same_origin = post_from(&server, &path, &[("origin", &own_origin)]).await;
    assert_eq!(same_origin.status, StatusCode::OK);
}

#[tokio::test]
async fn an_htmx_action_from_the_page_passes_the_cross_site_check() {
    let (id, driver) = open_order();
    let server = TestServer::start(refreshing_app(&driver)).await;
    let own_origin = server.origin().to_owned();

    let answer = post_from(
        &server,
        &format!("/orders/{id}/cancel"),
        &[
            ("hx-request", "true"),
            ("hx-target", "orders"),
            ("sec-fetch-site", "same-origin"),
            ("origin", &own_origin),
        ],
    )
    .await;

    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(driver.transaction_count(), 1);
}

fn nonce_of(answer: &TestResponse) -> String {
    let policy = answer.headers["content-security-policy"].to_str().unwrap();
    assert!(
        policy.starts_with("default-src 'none'; script-src 'self' 'nonce-"),
        "{policy}"
    );
    let nonce = policy
        .split("'nonce-")
        .nth(1)
        .and_then(|rest| rest.split('\'').next())
        .unwrap();
    assert_eq!(nonce.len(), 32, "{policy}");
    nonce.to_owned()
}

#[tokio::test]
async fn live_answers_while_ready_says_only_that_it_is_unavailable() {
    let server = TestServer::start(App::new(Db::new(FakeDriver::down()))).await;

    let live = get(&server, "/live", &[]).await;
    let ready = get(&server, "/ready", &[]).await;

    assert_eq!(live.status, StatusCode::OK);
    assert_eq!(live.body, "ok");
    assert_secure(&live.headers);
    assert_eq!(ready.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(ready.body, "unavailable");
}

#[tokio::test]
async fn ready_answers_once_the_database_does() {
    let driver = FakeDriver::default();
    let server = TestServer::start(app(&driver)).await;

    let answer = get(&server, "/ready", &[]).await;

    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(driver.queries()[0].0, "SELECT 1");
}

#[tokio::test]
async fn an_app_route_on_a_health_path_fails_to_serve() {
    let app = empty_app().area("shop", |routes: &mut Routes<Web>| {
        routes.post("/live", greet);
    });

    assert_fails_to_serve(app, "POST /live is reserved").await;
}

const INCOMING_TRACE: &str = "4bf92f3577b34da6a3ce929d0e0e4736";
const OFFERED_REQUEST_ID: &str = "0192f0c4-7d4e-7c1a-9f3e-5b2a1c0d9e8f";

#[tokio::test]
async fn a_client_s_trace_and_request_id_are_never_taken_and_every_response_carries_its_own() {
    let server = server().await;
    let incoming = format!("00-{INCOMING_TRACE}-00f067aa0ba902b7-01");

    let offered = get(&server, "/hello", &[("traceparent", &incoming)]).await;
    let missing = get(&server, "/nowhere", &[("traceparent", "00-junk")]).await;
    let given_id = get(&server, "/hello", &[("x-request-id", OFFERED_REQUEST_ID)]).await;

    let traceparent = offered.headers["traceparent"].to_str().unwrap();
    assert!(traceparent.starts_with("00-"), "{traceparent}");
    assert!(!traceparent.contains(INCOMING_TRACE), "{traceparent}");
    assert!(offered.headers.contains_key("x-request-id"));
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert!(
        !missing.headers["traceparent"]
            .to_str()
            .unwrap()
            .contains(INCOMING_TRACE)
    );
    assert!(missing.headers.contains_key("x-request-id"));
    let request_id = given_id.headers["x-request-id"].to_str().unwrap();
    assert!(request_id.parse::<Uuid>().is_ok(), "{request_id}");
    assert_ne!(request_id, OFFERED_REQUEST_ID);
}

const FORM: &str = "application/x-www-form-urlencoded";

struct Preferences {
    quantity: u32,
    gift: bool,
    note: Option<String>,
}

impl Input for Preferences {
    fn from_json(value: Value) -> Result<Self> {
        let mut fields = InputFields::new(value)?;
        let quantity = fields.take("quantity");
        let gift = fields.take("gift");
        let note = fields.optional("note");
        fields.finish()?;
        Ok(Self {
            quantity: quantity?,
            gift: gift?,
            note: note?,
        })
    }
}

impl<C: Writes> Resolve<C> for Preferences {
    fn resolve(context: &mut C) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(context.input().and_then(Self::from_json))
    }
}

async fn save_preferences(preferences: Preferences) -> Result<Listing> {
    Ok(Listing(json!({
        "quantity": preferences.quantity,
        "gift": preferences.gift,
        "note": preferences.note,
    })))
}

async fn post_form(
    server: &TestServer,
    path: &str,
    body: &str,
    headers: &[(&str, &str)],
) -> TestResponse {
    let request = server
        .post(path)
        .header("content-type", FORM)
        .body(body.to_owned());
    with_headers(request, headers).await
}

#[tokio::test]
async fn a_form_posts_numbers_and_checkboxes_as_strings() {
    let app = empty_app().area("shop", |routes: &mut Routes<Web>| {
        routes.post("/preferences", save_preferences);
    });
    let server = TestServer::start(app).await;
    let post = |body| post_form(&server, "/preferences", body, &[]);

    let checked = post("quantity=3&gift=false&gift=true&note=").await;
    let unchecked = post("quantity=12&gift=false&note=Ring+twice").await;
    let wrong = post("quantity=three&gift=false").await;
    let broken = post("quantity=%zz").await;

    assert_eq!(checked.status, StatusCode::OK);
    assert_eq!(
        checked.json(),
        json!({ "quantity": 3, "gift": true, "note": null })
    );
    assert_eq!(
        unchecked.json(),
        json!({ "quantity": 12, "gift": false, "note": "Ring twice" })
    );
    assert_eq!(wrong.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(wrong.body.contains("<li>quantity: "), "{}", wrong.body);
    assert_eq!(broken.status, StatusCode::BAD_REQUEST);
}

const FORM_SCREEN: &str = r#"<Section title="Orders">
  <Form id="new-order" post="/orders" target="orders">
    <Field name="status" required />
    <Submit label="Add" />
  </Form>
  <Table id="orders" of={data} columns="status" />
</Section>
"#;

fn form_app(driver: &FakeDriver, screens: Screens) -> App {
    App::new(Db::new(driver.clone()))
        .screens(screens)
        .area("shop", |routes: &mut Routes<Web>| {
            routes.screen("/orders", "orders", list_orders);
            routes
                .post("/orders", create_order)
                .then_screen("orders", "orders");
        })
}

const FROM_THE_FORM: [(&str, &str); 3] = [
    ("hx-request", "true"),
    ("hx-target", "orders"),
    ("hx-trigger", "new-order"),
];

#[tokio::test]
async fn an_htmx_form_post_refreshes_the_target_and_renders_the_form_afresh() {
    let driver = FakeDriver::default();
    let server = TestServer::start(form_app(&driver, orders_screens(FORM_SCREEN))).await;

    let answer = post_form(&server, "/orders", "status=open", &FROM_THE_FORM).await;

    assert_eq!(answer.status, StatusCode::OK);
    let html = answer.body.clone();
    assert!(html.starts_with("<table id=\"orders\">"), "{html}");
    assert!(
        html.contains("hx-swap-oob=\"true\"></div>"),
        "the notice is cleared: {html}"
    );
    let form = &html[html.find("<form").unwrap()..];
    assert!(
        form.starts_with("<form hx-swap-oob=\"true\" id=\"new-order\""),
        "{html}"
    );
    assert!(form.contains("<p class=\"sofie-field-error\" id=\"new-order-status-error\""));
    assert_eq!(driver.transaction_count(), 1);
}

#[tokio::test]
async fn an_invalid_htmx_form_post_puts_each_message_under_its_field() {
    let server = TestServer::start(form_app(
        &FakeDriver::default(),
        orders_screens(FORM_SCREEN),
    ))
    .await;

    let answer = post_form(&server, "/orders", "status=", &FROM_THE_FORM).await;

    assert_eq!(answer.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_notice(
        &answer,
        "<p>Please correct the highlighted field.</p>\
<div hx-swap-oob=\"innerHTML:#new-order .sofie-field-error\"></div>\
<div hx-swap-oob=\"innerHTML:#new-order-status-error\">is required</div>",
    );
}

async fn refuse_move() -> Result<()> {
    Err(Error::invalid(
        "order_status",
        "cannot move from paid to open",
    ))
}

#[tokio::test]
async fn an_htmx_field_error_the_form_shows_no_slot_for_is_told_as_text() {
    let app = form_app(&FakeDriver::default(), orders_screens(FORM_SCREEN)).area(
        "shop",
        |routes: &mut Routes<Web>| {
            routes
                .post("/orders/moved", refuse_move)
                .then_screen("orders", "orders");
        },
    );
    let server = TestServer::start(app).await;

    let answer = post_form(&server, "/orders/moved", "status=open", &FROM_THE_FORM).await;

    assert_eq!(answer.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_notice(
        &answer,
        "<p>Order status: cannot move from paid to open</p>\
<div hx-swap-oob=\"innerHTML:#new-order .sofie-field-error\"></div>",
    );
}

#[tokio::test]
async fn a_form_posted_without_javascript_is_redirected_or_shown_again() {
    let driver = FakeDriver::default();
    let server = TestServer::start(form_app(&driver, orders_screens(FORM_SCREEN))).await;

    let placed = post_form(&server, "/orders", "status=open", &[]).await;
    let refused = post_form(&server, "/orders", "status=", &[]).await;

    assert_secure(&placed.headers);
    assert_eq!(placed.status, StatusCode::SEE_OTHER);
    assert_eq!(placed.headers["location"], "/orders");
    assert_eq!(placed.body, "");
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    nonce_of(&refused);
    let page = refused.body.clone();
    assert!(page.starts_with("<!doctype html>"), "{page}");
    assert_contains(
        &page,
        "aria-live=\"polite\"><p>Status: is required</p></div>",
    );
    assert_eq!(driver.transaction_count(), 1);
}

const TYPED_FORM_SCREEN: &str = r#"<Section title="Orders">
  <Form id="new-order" post="/orders" target="orders">
    <Field name="status" required />
    <Field name="note" />
  </Form>
  <Table id="orders" of={data} columns="status" />
</Section>
"#;

struct TypedOrder;

impl FormFields for TypedOrder {
    const FIELDS: &'static [FormField] = &[
        FormField {
            name: "status",
            input: String::INPUT,
            required: true,
            constraints: &[],
        },
        FormField {
            name: "note",
            input: String::INPUT,
            required: false,
            constraints: &[],
        },
        FormField {
            name: "secret",
            input: <Secret<String>>::INPUT,
            required: false,
            constraints: &[],
        },
    ];
}

#[tokio::test]
async fn a_form_shown_again_without_javascript_keeps_what_was_typed() {
    let mut screens = orders_screens(TYPED_FORM_SCREEN);
    screens
        .describe_form::<TypedOrder>("shop/web/orders", "new-order")
        .unwrap();
    let server = TestServer::start(form_app(&FakeDriver::default(), screens)).await;

    let refused = post_form(
        &server,
        "/orders",
        "status=&note=Ring+%3Ctwice%3E&secret=hunter22",
        &[],
    )
    .await;

    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    let page = refused.body.clone();
    assert_contains(
        &page,
        "name=\"note\" aria-describedby=\"new-order-note-error\" value=\"Ring &lt;twice&gt;\">",
    );
    assert!(!page.contains("hunter22"), "{page}");
    assert!(
        page.contains("<input type=\"password\" id=\"new-order-secret\""),
        "a field the screen does not write comes from the input: {page}"
    );
    assert_contains(
        &page,
        "name=\"status\" aria-describedby=\"new-order-status-error\" required aria-invalid=\"true\">",
    );
    assert_contains(
        &page,
        "id=\"new-order-status-error\" aria-live=\"polite\">is required</p>",
    );
    assert_eq!(page.matches("aria-invalid").count(), 1, "{page}");
}

#[tokio::test]
async fn refreshing_a_screen_whose_path_the_action_cannot_fill_fails_to_serve() {
    let app = empty_app().screens(orders_screens(ORDERS_SCREEN)).area(
        "shop",
        |routes: &mut Routes<Web>| {
            routes.screen("/orders/{order}", "orders", list_orders);
            routes
                .post("/orders", create_order)
                .then_screen("orders", "orders");
        },
    );

    let expected =
        "POST /orders refreshes the screen at /orders/{order}, whose `{order}` it does not have";
    assert_fails_to_serve(app, expected).await;
}

async fn priced_order() -> Result<Listing> {
    Ok(Listing(
        json!({ "total": { "minor": 123_450, "currency": "EUR" } }),
    ))
}

/// Not gated on `i18n`: the page must be the same with or without it.
#[tokio::test]
async fn a_page_without_translations_renders_the_same_with_or_without_i18n() {
    let app = empty_app()
        .screens(
            sofie_http::screens_from_sources(&[(
                "shop/web/order",
                r#"<Section title="Order"><Money value={data.total} /></Section>"#,
            )])
            .unwrap(),
        )
        .area("shop", |routes: &mut Routes<Web>| {
            routes.screen("/order", "order", priced_order);
        });
    let server = TestServer::start(app).await;

    let page = get(&server, "/order", &[("accept-language", "de")]).await;

    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert!(
        page.body.starts_with("<!doctype html><html lang=\"en\">"),
        "{}",
        page.body
    );
    assert_contains(&page.body, "<span class=\"sofie-money\">1234.50 EUR</span>");
}

#[cfg(feature = "chosen-screens")]
async fn chosen_screen() -> Result<sofie_http::Screen<Listing>> {
    Ok(sofie_http::Screen::render("orders", list_orders().await?))
}

#[cfg(feature = "chosen-screens")]
#[tokio::test]
async fn a_handler_chooses_its_screen_and_json_callers_get_the_data_alone() {
    let app = empty_app().screens(orders_screens(ORDERS_SCREEN)).area(
        "shop",
        |routes: &mut Routes<Web>| {
            routes.get("/chosen", chosen_screen);
        },
    );
    let server = TestServer::start(app).await;

    let page = get(&server, "/chosen", &[]).await;
    let data = get(&server, "/chosen", &[("accept", "application/json")]).await;

    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert_contains(&page.body, "<title>Orders</title>");
    assert_contains(&page.body, "&lt;script&gt;");
    assert_eq!(
        data.json(),
        json!([{ "status": "open" }, { "status": "<script>" }])
    );
}

#[tokio::test]
async fn connections_past_the_cap_wait_to_be_accepted() {
    let one = std::num::NonZeroUsize::MIN;
    let server = TestServer::start(
        app(&FakeDriver::default()).serving(|serving| serving.max_connections(one)),
    )
    .await;
    let holder = connect(&server).await;
    let waiting = tokio::spawn(server.get("/hello").into_future());
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert!(!waiting.is_finished());
    drop(holder);
    assert_eq!(waiting.await.unwrap().status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn a_panicking_handler_answers_a_logged_500() {
    let telemetry = sofie_telemetry::capture();
    let server = server().await;

    let answer = send(&server, Method::POST, "/panic", "").await;

    assert_eq!(answer.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(answer.body, r#"{"error":"Something went wrong."}"#);
    assert!(telemetry.events().iter().any(|event| {
        event.level == tracing::Level::ERROR
            && event
                .fields
                .get("error")
                .is_some_and(|error| error.contains("a bug in the handler"))
    }));
}

#[tokio::test]
async fn an_unreadable_body_is_answered_without_the_parser_s_words() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let server = server().await;
    let mut stream = connect(&server).await;

    stream
        .write_all(
            b"POST /orders HTTP/1.1\r\nHost: shop.test\r\nContent-Type: application/json\r\n\
              Transfer-Encoding: chunked\r\n\r\nzz\r\n",
        )
        .await
        .unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).await.unwrap();

    assert!(answer.starts_with("HTTP/1.1 400"), "{answer}");
    assert!(
        answer.ends_with(r#"{"error":"The request body could not be read."}"#),
        "{answer}"
    );
}

async fn blank_order() -> Result<Listing> {
    Ok(Listing(json!({})))
}

async fn invalid_order() -> Result<()> {
    Err(Error::invalid("status", "is required"))
}

#[tokio::test]
async fn a_form_refused_without_javascript_comes_back_filled_in_on_its_own_page() {
    let new_order = r#"<Section title="New order">
  <Form id="order" post="/orders/new"><Field name="status" required /><Field name="note" /><Submit label="Place" /></Form>
</Section>
"#;
    let screens = sofie_http::screens_from_sources(&[
        ("shop/web/orders", ORDERS_SCREEN),
        ("shop/web/new", new_order),
    ])
    .unwrap();
    let app = empty_app()
        .screens(screens)
        .area("shop", |routes: &mut Routes<Web>| {
            routes.screen("/orders", "orders", list_orders);
            routes.screen("/orders/new", "new", blank_order);
            routes
                .post("/orders/new", invalid_order)
                .then_screen("orders", "orders");
        });
    let server = TestServer::start(app).await;

    let refused = server
        .post("/orders/new")
        .header("sec-fetch-site", "same-origin")
        .header("accept", "text/html")
        .form(&[("status", ""), ("note", "kept")])
        .await;

    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_contains(&refused.body, "<h2>New order</h2>");
    assert_contains(&refused.body, "value=\"kept\"");
}

#[cfg(feature = "tokens")]
mod tokens {
    use sofie_http::{Area, Routes, Visitor};

    use super::{assert_fails_to_serve, empty_app};

    async fn hello() -> sofie_core::Result<()> {
        Ok(())
    }

    struct Api;

    impl Area for Api {
        const NAME: &'static str = "api";
        const PREFIX: &'static str = "/api";
        const SIGN_IN: &'static str = "/";
        const TOKENS: bool = true;
        type Principal = Visitor;
    }

    #[tokio::test]
    async fn an_area_taking_tokens_whose_scopes_nothing_would_hold_fails_to_serve() {
        let app = empty_app().area("shop", |routes: &mut Routes<Api>| {
            routes.get("/hello", hello);
        });

        assert_fails_to_serve(app, "area `api` takes bearer tokens").await;
    }
}

#[tokio::test]
async fn a_route_under_the_framework_s_own_prefix_fails_to_serve() {
    let app = empty_app().area("shop", |routes: &mut Routes<Web>| {
        routes.get("/_sofie/feed", list_orders);
    });

    assert_fails_to_serve(app, "route GET /_sofie/feed is reserved").await;
}

struct Members;

impl Area for Members {
    const NAME: &'static str = "web";
    const PREFIX: &'static str = "/members";
    const SIGN_IN: &'static str = "/";
    type Principal = Order;
}

#[tokio::test]
async fn two_area_types_of_one_name_fail_to_serve() {
    let app = empty_app()
        .area("shop", |routes: &mut Routes<Web>| {
            routes.get("/orders", list_orders);
        })
        .area("members", |routes: &mut Routes<Members>| {
            routes.get("/members/orders", list_orders);
        });

    assert_fails_to_serve(app, "two area types are named `web`").await;
}

#[tokio::test]
async fn an_action_redirects_to_a_path_filled_from_its_own_path_and_its_reply() {
    let server = TestServer::start(App::new(Db::new(FakeDriver::default())).area(
        "shop",
        |routes: &mut Routes<Web>| {
            routes
                .post("/shops/{shop}/orders", create_order)
                .then_redirect("/shops/{shop}/orders/{id}");
            routes
                .post("/shops/{shop}/drafts", create_order)
                .then_redirect("/shops/{shop}/drafts/{draft}");
        },
    ))
    .await;

    let placed = post_form(&server, "/shops/north%20side/orders", "status=open", &[]).await;
    let unfilled = post_form(&server, "/shops/north/drafts", "status=open", &[]).await;

    assert_eq!(placed.status, StatusCode::SEE_OTHER, "{}", placed.body);
    let location = &placed.headers["location"];
    let id = location
        .to_str()
        .unwrap()
        .strip_prefix("/shops/north%20side/orders/")
        .unwrap();
    assert!(id.parse::<Uuid>().is_ok(), "{location:?}");
    assert_eq!(unfilled.status, StatusCode::INTERNAL_SERVER_ERROR);
}

#[cfg(feature = "idempotency")]
#[tokio::test(start_paused = true)]
async fn the_idempotency_sweeper_stops_with_the_server() {
    let driver = FakeDriver::default();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let app = App::new(Db::new(driver.clone()))
        .serving(|serving| serving.signing_keys(SigningKeys::new(&[[7; 32]]).unwrap()));

    app.serve(listener, std::future::ready(())).await.unwrap();
    tokio::time::sleep(Duration::from_hours(3)).await;

    let swept = driver
        .queries()
        .into_iter()
        .filter(|(sql, _)| sql.contains("idempotency"))
        .count();
    assert_eq!(swept, 0);
}

/// Unread bytes at close make the kernel reset the connection, which can destroy the answer.
#[tokio::test]
async fn an_early_refusal_reads_the_body_it_refuses() {
    let server = server().await;
    for path in ["/nowhere", "/orders/1"] {
        let answer = answer_to_a_large_post(&server, path, "").await;

        assert!(
            answer.starts_with("HTTP/1.1 404") || answer.starts_with("HTTP/1.1 405"),
            "{path}: {answer}"
        );
    }
}

#[cfg(feature = "browser-telemetry")]
#[tokio::test]
async fn a_framework_endpoint_reads_the_body_it_refuses() {
    let server = server().await;

    let answer = answer_to_a_large_post(
        &server,
        "/_sofie/telemetry",
        "Sec-Fetch-Site: cross-site\r\n",
    )
    .await;

    assert!(answer.starts_with("HTTP/1.1 403"), "{answer}");
}

async fn answer_to_a_large_post(server: &TestServer, path: &str, headers: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let body = vec![b'x'; 3 * 1024 * 1024];
    let (mut reading, mut writing) = connect(server).await.into_split();
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: test\r\nContent-Type: application/json\r\n{headers}\
         Content-Length: {}\r\n\r\n",
        body.len()
    );
    // The half is returned, not dropped: dropping it would end the request early.
    let writer = tokio::spawn(async move {
        writing.write_all(head.as_bytes()).await?;
        writing.write_all(&body).await.map(|()| writing)
    });
    let mut answer = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(5), async {
        let mut chunk = [0; 1024];
        while !String::from_utf8_lossy(&answer).contains("\r\n\r\n") {
            match reading.read(&mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(count) => answer.extend_from_slice(&chunk[..count]),
            }
        }
    })
    .await;

    assert!(read.is_ok(), "{path}");
    assert!(
        writer.await.unwrap().is_ok(),
        "{path}: the body was cut off"
    );
    String::from_utf8_lossy(&answer).into_owned()
}

async fn named(name: AppName) -> Result<Listing> {
    Ok(Listing(json!({ "name": name.as_str() })))
}

#[tokio::test]
async fn a_handler_reads_the_name_the_app_serves_under() {
    let named_app = empty_app().serving(|serving| serving.named("shop")).area(
        "shop",
        |routes: &mut Routes<Web>| {
            routes.get("/name", named);
        },
    );
    let unnamed_app = empty_app().area("shop", |routes: &mut Routes<Web>| {
        routes.get("/name", named);
    });
    let named_server = TestServer::start(named_app).await;
    let unnamed_server = TestServer::start(unnamed_app).await;

    let given = named_server.get("/name").accept_json().await;
    let unnamed = unnamed_server.get("/name").accept_json().await;

    assert_eq!(given.json(), json!({ "name": "shop" }));
    assert_eq!(unnamed.json(), json!({ "name": "sofie" }));
}

#[cfg(feature = "listing")]
async fn listed_orders(listing: sofie_http::Listing<Order>, read: sofie_db::Read) -> Result<()> {
    listing.page(&read).await.map(drop)
}

#[cfg(feature = "listing")]
#[tokio::test]
async fn a_json_route_refuses_a_filter_with_422_whatever_its_caller_accepts() {
    let server = TestServer::start(empty_app().area("shop", |routes: &mut Routes<Web>| {
        routes.get("/orders", listed_orders);
    }))
    .await;

    for accept in [
        None,
        Some("*/*"),
        Some("text/html"),
        Some("application/json"),
    ] {
        let headers: Vec<(&str, &str)> = accept
            .map(|accept| ("accept", accept))
            .into_iter()
            .collect();
        let answer = get(&server, "/orders?id=7", &headers).await;

        assert_eq!(
            answer.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{accept:?}"
        );
    }
}

struct Around {
    near: String,
    radius: Option<u32>,
}

impl Input for Around {
    fn from_json(value: Value) -> Result<Self> {
        let mut fields = InputFields::new(value)?;
        let near = fields.take("near");
        let radius = fields.optional("radius");
        fields.finish()?;
        Ok(Self {
            near: near?,
            radius: radius?,
        })
    }
}

impl sofie_db::FieldNames for Around {
    const FIELD_NAMES: &'static [&'static str] = &["near", "radius"];
}

async fn shops_around(query: sofie_http::Query<Around>) -> Result<Listing> {
    let sofie_http::Query(around) = query;
    Ok(Listing(
        json!({ "near": around.near, "radius": around.radius }),
    ))
}

#[tokio::test]
async fn a_get_reads_its_query_typed_refusing_a_bad_value_with_422_and_an_unknown_key_with_400() {
    let server = TestServer::start(empty_app().area("shop", |routes: &mut Routes<Web>| {
        routes.get("/shops", shops_around);
    }))
    .await;

    let read = server.get("/shops?near=Ulm&radius=5").accept_json().await;
    let refused = server.get("/shops?near=Ulm&radius=far").accept_json().await;
    let unknown = server.get("/shops?near=Ulm&size=2").accept_json().await;

    assert_eq!(read.json(), json!({ "near": "Ulm", "radius": 5 }));
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(refused.json()["errors"][0]["field"], "radius");
    assert_eq!(unknown.status, StatusCode::BAD_REQUEST);
}
