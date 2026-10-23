#[cfg(feature = "capture")]
mod capture;
#[cfg(feature = "development")]
pub mod development;
mod entered;
mod fields;
mod filter;
mod init;
mod json;
mod line;
#[cfg(not(feature = "otlp"))]
mod local;
#[cfg(feature = "log-fields")]
mod logged;
#[cfg(feature = "metrics")]
pub mod metrics;
#[cfg(feature = "otlp")]
mod otlp;
mod outage;
#[cfg(feature = "product-events")]
mod product;
mod sample;
mod scrub;
mod store;
mod subscriber;
mod timestamp;
mod traceparent;
mod writer;

#[cfg(not(feature = "otlp"))]
use local as context;
#[cfg(feature = "otlp")]
use otlp as context;

#[cfg(feature = "capture")]
pub use capture::{Capture, CapturedEvent, CapturedSpan, capture};
pub use init::{InitError, init};
#[cfg(feature = "log-fields")]
pub use logged::{Logged, Member, PlainMember, SensitiveMember};
pub use outage::outage;
#[cfg(feature = "product-events")]
pub use product::{PRODUCT_TARGET, ProductRecord, attach_principal, keep_product_events};
pub use traceparent::TraceParent;
#[doc(hidden)]
pub use tracing as __tracing;

use subscriber::Telemetry;
use tracing::Span;
use tracing::span::Id;

/// Without a span-tracking subscriber a traceparent is made up, so a caller can always answer one.
#[must_use]
pub fn continue_trace(span: &Span, parent: Option<TraceParent>) -> TraceParent {
    let own = with_telemetry(span, |telemetry, id| telemetry.spans.adopt(id, parent))
        .unwrap_or_else(|| parent.map_or_else(TraceParent::root, TraceParent::child));
    span.record("trace_id", own.trace_id());
    own
}

#[must_use]
pub fn current_traceparent() -> Option<TraceParent> {
    with_telemetry(&Span::current(), |telemetry, id| {
        Some(telemetry.spans.nearest_trace(id)?.traceparent())
    })
}

/// Each product event inside `span` carries what `evaluated` answers then, as `feature_flags`.
#[cfg_attr(
    not(feature = "product-events"),
    expect(unused_variables, reason = "only product events carry flags")
)]
pub fn attach_feature_flags(span: &Span, evaluated: impl Fn() -> String + Send + Sync + 'static) {
    #[cfg(feature = "product-events")]
    with_telemetry(span, |telemetry, id| {
        telemetry.spans.with(id, |entry| {
            entry.feature_flags = Some(std::sync::Arc::new(evaluated));
        })
    });
}

/// Call once, when the process stops.
pub async fn shutdown() {
    #[cfg(feature = "otlp")]
    otlp::shutdown().await;
    #[cfg(not(feature = "otlp"))]
    flush_lines();
}

fn flush_lines() {
    tracing::dispatcher::get_default(|dispatch| {
        if let Some(telemetry) = dispatch.downcast_ref::<Telemetry>() {
            telemetry.flush();
        }
    });
}

fn with_telemetry<T>(span: &Span, visit: impl FnOnce(&Telemetry, &Id) -> Option<T>) -> Option<T> {
    span.with_subscriber(|(id, dispatch)| visit(dispatch.downcast_ref::<Telemetry>()?, id))
        .flatten()
}
