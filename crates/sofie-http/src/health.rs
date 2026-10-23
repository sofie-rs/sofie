//! Answered before routing, so no app route can shadow them.

use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;

use http::request::Parts;
use http::{Method, StatusCode};
use hyper::body::Incoming;
use sofie_core::{Error, Result};
use sofie_db::{BoxFuture, Db};

use crate::Serving;
use crate::answer::{self, Response, TEXT};
use crate::pipeline::Asked;

const LIVE: &str = "/live";
const READY: &str = "/ready";
/// Reserved whether or not the app serves them.
const RESERVED: [&str; 4] = [LIVE, READY, "/metrics", "/_sofie/telemetry"];
const READY_TIMEOUT: Duration = Duration::from_secs(2);
/// A flood of probes, which anyone may send, costs one round of checks per this.
const READY_KEPT: Duration = Duration::from_secs(1);

#[derive(Clone)]
pub(crate) struct Dependency {
    pub(crate) name: &'static str,
    pub(crate) check: Arc<dyn Fn() -> BoxFuture<'static, Result<()>> + Send + Sync>,
}

pub(crate) struct Endpoints {
    pub(crate) serving: Serving,
    #[cfg(feature = "browser-telemetry")]
    pub(crate) browser: crate::browser::Browser,
    #[cfg(all(feature = "toolbar", debug_assertions))]
    pub(crate) toolbar: crate::toolbar::Toolbar,
}

impl Endpoints {
    pub(crate) async fn answer(
        &self,
        asked: Asked<'_>,
        db: &Db,
        #[cfg_attr(
            not(any(
                feature = "browser-telemetry",
                all(feature = "toolbar", debug_assertions)
            )),
            expect(unused_variables)
        )]
        body: &mut Incoming,
    ) -> ControlFlow<Response> {
        if let Some(probe) = Probe::of(asked.request) {
            return ControlFlow::Break(probe.answer(db, &self.serving).await);
        }
        #[cfg(feature = "metrics")]
        crate::metrics::scrape(asked.request, self.serving.metrics_token.as_ref()).await?;
        #[cfg(feature = "browser-telemetry")]
        self.browser.answer(asked, db, body).await?;
        #[cfg(all(feature = "toolbar", debug_assertions))]
        self.toolbar.answer(asked, db, body).await?;
        ControlFlow::Continue(())
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Probe {
    Live,
    Ready,
}

impl Probe {
    pub(crate) fn of(request: &Parts) -> Option<Self> {
        if request.method != Method::GET && request.method != Method::HEAD {
            return None;
        }
        [Self::Live, Self::Ready]
            .into_iter()
            .find(|probe| probe.path() == request.uri.path())
    }

    pub(crate) fn path(self) -> &'static str {
        match self {
            Self::Live => LIVE,
            Self::Ready => READY,
        }
    }

    async fn answer(self, db: &Db, serving: &Serving) -> Response {
        let (status, body) = match self {
            Self::Live => (StatusCode::OK, "ok"),
            Self::Ready => serving.readiness.checked(db, &serving.dependencies).await,
        };
        answer::response(status, Some(TEXT), body)
    }
}

#[cfg(feature = "shield")]
pub(crate) fn is_reserved(path: &str) -> bool {
    RESERVED.contains(&path)
}

/// The framework answers its own paths before routing, so a route there would never be reached.
const RESERVED_PREFIX: &str = "/_sofie/";

pub(crate) fn refuse_reserved(method: &Method, path: &str) -> Result<()> {
    if !RESERVED.contains(&path) && !path.starts_with(RESERVED_PREFIX) {
        return Ok(());
    }
    Err(Error::internal_message(format!(
        "route {method} {path} is reserved for the framework's own endpoints"
    )))
}

/// The last answer and when it was found; held across the checks, so probes arriving meanwhile
/// wait for it instead of checking again.
#[derive(Default)]
pub(crate) struct Readiness(tokio::sync::Mutex<Option<(Instant, StatusCode, &'static str)>>);

impl Readiness {
    async fn checked(&self, db: &Db, dependencies: &[Dependency]) -> (StatusCode, &'static str) {
        let mut kept = self.0.lock().await;
        if let Some((at, status, body)) = *kept
            && at.elapsed() < READY_KEPT
        {
            return (status, body);
        }
        let (status, body) = readiness(db, dependencies).await;
        *kept = Some((Instant::now(), status, body));
        (status, body)
    }
}

/// Says what is down, and why, only to the log: anyone may ask.
async fn readiness(db: &Db, dependencies: &[Dependency]) -> (StatusCode, &'static str) {
    let fixed: [(&str, BoxFuture<'_, Result<()>>); 2] = [
        ("database", Box::pin(db.ping())),
        ("queue", Box::pin(sofie_db::ping_queue(db))),
    ];
    let declared = dependencies
        .iter()
        .map(|dependency| (dependency.name, (dependency.check)()));
    for (name, check) in fixed.into_iter().chain(declared) {
        let reason = match tokio::time::timeout(READY_TIMEOUT, check).await {
            Ok(Ok(())) => continue,
            Ok(Err(error)) => error.to_string(),
            Err(_) => format!("no answer within {READY_TIMEOUT:?}"),
        };
        tracing::warn!(dependency = name, error = %reason, "not ready");
        return (StatusCode::SERVICE_UNAVAILABLE, "unavailable");
    }
    match sofie_telemetry::outage() {
        Some(output) => {
            tracing::warn!(
                telemetry.output = output,
                "ready, but telemetry is not delivered"
            );
            (StatusCode::OK, "degraded")
        }
        None => (StatusCode::OK, "ok"),
    }
}

#[cfg(all(test, feature = "metrics", feature = "browser-telemetry"))]
mod tests {
    #[test]
    fn reserves_the_paths_the_optional_endpoints_serve() {
        assert!(super::RESERVED.contains(&crate::metrics::PATH));
        assert!(super::RESERVED.contains(&crate::browser::PATH));
    }
}
