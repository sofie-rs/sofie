use std::time::{Duration, Instant};

use http::request::Parts;
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use sofie_core::{Result, Uuid};
use sofie_telemetry::TraceParent;
use tracing::Span;
use tracing::field::Empty;

use crate::answer::Response;

const TRACEPARENT: &str = "traceparent";
const REQUEST_ID: &str = "x-request-id";
const EDGE_ID_MAX_LENGTH: usize = 128;
const SLOW_REQUEST: Duration = Duration::from_secs(1);

/// The matched route's name, else its declared pattern or a framework path.
#[derive(Clone, Copy)]
pub(crate) struct RouteLabel<'r> {
    pub(crate) pattern: &'r str,
    pub(crate) name: Option<&'static str>,
    pub(crate) area: Option<&'static str>,
    pub(crate) module: Option<&'static str>,
}

/// A probe's has no span and logs nothing; metrics still count it.
pub(crate) struct RequestTrace {
    span: Option<Span>,
    traceparent: TraceParent,
    request_id: String,
    started: Instant,
    #[cfg(feature = "metrics")]
    measure: crate::metrics::RequestMeasure,
}

impl RequestTrace {
    /// `edge_id_header` is `Some` only for a trusted proxy, whose request id and `traceparent` are
    /// kept: anyone else's would let a client pick trace ids and sampling.
    pub(crate) fn start(
        request: &Parts,
        route: Option<RouteLabel<'_>>,
        edge_id_header: Option<&HeaderName>,
    ) -> Result<Self> {
        // Always the server's own: a client's id could carry what a log must not.
        let request_id = Uuid::now_v7()?.to_string();
        let edge_request_id = edge_id_header
            .and_then(|header| request.headers.get(header))
            .and_then(edge_request_id);
        let name = span_name(&request.method, route);
        let span = tracing::info_span!(
            "request",
            otel.name = name.as_str(),
            otel.kind = "server",
            otel.status_code = Empty,
            http.request.method = request.method.as_str(),
            http.route = route.map(|route| route.pattern),
            http.response.status_code = Empty,
            area = route.and_then(|route| route.area),
            module = route.and_then(|route| route.module),
            request_id = request_id.as_str(),
            edge_request_id,
            trace_id = Empty,
            duration_ms = Empty,
            feature_flags = Empty,
            permission = Empty,
        );
        let incoming = edge_id_header
            .and_then(|_| request.headers.get(TRACEPARENT))
            .and_then(|value| value.to_str().ok())
            .and_then(TraceParent::parse);
        Ok(Self {
            traceparent: sofie_telemetry::continue_trace(&span, incoming),
            span: Some(span),
            request_id,
            started: Instant::now(),
            #[cfg(feature = "metrics")]
            measure: crate::metrics::RequestMeasure::of(&request.method, route),
        })
    }

    #[cfg_attr(
        not(feature = "metrics"),
        expect(unused_variables, reason = "only metrics count a probe")
    )]
    pub(crate) fn probe(request: &Parts, route: Option<RouteLabel<'_>>) -> Result<Self> {
        Ok(Self {
            span: None,
            traceparent: sofie_telemetry::continue_trace(&Span::none(), None),
            request_id: Uuid::now_v7()?.to_string(),
            started: Instant::now(),
            #[cfg(feature = "metrics")]
            measure: crate::metrics::RequestMeasure::of(&request.method, route),
        })
    }

    pub(crate) fn span(&self) -> Span {
        self.span.clone().unwrap_or_else(Span::none)
    }

    pub(crate) fn finish(&self, mut response: Response) -> Response {
        let status = response.status();
        let elapsed = self.started.elapsed();
        #[cfg(feature = "metrics")]
        self.measure.record(status.as_u16(), elapsed);
        if let Some(span) = &self.span {
            log_finished(span, status, elapsed);
        }
        let headers = response.headers_mut();
        insert(headers, TRACEPARENT, self.traceparent.to_string());
        insert(headers, REQUEST_ID, self.request_id.clone());
        response
    }
}

fn log_finished(span: &Span, status: StatusCode, elapsed: Duration) {
    let duration_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
    let code = status.as_u16();
    span.record("http.response.status_code", code);
    span.record("duration_ms", duration_ms);
    if status.is_server_error() {
        span.record("otel.status_code", "ERROR");
    }
    span.in_scope(|| {
        // One line per request costs more than it tells; the span keeps the rest. A
        // development build keeps every line, which the dev tools read.
        if cfg!(debug_assertions) || is_notable(status, elapsed) {
            tracing::info!(
                http.response.status_code = code,
                duration_ms,
                "request finished"
            );
        } else {
            tracing::debug!(
                http.response.status_code = code,
                duration_ms,
                "request finished"
            );
        }
    });
}

/// A handler's path and its module's, read once when its route is added.
pub(crate) type HandlerName = (&'static str, &'static str);

pub(crate) fn handler_span((name, module): HandlerName) -> Span {
    tracing::info_span!("handler", otel.name = name, module)
}

pub(crate) fn handler_name<F>() -> HandlerName {
    let name = std::any::type_name::<F>();
    (name, name.split("::").next().unwrap_or_default())
}

fn is_notable(status: StatusCode, elapsed: Duration) -> bool {
    status.is_server_error() || elapsed >= SLOW_REQUEST
}

/// Bounded and plain, so a proxy cannot inject log syntax or bloat every line.
fn edge_request_id(value: &HeaderValue) -> Option<&str> {
    let text = value.to_str().ok()?;
    let plain = text
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:;=/+@".contains(&byte));
    (plain && !text.is_empty() && text.len() <= EDGE_ID_MAX_LENGTH).then_some(text)
}

fn span_name(method: &Method, route: Option<RouteLabel<'_>>) -> String {
    match route {
        Some(route) => format!("{method} {}", route.name.unwrap_or(route.pattern)),
        None => method.to_string(),
    }
}

/// ASCII by construction, so the conversion cannot fail.
fn insert(headers: &mut HeaderMap, name: &'static str, text: String) {
    if let Ok(value) = HeaderValue::try_from(text) {
        headers.insert(name, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_failed_or_slow_request_is_logged_at_info() {
        let fast = Duration::from_millis(5);

        assert!(!is_notable(StatusCode::OK, fast));
        assert!(!is_notable(StatusCode::NOT_FOUND, fast));
        assert!(is_notable(StatusCode::INTERNAL_SERVER_ERROR, fast));
        assert!(is_notable(StatusCode::OK, SLOW_REQUEST));
    }
}
