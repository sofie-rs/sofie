use std::fmt;
use std::ops::ControlFlow;
use std::time::{Duration, Instant};

use http::header::{AUTHORIZATION, WWW_AUTHENTICATE};
use http::request::Parts;
use http::{HeaderValue, Method, StatusCode};
use sofie_core::{Error, Result};
use sofie_telemetry::metrics::{self, Instrument, Kind};

use crate::answer::{self, Response, TEXT};
use crate::trace::RouteLabel;

pub(crate) const PATH: &str = "/metrics";
const PROMETHEUS_TEXT: &str = "text/plain; version=0.0.4; charset=utf-8";
/// Generated wiring names every route.
const UNNAMED: &str = "unnamed";
const UNMATCHED: &str = "unmatched";
const OTHER_METHOD: &str = "_OTHER";
const MIN_TOKEN_LENGTH: usize = 32;

static REQUEST_DURATION: Instrument = Instrument {
    name: "http.server.request.duration",
    description: "How long a request took, by route name, method, status and area.",
    kind: Kind::Duration,
    labels: &[
        "http.route",
        "http.request.method",
        "http.response.status_code",
        "area",
    ],
};
static HANDLER_DURATION: Instrument = Instrument {
    name: "sofie.handler.duration",
    description: "How long an action or a loader took, commit included, by handler and module.",
    kind: Kind::Duration,
    labels: &["code.function.name", "module"],
};

#[derive(Clone, Copy)]
pub(crate) struct RequestMeasure {
    route: &'static str,
    method: &'static str,
    area: &'static str,
}

impl RequestMeasure {
    pub(crate) fn of(method: &Method, route: Option<RouteLabel<'_>>) -> Self {
        Self {
            route: route.map_or(UNMATCHED, |route| route.name.unwrap_or(UNNAMED)),
            method: method_label(method),
            area: route.and_then(|route| route.area).unwrap_or_default(),
        }
    }

    pub(crate) fn record(self, status: u16, elapsed: Duration) {
        let labels = [
            self.route.into(),
            self.method.into(),
            status.into(),
            self.area.into(),
        ];
        metrics::record_duration(&REQUEST_DURATION, &labels, elapsed);
    }
}

/// A method a client made up is one label, so it cannot add series.
fn method_label(method: &Method) -> &'static str {
    match *method {
        Method::GET => "GET",
        Method::HEAD => "HEAD",
        Method::POST => "POST",
        Method::PUT => "PUT",
        Method::PATCH => "PATCH",
        Method::DELETE => "DELETE",
        Method::OPTIONS => "OPTIONS",
        _ => OTHER_METHOD,
    }
}

pub(crate) async fn timed<T>(
    (name, module): crate::trace::HandlerName,
    work: impl Future<Output = T>,
) -> T {
    let started = Instant::now();
    let output = work.await;
    metrics::record_duration(
        &HANDLER_DURATION,
        &[name.into(), module.into()],
        started.elapsed(),
    );
    output
}

impl crate::Serving {
    /// Without it `/metrics` does not exist; `sofie::run` reads `SOFIE_METRICS_TOKEN`.
    #[must_use]
    pub fn metrics_token(mut self, token: MetricsToken) -> Self {
        self.metrics_token = Some(token);
        self
    }
}

/// Never printed: `Debug` shows only that there is one.
pub struct MetricsToken(String);

impl MetricsToken {
    pub fn parse(text: &str) -> Result<Self> {
        let text = text.trim();
        if text.len() < MIN_TOKEN_LENGTH || !text.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err(Error::internal_message(format!(
                "a metrics token needs at least {MIN_TOKEN_LENGTH} visible ASCII characters"
            )));
        }
        Ok(Self(text.to_owned()))
    }

    /// Constant time, so timing does not tell how much of a guess was right.
    fn admits(&self, authorization: Option<&HeaderValue>) -> bool {
        let Some(sent) = authorization.and_then(crate::caller::bearer_credentials) else {
            return false;
        };
        let (sent, own) = (sent.as_bytes(), self.0.as_bytes());
        sent.len() == own.len()
            && sent
                .iter()
                .zip(own)
                .fold(0, |difference, (sent, own)| difference | (sent ^ own))
                == 0
    }
}

impl fmt::Debug for MetricsToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MetricsToken(..)")
    }
}

pub(crate) async fn scrape(request: &Parts, token: Option<&MetricsToken>) -> ControlFlow<Response> {
    if request.uri.path() != PATH
        || (request.method != Method::GET && request.method != Method::HEAD)
    {
        return ControlFlow::Continue(());
    }
    let Some(token) = token else {
        return ControlFlow::Break(answer::response(
            StatusCode::NOT_FOUND,
            Some(TEXT),
            "Not found.",
        ));
    };
    if !token.admits(request.headers.get(AUTHORIZATION)) {
        tracing::warn!(
            http.route = PATH,
            "refused a scrape without the metrics token"
        );
        let mut refused = answer::response(StatusCode::UNAUTHORIZED, Some(TEXT), "Unauthorized.");
        refused
            .headers_mut()
            .insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        return ControlFlow::Break(refused);
    }
    ControlFlow::Break(match metrics::prometheus().await {
        Some(text) => answer::response(StatusCode::OK, Some(PROMETHEUS_TEXT), text),
        None => answer::response(
            StatusCode::SERVICE_UNAVAILABLE,
            Some(TEXT),
            "unavailable: metrics",
        ),
    })
}
