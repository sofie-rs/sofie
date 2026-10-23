#![cfg(feature = "cors")]

use std::time::Duration;

use http::StatusCode;
use sofie_core::Result;
use sofie_db::Db;
use sofie_http::{App, Area, Cors, CorsOrigins, Method, Routes, Visitor, Web};
use sofie_testing::{FakeDriver, TestRequest, TestResponse, TestServer};
use tokio::net::TcpListener;

const APP: &str = "https://app.example";
const OTHER: &str = "https://evil.example";
const SESSION: &str = "__Host-sofie-session=x";

struct Api;

impl Area for Api {
    const NAME: &'static str = "api";
    const PREFIX: &'static str = "/api";
    const SIGN_IN: &'static str = "/";
    type Principal = Visitor;
}

async fn hello() -> Result<()> {
    Ok(())
}

fn web(routes: &mut Routes<Web>) {
    routes.get("/hello", hello);
    routes.post("/note", hello);
}

fn api(routes: &mut Routes<Api>) {
    routes.get("/notes", hello);
    routes.post("/notes", hello);
    routes.put("/notes", hello);
}

fn app(cors: Cors) -> App {
    App::new(Db::new(FakeDriver::default()))
        .area("notes", web)
        .area("notes", api)
        .cors::<Api>(cors)
}

fn with_credentials() -> Cors {
    Cors::new()
        .origin(APP)
        .methods(&[Method::GET, Method::POST])
        .headers(&["content-type", "x-trace"])
        .exposed_headers(&["x-request-id"])
        .credentials()
        .max_age(Duration::from_mins(10))
}

fn preflight(server: &TestServer, path: &str, origin: &str, method: &str) -> TestRequest {
    server
        .request(Method::OPTIONS, path)
        .header("origin", origin)
        .header("access-control-request-method", method)
}

fn cross_site_post(server: &TestServer, path: &str, origin: &str) -> TestRequest {
    server
        .post(path)
        .header("origin", origin)
        .header("sec-fetch-site", "cross-site")
}

fn header<'r>(response: &'r TestResponse, name: &str) -> Option<&'r str> {
    response
        .headers
        .get(name)
        .map(|value| value.to_str().unwrap())
}

fn varies_by_origin(response: &TestResponse) -> bool {
    response
        .headers
        .get_all("vary")
        .iter()
        .any(|value| value == "Origin")
}

#[tokio::test]
async fn an_allowed_preflight_is_answered_before_routing() {
    let server = TestServer::start(app(with_credentials())).await;

    let allowed = preflight(&server, "/api/notes", APP, "POST")
        .header("access-control-request-headers", "Content-Type, X-Trace")
        .await;

    assert_eq!(allowed.status, StatusCode::NO_CONTENT);
    assert_eq!(header(&allowed, "access-control-allow-origin"), Some(APP));
    assert_eq!(
        header(&allowed, "access-control-allow-credentials"),
        Some("true")
    );
    assert_eq!(
        header(&allowed, "access-control-allow-methods"),
        Some("GET, POST")
    );
    assert_eq!(
        header(&allowed, "access-control-allow-headers"),
        Some("content-type, x-trace")
    );
    assert_eq!(header(&allowed, "access-control-max-age"), Some("600"));
    assert!(varies_by_origin(&allowed));
    assert_eq!(header(&allowed, "x-frame-options"), Some("DENY"));
}

#[tokio::test]
async fn a_preflight_outside_the_policy_is_refused_without_cors_headers() {
    let server = TestServer::start(app(with_credentials())).await;

    let refused = [
        preflight(&server, "/api/notes", OTHER, "POST").await,
        preflight(&server, "/api/notes", "null", "POST").await,
        preflight(&server, "/api/notes", APP, "PUT").await,
        preflight(&server, "/api/notes", APP, "POST")
            .header("access-control-request-headers", "authorization")
            .await,
    ];
    let other_area = preflight(&server, "/note", APP, "POST").await;

    for response in &refused {
        assert_eq!(response.status, StatusCode::FORBIDDEN);
        assert_eq!(header(response, "access-control-allow-origin"), None);
    }
    assert_eq!(other_area.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(header(&other_area, "access-control-allow-origin"), None);
}

#[tokio::test]
async fn an_answer_carries_cors_headers_only_for_an_allowed_origin() {
    let server = TestServer::start(app(with_credentials())).await;

    let allowed = server.get("/api/notes").header("origin", APP).await;
    let other = server.get("/api/notes").header("origin", OTHER).await;
    let other_area = server.get("/hello").header("origin", APP).await;

    assert_eq!(header(&allowed, "access-control-allow-origin"), Some(APP));
    assert_eq!(
        header(&allowed, "access-control-allow-credentials"),
        Some("true")
    );
    assert_eq!(
        header(&allowed, "access-control-expose-headers"),
        Some("x-request-id")
    );
    assert_eq!(header(&other, "access-control-allow-origin"), None);
    assert!(varies_by_origin(&allowed) && varies_by_origin(&other));
    assert_eq!(header(&other_area, "access-control-allow-origin"), None);
    assert!(!varies_by_origin(&other_area));
}

#[tokio::test]
async fn a_cross_origin_write_passes_csrf_only_from_an_origin_cors_allows() {
    let server = TestServer::start(app(with_credentials())).await;

    let allowed = cross_site_post(&server, "/api/notes", APP)
        .header("cookie", SESSION)
        .await;
    let other_origin = cross_site_post(&server, "/api/notes", OTHER)
        .header("cookie", SESSION)
        .await;
    let without_origin = server
        .post("/api/notes")
        .header("sec-fetch-site", "cross-site")
        .await;
    let other_area = cross_site_post(&server, "/note", APP).await;

    assert_eq!(allowed.status, StatusCode::NO_CONTENT);
    assert_eq!(header(&allowed, "access-control-allow-origin"), Some(APP));
    assert_eq!(other_origin.status, StatusCode::FORBIDDEN);
    assert_eq!(without_origin.status, StatusCode::FORBIDDEN);
    assert_eq!(other_area.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn without_credentials_a_cross_origin_write_carrying_the_session_is_refused() {
    let server = TestServer::start(app(Cors::new().origin(APP))).await;

    let without_session = cross_site_post(&server, "/api/notes", APP).await;
    let with_session = cross_site_post(&server, "/api/notes", APP)
        .header("cookie", SESSION)
        .await;

    assert_eq!(without_session.status, StatusCode::NO_CONTENT);
    assert_eq!(
        header(&without_session, "access-control-allow-credentials"),
        None
    );
    assert_eq!(with_session.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn any_origin_is_answered_with_a_star_and_never_with_the_session() {
    let server = TestServer::start(app(Cors::new().origin("*"))).await;

    let read = server.get("/api/notes").header("origin", OTHER).await;
    let write = cross_site_post(&server, "/api/notes", OTHER).await;
    let with_session = cross_site_post(&server, "/api/notes", OTHER)
        .header("cookie", SESSION)
        .await;

    assert_eq!(header(&read, "access-control-allow-origin"), Some("*"));
    assert!(!varies_by_origin(&read));
    assert_eq!(write.status, StatusCode::NO_CONTENT);
    assert_eq!(with_session.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn origins_from_settings_add_to_the_areas_policy() {
    let origins = CorsOrigins::parse("api=https://admin.example").unwrap();
    let server =
        TestServer::start(app(with_credentials()).serving(|serving| serving.cors_origins(origins)))
            .await;

    let from_code = server.get("/api/notes").header("origin", APP).await;
    let from_settings = server
        .get("/api/notes")
        .header("origin", "https://admin.example")
        .await;

    assert_eq!(header(&from_code, "access-control-allow-origin"), Some(APP));
    assert_eq!(
        header(&from_settings, "access-control-allow-origin"),
        Some("https://admin.example")
    );
}

#[tokio::test]
async fn settings_naming_an_area_without_a_policy_refuse_to_start() {
    let origins = CorsOrigins::parse("web=https://app.example").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();

    let served = app(with_credentials())
        .serving(|serving| serving.cors_origins(origins))
        .serve(listener, std::future::ready(()))
        .await;

    let error = served.unwrap_err().to_string();
    assert!(error.contains("SOFIE_CORS_ORIGINS"), "{error}");
}
