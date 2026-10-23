//! A binary of its own: a span's interest is cached per callsite.

use sofie_core::{Result, TestClock, Timestamp};
use sofie_db::Db;
use sofie_http::{App, Module, RouteName, Routes, Web};
use sofie_testing::{FakeDriver, TestServer};

async fn hello() -> Result<()> {
    Ok(())
}

fn app() -> App {
    let clock = TestClock::starting_at(Timestamp::from_second(1_700_000_000).unwrap());
    App::new(Db::new(FakeDriver::default()).with_clock(clock.clock())).area(
        "files",
        |routes: &mut Routes<Web>| {
            routes.get("/hello", hello);
        },
    )
}

#[cfg(feature = "trusted-proxies")]
async fn edge_ids_recorded(
    proxies: sofie_http::TrustedProxies,
    requests: &[(&str, &str)],
) -> Vec<String> {
    let telemetry = sofie_telemetry::capture();
    let server = TestServer::start(app().serving(|serving| serving.trusted_proxies(proxies))).await;
    for (header, value) in requests {
        let answer = server.get("/hello").header(header, value).await;
        assert_ne!(answer.header("x-request-id"), *value);
    }
    telemetry
        .spans()
        .into_iter()
        .filter(|span| span.name == "request")
        .filter_map(|span| span.fields.get("edge_request_id").cloned())
        .collect()
}

#[cfg(feature = "trusted-proxies")]
#[tokio::test]
async fn a_trusted_proxys_well_formed_request_id_is_recorded_beside_the_servers_own() {
    let trusted = || sofie_http::TrustedProxies::parse("127.0.0.1").unwrap();
    let too_long = "a".repeat(129);

    let default_header = edge_ids_recorded(
        trusted(),
        &[
            ("x-request-id", "edge-1"),
            ("x-request-id", "edge 2"),
            ("x-request-id", &too_long),
            ("x-request-id", "edge\"x"),
        ],
    )
    .await;
    let configured = edge_ids_recorded(
        trusted().request_id_header("Fly-Request-Id").unwrap(),
        &[("x-request-id", "edge-3"), ("fly-request-id", "01J9:ams")],
    )
    .await;
    let untrusted = edge_ids_recorded(
        sofie_http::TrustedProxies::default(),
        &[("x-request-id", "edge-4")],
    )
    .await;

    assert_eq!(default_header, ["edge-1"]);
    assert_eq!(configured, ["01J9:ams"]);
    assert!(untrusted.is_empty(), "{untrusted:?}");
}

#[cfg(feature = "trusted-proxies")]
#[tokio::test]
async fn only_a_trusted_proxy_s_trace_is_continued() {
    let incoming = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
    let trace_of = |proxies| async move {
        let server =
            TestServer::start(app().serving(|serving| serving.trusted_proxies(proxies))).await;
        let answer = server.get("/hello").header("traceparent", incoming).await;
        answer.header("traceparent").to_owned()
    };

    let trusted = trace_of(sofie_http::TrustedProxies::parse("127.0.0.1").unwrap()).await;
    let untrusted = trace_of(sofie_http::TrustedProxies::default()).await;

    assert!(
        trusted.starts_with("00-4bf92f3577b34da6a3ce929d0e0e4736-"),
        "{trusted}"
    );
    assert!(
        !untrusted.contains("4bf92f3577b34da6a3ce929d0e0e4736"),
        "{untrusted}"
    );
}

#[tokio::test]
async fn a_request_span_is_named_by_the_route_name_from_the_wiring() {
    const NAMES: &[RouteName] = &[RouteName {
        method: "GET",
        path: "/hello",
        name: "files.web.hello",
    }];
    let telemetry = sofie_telemetry::capture();
    let named = TestServer::start(app().modules(&[Module {
        route_names: NAMES,
        ..Module::new("files")
    }]))
    .await;

    named.get("/hello").await;

    let span_names: Vec<String> = telemetry
        .spans()
        .into_iter()
        .filter(|span| span.name == "request")
        .filter_map(|span| span.fields.get("otel.name").cloned())
        .collect();
    assert!(
        span_names.contains(&"GET files.web.hello".to_owned()),
        "{span_names:?}"
    );
}

#[tokio::test]
async fn the_request_and_its_handler_carry_the_module() {
    let telemetry = sofie_telemetry::capture();
    let server = TestServer::start(app()).await;

    server.get("/hello").await;

    let spans = telemetry.spans();
    let module_of = |name: &str| {
        spans
            .iter()
            .find(|span| span.name == name)
            .and_then(|span| span.fields.get("module").cloned())
    };
    assert_eq!(module_of("request").as_deref(), Some("files"));
    assert_eq!(module_of("handler").as_deref(), Some("telemetry"));
}

#[tokio::test]
async fn a_screen_renders_in_a_span_inside_the_request() {
    let telemetry = sofie_telemetry::capture();
    let screens = [("files/web/hello", "<Section title=\"Hello\"></Section>")];
    let app = App::new(Db::new(FakeDriver::default()))
        .screens(sofie_http::screens_from_sources(&screens).unwrap())
        .area("files", |routes: &mut Routes<Web>| {
            routes.screen("/hello", "hello", hello);
        });
    let server = TestServer::start(app).await;

    server.get("/hello").await;

    let spans = telemetry.spans();
    let render = spans.iter().find(|span| span.name == "render").unwrap();
    let parent = spans.iter().find(|span| Some(span.id) == render.parent);
    assert_eq!(parent.map(|span| span.name), Some("request"));
    assert_eq!(render.fields["screen"], "files/web/hello");
    assert_eq!(render.fields["otel.name"], "files/web/hello");
}

#[cfg(feature = "browser-telemetry")]
mod browser {
    use serde_json::json;
    use sofie_http::{RouteName, Routes, Web};

    use super::*;

    const NAMES: &[RouteName] = &[RouteName {
        method: "GET",
        path: "/orders",
        name: "files.web.orders",
    }];

    fn page_app() -> App {
        let screens = [("files/web/orders", "<Section title=\"Orders\"></Section>")];
        App::new(Db::new(FakeDriver::default()))
            .screens(sofie_http::screens_from_sources(&screens).unwrap())
            .modules(&[Module {
                route_names: NAMES,
                ..Module::new("files")
            }])
            .area("files", |routes: &mut Routes<Web>| {
                routes.screen("/orders", "orders", hello);
            })
    }

    #[tokio::test]
    async fn a_page_tells_the_client_where_to_send_and_in_which_trace() {
        let _telemetry = sofie_telemetry::capture();
        let server = TestServer::start(page_app()).await;

        let page = server.get("/orders").await;

        let traceparent = page.header("traceparent");
        let meta = format!(
            "<meta name=\"sofie-telemetry\" content=\"/_sofie/telemetry\" \
             data-route=\"files.web.orders\" data-traceparent=\"{traceparent}\" data-sample=\"1\">"
        );
        assert!(
            page.body.contains(&format!("{meta}</head>")),
            "{}",
            page.body
        );
    }

    #[tokio::test]
    async fn only_listed_kinds_and_names_are_recorded_for_a_known_route() {
        let telemetry = sofie_telemetry::capture();
        let server = TestServer::start(page_app()).await;
        let posted = json!({
            "route": "files.web.orders",
            "records": [
                { "kind": "vital", "name": "LCP", "value": 1234.5 },
                { "kind": "error", "name": "Secret: ada@example.com" },
                { "kind": "keystroke", "name": "a" },
            ],
        });

        let answer = server.post("/_sofie/telemetry").json(&posted).await;

        assert_eq!(answer.status, http::StatusCode::NO_CONTENT);
        let recorded: Vec<(String, String, String)> = telemetry
            .events()
            .into_iter()
            .filter(|event| event.fields.contains_key("browser.kind"))
            .map(|event| {
                let field = |name: &str| event.fields[name].clone();
                (
                    field("browser.kind"),
                    field("browser.name"),
                    field("http.route"),
                )
            })
            .collect();
        let expected = [("vital", "LCP"), ("error", "other")].map(|(kind, name)| {
            (
                kind.to_owned(),
                name.to_owned(),
                "files.web.orders".to_owned(),
            )
        });
        assert_eq!(recorded, expected);
    }

    #[tokio::test]
    async fn a_client_sending_past_its_rate_is_refused() {
        let server = TestServer::start(page_app()).await;
        let posted = json!({ "route": "files.web.orders", "records": [] });

        for _ in 0..60 {
            let answer = server.post("/_sofie/telemetry").json(&posted).await;
            assert_eq!(answer.status, http::StatusCode::NO_CONTENT);
        }
        let refused = server.post("/_sofie/telemetry").json(&posted).await;

        assert_eq!(refused.status, http::StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test(start_paused = true)]
    async fn a_body_that_never_arrives_is_cut_at_the_request_timeout() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let server = TestServer::start(page_app()).await;
        let address = server.origin().trim_start_matches("http://").to_owned();
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();

        stream
            .write_all(
                b"POST /_sofie/telemetry HTTP/1.1\r\nHost: shop.test\r\n\
                  Content-Length: 8192\r\n\r\n{",
            )
            .await
            .unwrap();
        let mut answer = Vec::new();
        stream.read_to_end(&mut answer).await.unwrap();

        let answer = String::from_utf8_lossy(&answer);
        assert!(answer.starts_with("HTTP/1.1 503"), "{answer}");
    }
}

#[cfg(all(feature = "toolbar", debug_assertions))]
#[tokio::test]
async fn a_debug_page_shows_its_span_tree_under_it() {
    let _telemetry = sofie_telemetry::capture();
    let screens = [("files/web/hello", "<Section title=\"Hello\"></Section>")];
    let app = App::new(Db::new(FakeDriver::default()))
        .screens(sofie_http::screens_from_sources(&screens).unwrap())
        .area("files", |routes: &mut sofie_http::Routes<Web>| {
            routes.screen("/hello", "hello", hello);
        });
    let server = TestServer::start(app).await;

    let page = server.get("/hello").await;

    let toolbar = page
        .body
        .split("<details id=\"sofie-toolbar\"")
        .nth(1)
        .unwrap_or_default();
    assert!(toolbar.contains("request GET /hello "), "{}", page.body);
    assert!(
        toolbar.contains("\n  handler telemetry::hello "),
        "{}",
        page.body
    );
    assert!(
        toolbar.contains("\n  render files/web/hello "),
        "{}",
        page.body
    );
    assert!(
        page.body.ends_with("</form></details></body></html>"),
        "{}",
        page.body
    );
}

#[tokio::test]
async fn a_session_cookie_that_does_not_open_is_logged_with_its_reason() {
    let telemetry = sofie_telemetry::capture();
    let server = TestServer::start(app()).await;
    let other_keys = sofie_auth::SigningKeys::new(&[[9; 32]]).unwrap();
    let claims = sofie_auth::SessionClaims::new(
        sofie_core::Uuid::new_v4().unwrap(),
        "files",
        sofie_core::Uuid::new_v4().unwrap(),
        Timestamp::from_second(1_700_000_000).unwrap(),
    );
    let foreign = claims.seal(&other_keys).unwrap();

    for cookie in ["not base64!".to_owned(), foreign] {
        server
            .get("/hello")
            .header("cookie", &format!("__Host-sofie-session={cookie}"))
            .await;
    }

    let refused: Vec<(tracing::Level, String)> = telemetry
        .events()
        .into_iter()
        .filter_map(|event| Some((event.level, event.fields.get("session.refused")?.clone())))
        .collect();
    assert_eq!(
        refused,
        [
            (tracing::Level::DEBUG, "malformed".to_owned()),
            (tracing::Level::INFO, "unauthenticated".to_owned()),
        ]
    );
}

#[tokio::test]
async fn a_probe_opens_no_request_span_and_logs_no_line() {
    let telemetry = sofie_telemetry::capture();
    let server = TestServer::start(app()).await;

    let live = server.get("/live").await;
    server.get("/ready").await;
    server.get("/hello").await;

    let routes: Vec<String> = telemetry
        .spans()
        .into_iter()
        .filter(|span| span.name == "request")
        .filter_map(|span| span.fields.get("http.route").cloned())
        .collect();
    let finished = telemetry
        .events()
        .into_iter()
        .filter(|event| event.fields.get("message").map(String::as_str) == Some("request finished"))
        .count();
    assert_eq!(live.status, http::StatusCode::OK);
    assert_eq!(routes, ["/hello"]);
    assert_eq!(finished, 1);
}
