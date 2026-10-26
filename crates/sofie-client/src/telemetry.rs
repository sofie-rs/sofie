//! Spans and metrics never record a header, a body or a secret.

use std::time::{Duration, Instant};

use http::Method;
use sofie_core::{Error, Result};
use tracing::field::Empty;
use tracing::{Instrument, Span};

use crate::Shared;
use crate::breaker;
use crate::request::Response;
use crate::target::Target;

#[cfg(feature = "metrics")]
static GATEWAY_DURATION: sofie_telemetry::metrics::Instrument =
    sofie_telemetry::metrics::Instrument {
        name: "sofie.gateway.duration",
        description: "How long a call of a gateway's adapter or an operation of its vendor took, by vendor, operation and outcome.",
        kind: sofie_telemetry::metrics::Kind::Duration,
        labels: &["gateway.vendor", "gateway.operation", "outcome"],
    };
#[cfg(feature = "metrics")]
static OUTGOING_DURATION: sofie_telemetry::metrics::Instrument =
    sofie_telemetry::metrics::Instrument {
        name: "sofie.outgoing.duration",
        description: "How long a request leaving the app took, by gateway.vendor, gateway.operation (the method) and outcome (status class or error).",
        kind: sofie_telemetry::metrics::Kind::Duration,
        labels: &["gateway.vendor", "gateway.operation", "outcome"],
    };

pub fn call<T>(
    vendor: &'static str,
    operation: &'static str,
    work: impl Future<Output = Result<T>>,
) -> impl Future<Output = Result<T>> {
    let span = tracing::info_span!(
        "gateway",
        otel.name = format!("{vendor}.{operation}"),
        gateway.vendor = vendor,
        gateway.operation = operation,
        outcome = Empty,
    );
    measured(span, vendor, operation, work)
}

pub(crate) fn operation<T>(
    shared: &Shared,
    name: &'static str,
    work: impl Future<Output = Result<T>>,
) -> impl Future<Output = Result<T>> {
    let vendor = shared.vendor;
    let span = tracing::info_span!(
        "outgoing",
        otel.name = format!("{vendor}.{name}"),
        otel.kind = "client",
        gateway.vendor = vendor,
        gateway.operation = name,
        outcome = Empty,
    );
    measured(span, vendor, name, bounded(shared, name, work))
}

async fn bounded<T>(
    shared: &Shared,
    name: &'static str,
    work: impl Future<Output = Result<T>>,
) -> Result<T> {
    let vendor = shared.vendor;
    let passage = shared.operations.admit(vendor)?;
    let total = shared.limits.total;
    let result = tokio::time::timeout(total, work).await.unwrap_or_else(|_| {
        Err(Error::Unavailable(format!(
            "{vendor}'s {name} did not finish within {} ms",
            total.as_millis()
        )))
    });
    match &result {
        Err(error) if breaker::refused_open(error) => {}
        Err(Error::Unavailable(_)) => {
            shared
                .operations
                .record(vendor, passage, false, Instant::now());
        }
        _ => shared
            .operations
            .record(vendor, passage, true, Instant::now()),
    }
    result
}

/// Requests are measured apart, under `sofie.outgoing.duration`, so none is counted twice.
async fn measured<T>(
    span: Span,
    vendor: &'static str,
    operation: &'static str,
    work: impl Future<Output = Result<T>>,
) -> Result<T> {
    let started = Instant::now();
    let result = work.instrument(span.clone()).await;
    let outcome = if result.is_ok() { "ok" } else { "error" };
    span.record("outcome", outcome);
    record(
        Meter::Gateway,
        [vendor, operation, outcome],
        started.elapsed(),
    );
    result
}

pub(crate) fn http_span(vendor: &'static str, method: &Method, target: &Target) -> Span {
    tracing::info_span!(
        "http.client",
        otel.name = method.as_str(),
        otel.kind = "client",
        otel.status_code = Empty,
        gateway.vendor = vendor,
        server.address = target.host.as_str(),
        server.port = target.port,
        http.request.method = method.as_str(),
        http.response.status_code = Empty,
        http.request.resend_count = Empty,
        duration_ms = Empty,
    )
}

pub(crate) fn finish_http(
    span: &Span,
    vendor: &'static str,
    method: &Method,
    result: &Result<Response>,
    started: Instant,
) {
    let elapsed = started.elapsed();
    span.record(
        "duration_ms",
        u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
    );
    let outcome = match result {
        Ok(response) => {
            span.record("http.response.status_code", response.status());
            if response.status.is_server_error() {
                span.record("otel.status_code", "ERROR");
            }
            status_class(response.status.as_u16())
        }
        Err(error) => {
            span.record("otel.status_code", "ERROR");
            span.in_scope(|| tracing::warn!(error = %error, "outgoing request failed"));
            "error"
        }
    };
    record(
        Meter::Outgoing,
        [vendor, method_name(method), outcome],
        elapsed,
    );
}

fn status_class(status: u16) -> &'static str {
    match status {
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        _ => "5xx",
    }
}

#[derive(Clone, Copy)]
enum Meter {
    Gateway,
    Outgoing,
}

fn record(meter: Meter, labels: [&'static str; 3], elapsed: Duration) {
    #[cfg(feature = "metrics")]
    {
        let instrument = match meter {
            Meter::Gateway => &GATEWAY_DURATION,
            Meter::Outgoing => &OUTGOING_DURATION,
        };
        let labels = labels.map(sofie_telemetry::metrics::Label::from);
        sofie_telemetry::metrics::record_duration(instrument, &labels, elapsed);
    }
    #[cfg(not(feature = "metrics"))]
    let _ = (meter, labels, elapsed);
}

/// A label is a `'static` name, which a method made at runtime is not.
fn method_name(method: &Method) -> &'static str {
    match *method {
        Method::GET => "GET",
        Method::POST => "POST",
        Method::PUT => "PUT",
        Method::PATCH => "PATCH",
        Method::DELETE => "DELETE",
        Method::HEAD => "HEAD",
        Method::OPTIONS => "OPTIONS",
        _ => "_OTHER",
    }
}
