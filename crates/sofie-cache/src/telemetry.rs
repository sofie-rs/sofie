//! Spans and metrics name the store and operation, never a key or a value.

use sofie_core::Result;
use tracing::Instrument;
use tracing::field::Empty;

#[cfg(feature = "metrics")]
static STORE_DURATION: sofie_telemetry::metrics::Instrument =
    sofie_telemetry::metrics::Instrument {
        name: "sofie.cache.duration",
        description: "How long a call of the cache's shared store took, by store, operation and outcome.",
        kind: sofie_telemetry::metrics::Kind::Duration,
        labels: &["cache.store", "cache.operation", "outcome"],
    };
#[cfg(feature = "metrics")]
static LOOKUPS: sofie_telemetry::metrics::Instrument = sofie_telemetry::metrics::Instrument {
    name: "sofie.cache.lookups",
    description: "Cache lookups, by the last tier asked (`memory` or the store) and hit or miss.",
    kind: sofie_telemetry::metrics::Kind::Counter,
    labels: &["cache.tier", "outcome"],
};

#[derive(Clone, Copy)]
pub(crate) enum Operation {
    Get,
    Set,
    Delete,
    DropTagged,
}

impl Operation {
    const fn names(self) -> (&'static str, &'static str) {
        match self {
            Self::Get => ("cache.get", "get"),
            Self::Set => ("cache.set", "set"),
            Self::Delete => ("cache.delete", "delete"),
            Self::DropTagged => ("cache.drop_tagged", "drop_tagged"),
        }
    }
}

pub(crate) async fn call<T>(
    store: &'static str,
    operation: Operation,
    work: impl Future<Output = Result<T>>,
) -> Result<T> {
    let (span_name, operation) = operation.names();
    let span = tracing::info_span!(
        "cache",
        otel.name = span_name,
        otel.kind = "client",
        cache.store = store,
        cache.operation = operation,
        outcome = Empty,
    );
    #[cfg(feature = "metrics")]
    let started = std::time::Instant::now();
    let result = work.instrument(span.clone()).await;
    let outcome = if result.is_ok() { "ok" } else { "error" };
    span.record("outcome", outcome);
    #[cfg(feature = "metrics")]
    sofie_telemetry::metrics::record_duration(
        &STORE_DURATION,
        &[store, operation, outcome].map(sofie_telemetry::metrics::Label::from),
        started.elapsed(),
    );
    result
}

pub(crate) fn looked_up(tier: &'static str, hit: bool) {
    #[cfg(feature = "metrics")]
    sofie_telemetry::metrics::record(
        &LOOKUPS,
        &[tier, if hit { "hit" } else { "miss" }].map(sofie_telemetry::metrics::Label::from),
        1.0,
    );
    #[cfg(not(feature = "metrics"))]
    let _ = (tier, hit);
}
