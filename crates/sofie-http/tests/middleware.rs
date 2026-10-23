#![cfg(feature = "steps")]

use std::sync::{Arc, Mutex};

use http::{Method, StatusCode};
use sofie_core::{Error, Result};
use sofie_db::{BoxFuture, Db};
use sofie_http::{App, Area, Cors, PipelineStep, RequestHead, ResponseHead, Routes, Visitor, Web};
use sofie_testing::{FakeDriver, TestServer};

struct Admin;

impl Area for Admin {
    const NAME: &'static str = "admin";
    const PREFIX: &'static str = "/admin";
    const SIGN_IN: &'static str = "/";
    type Principal = Visitor;
}

type Seen = Arc<Mutex<Vec<String>>>;

/// Implements `answered` only, so `check` keeps its default.
struct Records {
    name: &'static str,
    seen: Seen,
}

impl PipelineStep for Records {
    fn answered(&self, request: &RequestHead<'_>, answer: &mut ResponseHead<'_>) {
        self.seen.lock().unwrap().push(format!(
            "{} {} {} {}",
            self.name,
            request.method(),
            request.path(),
            answer.status()
        ));
        assert!(answer.elapsed() < std::time::Duration::from_secs(5));
        answer.set_header("x-answered-by", self.name).unwrap();
    }
}

struct Counts {
    checked: Seen,
}

impl PipelineStep for Counts {
    fn check<'a>(&'a self, request: &'a RequestHead<'a>) -> BoxFuture<'a, Result<()>> {
        self.checked.lock().unwrap().push(request.path().to_owned());
        Box::pin(async move {
            match request.header("x-blocked") {
                Some(_) => Err(Error::NotFound),
                None => Ok(()),
            }
        })
    }
}

struct Sees {
    seen: Seen,
}

impl PipelineStep for Sees {
    fn check<'a>(&'a self, request: &'a RequestHead<'a>) -> BoxFuture<'a, Result<()>> {
        self.seen
            .lock()
            .unwrap()
            .push(format!("check {}", request.path()));
        Box::pin(std::future::ready(Ok(())))
    }

    fn answered(&self, request: &RequestHead<'_>, _answer: &mut ResponseHead<'_>) {
        self.seen
            .lock()
            .unwrap()
            .push(format!("answered {}", request.path()));
    }
}

struct Overrides {
    refused: Seen,
}

impl PipelineStep for Overrides {
    fn answered(&self, _request: &RequestHead<'_>, answer: &mut ResponseHead<'_>) {
        for (name, value) in [
            ("x-frame-options", "ALLOWALL"),
            ("set-cookie", "__Host-sofie-session=forged; Path=/; Secure"),
            ("Strict-Transport-Security", "max-age=0"),
            ("permissions-policy", "camera=*"),
            ("access-control-allow-origin", "*"),
        ] {
            if let Err(error) = answer.set_header(name, value) {
                self.refused.lock().unwrap().push(error.to_string());
            }
        }
        answer
            .set_header("cache-control", "private, max-age=60")
            .unwrap();
    }
}

async fn hello() -> Result<()> {
    Ok(())
}

fn web(routes: &mut Routes<Web>) {
    routes.get("/hello", hello);
    routes.post("/note", hello);
}

fn admin(routes: &mut Routes<Admin>) {
    routes.get("/report", hello);
}

fn app() -> App {
    App::new(Db::new(FakeDriver::default()))
        .area("notes", web)
        .area("notes", admin)
}

fn seen() -> Seen {
    Arc::new(Mutex::new(Vec::new()))
}

#[tokio::test]
async fn area_steps_answer_before_app_wide_ones_which_see_every_routed_answer() {
    let seen = seen();
    let record = |name| Records {
        name,
        seen: Arc::clone(&seen),
    };
    let server = TestServer::start(
        app()
            .step_all(record("outer"))
            .step_all(record("inner"))
            .step::<Admin>(record("admin")),
    )
    .await;

    let report = server.get("/admin/report").await;
    let missing = server.get("/missing").await;

    assert_eq!(report.status, StatusCode::NO_CONTENT);
    assert_eq!(report.headers["x-answered-by"], "outer");
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(
        *seen.lock().unwrap(),
        [
            "admin GET /admin/report 204",
            "inner GET /admin/report 204",
            "outer GET /admin/report 204",
            "inner GET /missing 404",
            "outer GET /missing 404",
        ]
    );
}

#[tokio::test]
async fn an_app_wide_check_refuses_in_every_area_and_runs_after_csrf() {
    let checked = seen();
    let server = TestServer::start(app().step_all(Counts {
        checked: Arc::clone(&checked),
    }))
    .await;

    let blocked_web = server.get("/hello").header("x-blocked", "yes").await;
    let blocked_admin = server.get("/admin/report").header("x-blocked", "yes").await;
    let allowed = server.get("/hello").await;
    let cross_site = server
        .post("/note")
        .header("sec-fetch-site", "cross-site")
        .await;

    assert_eq!(blocked_web.status, StatusCode::NOT_FOUND);
    assert_eq!(blocked_admin.status, StatusCode::NOT_FOUND);
    assert_eq!(allowed.status, StatusCode::NO_CONTENT);
    assert_eq!(cross_site.status, StatusCode::FORBIDDEN);
    assert_eq!(
        *checked.lock().unwrap(),
        ["/hello", "/admin/report", "/hello"]
    );
}

#[tokio::test]
async fn a_step_cannot_change_the_frameworks_security_headers_or_cookie() {
    let refused = seen();
    let server = TestServer::start(app().step_all(Overrides {
        refused: Arc::clone(&refused),
    }))
    .await;

    let answer = server.get("/hello").await;

    assert_eq!(refused.lock().unwrap().len(), 5);
    assert_eq!(answer.headers["x-frame-options"], "DENY");
    assert!(
        answer.headers["strict-transport-security"]
            .to_str()
            .unwrap()
            .starts_with("max-age=63072000")
    );
    assert!(!answer.headers.contains_key("set-cookie"));
    assert!(!answer.headers.contains_key("access-control-allow-origin"));
    assert_eq!(answer.headers["cache-control"], "private, max-age=60");
}

#[tokio::test]
async fn app_wide_steps_see_no_probe_metrics_asset_or_preflight() {
    let seen = seen();
    let cors = Cors::new()
        .origin("https://app.example")
        .methods(&[Method::GET]);
    let server = TestServer::start(app().cors::<Admin>(cors).step_all(Sees {
        seen: Arc::clone(&seen),
    }))
    .await;

    let stylesheet = server.get(&sofie_engine::STYLESHEET.path()).await;
    server.get("/live").await;
    server.get("/ready").await;
    #[cfg(feature = "metrics")]
    server.get("/metrics").await;
    let preflight = server
        .request(Method::OPTIONS, "/admin/report")
        .header("origin", "https://app.example")
        .header("access-control-request-method", "GET")
        .await;
    server.get("/hello").await;

    assert_eq!(stylesheet.status, StatusCode::OK);
    assert_eq!(preflight.status, StatusCode::NO_CONTENT);
    assert_eq!(*seen.lock().unwrap(), ["check /hello", "answered /hello"]);
}
