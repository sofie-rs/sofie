use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use sofie_core::unpoisoned;
use tracing::subscriber::{DefaultGuard, NoSubscriber};
use tracing::{Dispatch, Level, Metadata};

use crate::context::Exporter;
use crate::fields::Fields;
use crate::store::SpanEntry;
use crate::subscriber::{Output, Telemetry};

#[derive(Clone, Debug)]
pub struct CapturedSpan {
    pub id: u64,
    pub parent: Option<u64>,
    pub name: &'static str,
    pub fields: BTreeMap<&'static str, String>,
    opened_as: u64,
}

#[derive(Clone, Debug)]
pub struct CapturedEvent {
    pub level: Level,
    pub target: &'static str,
    pub fields: BTreeMap<&'static str, String>,
}

pub struct Capture {
    telemetry: Arc<Telemetry>,
    recording: Arc<Recording>,
    _installed: DefaultGuard,
}

#[must_use]
pub fn capture() -> Capture {
    let recording = Arc::new(Recording::default());
    let output = Output::Recording(Arc::clone(&recording));
    let telemetry = Arc::new(Telemetry::new(output, Exporter::silent()));
    keep_interest_shared();
    let installed = tracing::subscriber::set_default(Arc::clone(&telemetry));
    Capture {
        telemetry,
        recording,
        _installed: installed,
    }
}

impl Capture {
    /// Closed and still open, in the order they opened.
    #[must_use]
    pub fn spans(&self) -> Vec<CapturedSpan> {
        let mut spans = unpoisoned::lock(&self.recording.closed_spans).clone();
        self.telemetry
            .spans
            .each(|id, entry| spans.push(CapturedSpan::of(id, entry)));
        spans.sort_by_key(|span| span.opened_as);
        spans
    }

    #[must_use]
    pub fn events(&self) -> Vec<CapturedEvent> {
        unpoisoned::lock(&self.recording.events).clone()
    }

    #[cfg(feature = "product-events")]
    #[must_use]
    pub fn product_events(&self) -> Vec<CapturedEvent> {
        let mut events = self.events();
        events.retain(|event| event.target == crate::PRODUCT_TARGET);
        events
    }
}

impl CapturedSpan {
    pub(crate) fn of(id: u64, entry: &SpanEntry) -> Self {
        Self {
            id,
            parent: entry.parent.as_ref().map(tracing::span::Id::into_u64),
            name: entry.metadata.name(),
            fields: rendered(&entry.fields),
            opened_as: entry.opened_as,
        }
    }
}

#[derive(Default)]
pub(crate) struct Recording {
    closed_spans: Mutex<Vec<CapturedSpan>>,
    events: Mutex<Vec<CapturedEvent>>,
}

impl Recording {
    pub(crate) fn span_closed(&self, span: CapturedSpan) {
        unpoisoned::lock(&self.closed_spans).push(span);
    }

    pub(crate) fn event(&self, metadata: &'static Metadata<'static>, fields: &Fields) {
        unpoisoned::lock(&self.events).push(CapturedEvent {
            level: *metadata.level(),
            target: metadata.target(),
            fields: rendered(fields),
        });
    }
}

fn rendered(fields: &Fields) -> BTreeMap<&'static str, String> {
    fields
        .iter()
        .map(|(name, value)| (name, value.to_string()))
        .collect()
}

#[cfg(not(feature = "otlp"))]
impl Exporter {
    pub(crate) fn silent() -> Self {
        Self
    }
}

/// With a single registered dispatcher, `tracing-core` caches a callsite's interest from the
/// default of whichever thread reaches it first, so a test running beside a capture could switch
/// the capture's spans off for good; a second, idle dispatcher makes it ask every dispatcher.
fn keep_interest_shared() {
    static IDLE: OnceLock<Dispatch> = OnceLock::new();
    IDLE.get_or_init(|| Dispatch::new(NoSubscriber::default()));
}
