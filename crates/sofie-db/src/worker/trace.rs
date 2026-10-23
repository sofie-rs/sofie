use sofie_core::Error;
use tracing::Span;
use tracing::field::Empty;

use crate::Claimed;

#[derive(Clone, Copy)]
pub(crate) enum Work {
    Event,
    Listener,
    Job,
}

impl Work {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Event => "event",
            Self::Listener => "listener",
            Self::Job => "job",
        }
    }
}

/// A listener joins the trace of the request that raised its event.
pub(crate) fn message_span(message: &Claimed, work: Work, registered: &str) -> Span {
    let name = format!("{} {registered}", work.label());
    let span = tracing::info_span!(
        "message",
        otel.name = name.as_str(),
        otel.kind = "consumer",
        otel.status_code = Empty,
        message.id = %message.id,
        module = registered.split('.').next(),
        attempt = message.attempt,
        trace_id = Empty,
    );
    let _ = sofie_telemetry::continue_trace(&span, message.traceparent);
    span
}

pub(crate) fn failed(message: &Claimed, error: &Error, dead: bool) {
    Span::current().record("otel.status_code", "ERROR");
    if dead {
        tracing::error!(
            error.kind = "job_failed",
            error = %error,
            message.name = %message.name,
            attempt = message.attempt,
            dead,
            "a message failed for the last time"
        );
        return;
    }
    tracing::warn!(
        error.kind = "job_failed",
        error = %error,
        message.name = %message.name,
        attempt = message.attempt,
        dead,
        "a message failed and will be retried"
    );
}
