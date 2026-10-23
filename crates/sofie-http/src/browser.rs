//! Nothing a person typed or a page showed is accepted: only a kind, a listed name and a number.

use std::ops::ControlFlow;

use http::{Method, StatusCode};
use hyper::body::Incoming;
use serde_json::Value;
use sofie_core::{Error, Result, Timestamp};
use sofie_db::Db;

use crate::admission::too_many_requests;
use crate::answer::{self, Response};
use crate::cookie::CookieNames;
use crate::pipeline::{Asked, read_body};
use crate::routes::Route;
use crate::throttle::{Limiter, Rate, Requester};
use crate::{RouteName, csrf};

pub(crate) const PATH: &str = "/_sofie/telemetry";
const BODY_LIMIT: usize = 8 * 1024;
const RECORDS_LIMIT: usize = 32;
/// Past this a value is a broken clock, not a measurement.
const VALUE_LIMIT: f64 = 3_600_000.0;
/// Past this an address is flooding the log, not browsing.
const RATE: Rate = Rate::per_minute(60);
const OTHER: &str = "other";
const UNNAMED: &str = "unnamed";

const KINDS: [(&str, &[&str]); 4] = [
    ("vital", &["CLS", "FCP", "INP", "LCP", "TTFB"]),
    (
        "error",
        &[
            "Error",
            "AggregateError",
            "EvalError",
            "RangeError",
            "ReferenceError",
            "SyntaxError",
            "TypeError",
            "URIError",
            "unhandledrejection",
        ],
    ),
    ("navigation", &["load", "htmx", "client"]),
    ("long_task", &["longtask"]),
];

#[cfg(feature = "metrics")]
static BROWSER_EVENTS: sofie_telemetry::metrics::Instrument =
    sofie_telemetry::metrics::Instrument {
        name: "sofie.browser.events",
        description: "Browser telemetry records, by kind, name and page route.",
        kind: sofie_telemetry::metrics::Kind::Counter,
        labels: &["browser.kind", "browser.name", "http.route"],
    };

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrowserSample(f64);

impl BrowserSample {
    pub fn parse(text: &str) -> Result<Self> {
        let share: f64 = text.trim().parse().map_err(|_| refusal(text))?;
        if !(0.0..=1.0).contains(&share) {
            return Err(refusal(text));
        }
        Ok(Self(share))
    }
}

impl Default for BrowserSample {
    fn default() -> Self {
        Self(1.0)
    }
}

fn refusal(text: &str) -> Error {
    Error::internal_message(format!(
        "browser sample `{text}` is not a share from 0 to 1, such as 0.1"
    ))
}

impl crate::Serving {
    /// `sofie::run` reads it from `SOFIE_BROWSER_SAMPLE`.
    #[must_use]
    pub fn browser_sample(mut self, sample: BrowserSample) -> Self {
        self.browser_sample = sample;
        self
    }
}

pub(crate) struct Browser {
    routes: Vec<&'static str>,
    sample: BrowserSample,
    limiter: Limiter,
    cookies: CookieNames,
}

impl Browser {
    pub(crate) fn new(
        names: &[RouteName],
        routes: &[Route],
        sample: BrowserSample,
        cookies: CookieNames,
    ) -> Self {
        let libraries = routes.iter().filter_map(|route| route.name);
        Self {
            routes: names
                .iter()
                .map(|name| name.name)
                .chain(libraries)
                .collect(),
            sample,
            limiter: Limiter::new(RATE),
            cookies,
        }
    }

    /// Every value is the server's own.
    pub(crate) fn meta(&self, route: Option<&'static str>, traceparent: &str) -> String {
        format!(
            "<meta name=\"sofie-telemetry\" content=\"{PATH}\" data-route=\"{}\" \
             data-traceparent=\"{traceparent}\" data-sample=\"{}\">",
            route.unwrap_or(UNNAMED),
            self.sample.0
        )
    }

    pub(crate) async fn answer(
        &self,
        asked: Asked<'_>,
        db: &Db,
        body: &mut Incoming,
    ) -> ControlFlow<Response> {
        if asked.request.method != Method::POST || asked.request.uri.path() != PATH {
            return ControlFlow::Continue(());
        }
        let now = db.clock().now().timestamp();
        ControlFlow::Break(self.receive(asked, body, now).await)
    }

    async fn receive(&self, asked: Asked<'_>, body: &mut Incoming, now: Timestamp) -> Response {
        let caller = asked.caller;
        if csrf::refused_by(asked.request, &self.cookies).is_some() {
            return answer::refusal(StatusCode::FORBIDDEN, "Refused.", caller);
        }
        if let Err(wait) = self.limiter.admit(Requester::of(None, asked.client), now) {
            return too_many_requests(wait, asked);
        }
        let body = match read_body(body, BODY_LIMIT, caller).await {
            Ok(body) => body,
            Err(refusal) => return *refusal,
        };
        let Ok(posted) = serde_json::from_slice::<Value>(&body) else {
            return answer::refusal(StatusCode::BAD_REQUEST, "Not telemetry.", caller);
        };
        let route = posted["route"]
            .as_str()
            .and_then(|named| self.routes.iter().copied().find(|route| *route == named))
            .unwrap_or(UNNAMED);
        let records = posted["records"].as_array().map_or(&[][..], Vec::as_slice);
        for record in records.iter().take(RECORDS_LIMIT) {
            if let Some((kind, name, value)) = accepted(record) {
                recorded(route, kind, name, value);
            }
        }
        answer::response(StatusCode::NO_CONTENT, None, bytes::Bytes::new())
    }
}

/// Milliseconds, unitless for `CLS`.
fn accepted(record: &Value) -> Option<(&'static str, &'static str, f64)> {
    let (kind, names) = KINDS
        .iter()
        .find(|(kind, _)| record["kind"].as_str() == Some(*kind))?;
    let name = names
        .iter()
        .copied()
        .find(|name| record["name"].as_str() == Some(*name))
        .unwrap_or(OTHER);
    let value = record["value"]
        .as_f64()
        .filter(|value| value.is_finite())
        .map_or(0.0, |value| value.clamp(0.0, VALUE_LIMIT));
    Some((kind, name, value))
}

fn recorded(route: &'static str, kind: &'static str, name: &'static str, value: f64) {
    tracing::info!(
        browser.kind = kind,
        browser.name = name,
        browser.value = value,
        http.route = route,
        "browser telemetry"
    );
    #[cfg(feature = "metrics")]
    sofie_telemetry::metrics::record(
        &BROWSER_EVENTS,
        &[kind.into(), name.into(), route.into()],
        1.0,
    );
}
