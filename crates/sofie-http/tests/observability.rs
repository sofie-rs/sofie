use http::StatusCode;
use sofie_core::{Error, Result};
use sofie_db::Db;
use sofie_http::{App, Routes, Web};
use sofie_testing::{FakeDriver, TestServer};

async fn hello() -> Result<()> {
    Ok(())
}

fn app(driver: &FakeDriver) -> App {
    App::new(Db::new(driver.clone())).area("files", |routes: &mut Routes<Web>| {
        routes.get("/hello", hello);
    })
}

#[tokio::test]
async fn ready_checks_the_queue_and_each_declared_dependency_without_naming_it() {
    let driver = FakeDriver::default();
    let up = TestServer::start(
        app(&driver).serving(|serving| serving.dependency("search", || async { Ok(()) })),
    )
    .await;
    let down = TestServer::start(app(&driver).serving(|serving| {
        serving.dependency("payments", || async {
            Err(Error::Unavailable(
                "connection refused by 10.0.0.7".to_owned(),
            ))
        })
    }))
    .await;

    let ready = up.get("/ready").await;
    let unready = down.get("/ready").await;

    assert_eq!(ready.status, StatusCode::OK);
    assert_eq!(ready.body, "ok");
    let queries: Vec<String> = driver.queries().into_iter().map(|(sql, _)| sql).collect();
    assert!(
        queries.contains(&"SELECT 1 FROM \"sofie\".\"messages\" LIMIT 1".to_owned()),
        "{queries:?}"
    );
    assert_eq!(unready.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(unready.body, "unavailable");
}

#[tokio::test]
async fn ready_checks_at_most_once_a_second_however_often_it_is_asked() {
    let driver = FakeDriver::default();
    let server = TestServer::start(app(&driver)).await;

    for _ in 0..5 {
        assert_eq!(server.get("/ready").await.status, StatusCode::OK);
    }

    let pings = driver
        .queries()
        .into_iter()
        .filter(|(sql, _)| sql == "SELECT 1")
        .count();
    assert_eq!(pings, 1);
}

#[tokio::test]
async fn the_framework_paths_are_reserved_for_it() {
    for path in ["/metrics", "/_sofie/telemetry"] {
        let app = App::new(Db::new(FakeDriver::default()))
            .serving(|serving| serving.signing_keys(sofie_http::SigningKeys::generate().unwrap()))
            .area("files", |routes: &mut Routes<Web>| {
                routes.get(path, hello);
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();

        let error = app.serve(listener, async {}).await.unwrap_err();

        assert!(error.to_string().contains("is reserved"), "{error}");
    }
}

#[cfg(feature = "metrics")]
mod metrics {
    use sofie_http::{MetricsToken, Module, RouteName};

    use super::*;

    const TOKEN: &str = "a-metrics-token-of-at-least-32-characters";
    const NAMES: &[RouteName] = &[RouteName {
        method: "GET",
        path: "/hello",
        name: "files.web.hello",
    }];

    #[tokio::test]
    async fn requests_are_scraped_by_route_name_with_the_token_only() {
        sofie_telemetry::metrics::start().unwrap();
        let driver = FakeDriver::default();
        let token = MetricsToken::parse(TOKEN).unwrap();
        let server = TestServer::start(
            app(&driver)
                .modules(&[Module {
                    route_names: NAMES,
                    ..Module::new("files")
                }])
                .serving(|serving| serving.metrics_token(token)),
        )
        .await;
        let untokened = TestServer::start(app(&driver)).await;

        server.get("/hello").await;
        let refused = server
            .get("/metrics")
            .header(
                "authorization",
                "Bearer a-wrong-token-of-at-least-32-characters",
            )
            .await;
        let scraped = server
            .get("/metrics")
            .header("authorization", &format!("Bearer {TOKEN}"))
            .await;
        let hidden = untokened.get("/metrics").await;

        assert_eq!(refused.status, StatusCode::UNAUTHORIZED);
        assert_eq!(hidden.status, StatusCode::NOT_FOUND);
        assert_eq!(scraped.status, StatusCode::OK);
        let series = "http_server_request_duration_seconds_count{http_route=\"files.web.hello\",\
                      http_request_method=\"GET\",http_response_status_code=\"204\",\
                      area=\"web\"}";
        assert!(scraped.body.contains(series), "{}", scraped.body);
        assert!(
            scraped.body.contains(
                "sofie_handler_duration_seconds_count{code_function_name=\"observability::hello\""
            ),
            "{}",
            scraped.body
        );
    }

    #[test]
    fn a_short_token_is_refused_and_a_token_never_printed() {
        assert!(MetricsToken::parse("short").is_err());

        let token = MetricsToken::parse(TOKEN).unwrap();

        assert!(!format!("{token:?}").contains(TOKEN));
    }
}
