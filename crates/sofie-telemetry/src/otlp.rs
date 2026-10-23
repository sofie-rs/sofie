//! Our own OTLP/HTTP exporter (review round G7): protobuf bodies, batched per signal and sent to
//! `OTEL_EXPORTER_OTLP_ENDPOINT` with `hyper`.

use std::borrow::Cow;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

use tracing::Metadata;

use crate::fields::{FieldValue, Fields, NAME_FIELD};
use crate::outage::Output;
use crate::sample::Sample;
use crate::store::SpanEntry;
use crate::subscriber::Export;
use crate::{InitError, TraceParent};

pub(crate) mod batch;
pub(crate) mod common;
mod logs;
pub(crate) mod protobuf;
pub(crate) mod settings;
mod spans;
mod transport;

use batch::{Closing, Queue};
use common::{Attribute, Value};
pub(crate) use logs::LogExport;
use settings::Settings;
use spans::{ParentSpan, SpanKind, SpanRecord, Spans, Status};
use tokio::time::Instant;

const KIND_FIELD: &str = "otel.kind";
const STATUS_CODE_FIELD: &str = "otel.status_code";
const STATUS_DESCRIPTION_FIELD: &str = "otel.status_description";
pub(crate) const FINAL_EXPORT_WAIT: Duration = Duration::from_secs(10);
const TRACE_SAMPLE_VARIABLE: &str = "SOFIE_TRACE_SAMPLE";

static SPANS: OnceLock<Arc<Queue<ClosedSpan>>> = OnceLock::new();
static TRACE_SAMPLE: OnceLock<Sample> = OnceLock::new();

#[derive(Clone, Copy, Debug)]
pub(crate) struct Trace {
    own: TraceParent,
    parent: ParentSpan,
    started: SystemTime,
}

impl Trace {
    pub(crate) fn open(parent: Option<&Self>) -> Self {
        let (own, parent) = match parent {
            Some(parent) => (parent.own.child(), ParentSpan::Local(parent.own.span_id)),
            None => (
                root(TRACE_SAMPLE.get().copied().unwrap_or(Sample::EVERY)),
                ParentSpan::None,
            ),
        };
        Self {
            own,
            parent,
            started: SystemTime::now(),
        }
    }

    /// The span must not have children yet.
    pub(crate) fn continue_from(&mut self, parent: TraceParent) {
        self.own = parent.child();
        self.parent = ParentSpan::Remote(parent.span_id);
    }

    #[cfg(feature = "development")]
    pub(crate) fn is_local_root(&self) -> bool {
        !matches!(self.parent, ParentSpan::Local(_))
    }

    pub(crate) fn traceparent(&self) -> TraceParent {
        self.own
    }

    fn is_sampled(&self) -> bool {
        self.own.flags & 1 == 1
    }
}

/// Decided once at the root, so a trace's spans are exported whole or not at all.
fn root(sample: Sample) -> TraceParent {
    let root = TraceParent::root();
    if sample.keeps(root.trace_id) {
        root
    } else {
        root.unsampled()
    }
}

pub(crate) struct Exporter {
    spans: Option<Arc<Queue<ClosedSpan>>>,
}

impl Exporter {
    pub(crate) fn silent() -> Self {
        Self { spans: None }
    }

    pub(crate) fn exporting() -> Result<Self, InitError> {
        let Some(settings) = Settings::from_environment()? else {
            return Ok(Self::silent());
        };
        let sample = Sample::from_environment(TRACE_SAMPLE_VARIABLE)?;
        let spans = batch::start(Spans, Arc::new(settings));
        let installed = SPANS.set(Arc::clone(&spans)).is_ok() && TRACE_SAMPLE.set(sample).is_ok();
        if !installed {
            return Err(InitError::new("the OTLP exporter was already installed"));
        }
        Ok(Self { spans: Some(spans) })
    }
}

impl Export for Exporter {
    fn closed(&self, span: &SpanEntry) {
        let (Some(spans), Some(trace)) = (&self.spans, &span.trace) else {
            return;
        };
        if trace.is_sampled() {
            spans.push(ClosedSpan {
                metadata: span.metadata,
                fields: Arc::clone(&span.fields),
                trace: *trace,
                end: SystemTime::now(),
            });
        }
    }
}

/// What a closing span hands over; its record is built on the exporter's task, off the request's.
#[derive(Clone)]
pub(crate) struct ClosedSpan {
    metadata: &'static Metadata<'static>,
    fields: Arc<Fields>,
    trace: Trace,
    end: SystemTime,
}

impl ClosedSpan {
    fn into_record(self) -> SpanRecord {
        let mut span = SpanRecord {
            own: self.trace.own,
            parent: self.trace.parent,
            name: Cow::Borrowed(self.metadata.name()),
            kind: SpanKind::Internal,
            start: self.trace.started,
            end: self.end,
            attributes: location(self.metadata),
            status: Status::Unset,
        };
        for (name, value) in self.fields.iter() {
            describe(&mut span, name, value);
        }
        span
    }
}

fn location(metadata: &'static Metadata<'static>) -> Vec<Attribute> {
    let mut attributes = Vec::new();
    if let Some(file) = metadata.file() {
        attributes.push(Attribute::new("code.file.path", file));
    }
    if let Some(module) = metadata.module_path() {
        attributes.push(Attribute::new("code.module.name", module));
    }
    if let Some(line) = metadata.line() {
        attributes.push(Attribute::new("code.line.number", i64::from(line)));
    }
    attributes
}

/// Text in an `otel.*` field sets the name, kind or status; other fields are attributes.
fn describe(span: &mut SpanRecord, name: &'static str, value: &FieldValue) {
    let text = match value {
        FieldValue::Text(text) | FieldValue::Formatted(text) | FieldValue::Composed(text) => {
            Some(text.as_str())
        }
        _ => None,
    };
    match (name, text) {
        (NAME_FIELD, Some(text)) => span.name = Cow::Owned(text.to_owned()),
        (KIND_FIELD, Some(text)) => span.kind = span_kind(text).unwrap_or(SpanKind::Internal),
        (STATUS_CODE_FIELD, Some(text)) => code_status(&mut span.status, text),
        (STATUS_DESCRIPTION_FIELD, Some(text)) => describe_status(&mut span.status, text),
        _ => span
            .attributes
            .push(Attribute::new(name, Value::from(value.clone()))),
    }
}

fn span_kind(text: &str) -> Option<SpanKind> {
    [
        ("server", SpanKind::Server),
        ("client", SpanKind::Client),
        ("producer", SpanKind::Producer),
        ("consumer", SpanKind::Consumer),
        ("internal", SpanKind::Internal),
    ]
    .into_iter()
    .find_map(|(name, kind)| text.eq_ignore_ascii_case(name).then_some(kind))
}

/// Fields come in declared order; a description alone means an error, and OK drops it.
fn code_status(status: &mut Status, text: &str) {
    *status = if text.eq_ignore_ascii_case("ok") {
        Status::Ok
    } else if !text.eq_ignore_ascii_case("error") {
        Status::Unset
    } else if let Status::Error(description) = std::mem::replace(status, Status::Unset) {
        Status::Error(description)
    } else {
        Status::Error(String::new())
    };
}

fn describe_status(status: &mut Status, text: &str) {
    if *status != Status::Ok {
        *status = Status::Error(text.to_owned());
    }
}

/// Every close is requested before any is awaited, so a dead collector costs one wait, not
/// one per signal; the logs close last, after the writer hands over its last lines.
pub(crate) async fn shutdown() {
    let deadline = Instant::now() + FINAL_EXPORT_WAIT;
    let spans = SPANS.get().map(|spans| (spans.close(), Output::Spans));
    #[cfg(feature = "metrics")]
    let metrics = crate::metrics::close(deadline)
        .await
        .map(|metrics| (metrics, Output::Metrics));
    #[cfg(not(feature = "metrics"))]
    let metrics = None;
    crate::flush_lines();
    let logs = logs::LOGS
        .get()
        .map(|logs| (logs.close(), Output::ExportedLogs));
    for (closing, output) in [spans, metrics, logs].into_iter().flatten() {
        finish(closing, output, deadline).await;
    }
}

async fn finish(closing: Closing, output: Output, deadline: Instant) {
    if !closing.finished_by(deadline).await {
        tracing::warn!(
            telemetry.output = output.name(),
            "the last export did not finish before shutdown"
        );
    }
}

#[cfg(all(test, feature = "capture"))]
mod tests {
    use std::sync::Arc;

    use tracing::field::Empty;

    use super::spans::{ParentSpan, SpanKind, SpanRecord, Status, scrub_span};
    use super::{ClosedSpan, Exporter, Queue, Value};
    use crate::TraceParent;
    use crate::sample::Sample;
    use crate::subscriber::{Output, Telemetry};

    const INCOMING: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
    const UNSAMPLED: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00";

    fn exported(work: impl FnOnce(&Queue<ClosedSpan>)) -> Vec<SpanRecord> {
        let spans = Arc::new(Queue::new(64, 64));
        let output = Output::Recording(Arc::default());
        let exporter = Exporter {
            spans: Some(Arc::clone(&spans)),
        };
        let telemetry = Telemetry::new(output, exporter);
        tracing::subscriber::with_default(telemetry, || work(&spans));
        spans
            .queued()
            .into_iter()
            .map(ClosedSpan::into_record)
            .collect()
    }

    fn attribute(span: &SpanRecord, key: &str) -> Option<Value> {
        span.attributes
            .iter()
            .find(|attribute| attribute.key == key)
            .map(|attribute| attribute.value.clone())
    }

    #[test]
    fn traced_spans_are_exported_with_their_traceparent_ids_once_closed() {
        let incoming = TraceParent::parse(INCOMING).unwrap();
        let mut own = None;
        let mut exported_before_close = 0;
        let spans = exported(|queued| {
            let request =
                tracing::info_span!("request", otel.name = "GET /orders", trace_id = Empty);
            own = Some(crate::continue_trace(&request, Some(incoming)));
            request.in_scope(|| {
                let internal = tracing::debug_span!("internal");
                internal.in_scope(|| tracing::info_span!("handler").in_scope(|| ()));
            });
            exported_before_close = queued.queued().len();
        });

        let own = own.unwrap();
        assert_eq!(exported_before_close, 1);
        let [handler, request] = spans.as_slice() else {
            panic!("two exported spans: {spans:?}");
        };
        assert_eq!(request.name, "GET /orders");
        assert_eq!(handler.name, "handler");
        assert_eq!(request.own.trace_id, incoming.trace_id);
        assert_eq!(request.parent, ParentSpan::Remote(incoming.span_id));
        assert_eq!(request.own.span_id, own.span_id);
        assert_eq!(handler.own.trace_id, request.own.trace_id);
        assert_eq!(handler.parent, ParentSpan::Local(request.own.span_id));
    }

    #[test]
    fn otel_fields_set_kind_and_status_and_other_fields_become_attributes() {
        let spans = exported(|_| {
            let span = tracing::info_span!(
                "request",
                otel.kind = "server",
                otel.status_code = Empty,
                route = "/orders",
                attempt = 2_u64,
            );
            span.record("otel.status_code", "ERROR");
        });

        let [span] = spans.as_slice() else {
            panic!("one exported span: {spans:?}");
        };
        assert_eq!(span.name, "request");
        assert_eq!(span.kind, SpanKind::Server);
        assert_eq!(span.status, Status::Error(String::new()));
        assert_eq!(span.parent, ParentSpan::None);
        assert_eq!(attribute(span, "route"), Some(Value::from("/orders")));
        assert_eq!(attribute(span, "attempt"), Some(Value::Int(2)));
        assert_eq!(attribute(span, "otel.kind"), None);
        assert!(attribute(span, "code.file.path").is_some());
    }

    #[test]
    fn a_status_reads_the_same_whichever_of_its_fields_is_recorded_first() {
        let statuses = |code: &'static str| {
            exported(|_| {
                let first = tracing::info_span!(
                    "first",
                    otel.status_code = Empty,
                    otel.status_description = Empty
                );
                first.record("otel.status_code", code);
                first.record("otel.status_description", "db down");
                let second = tracing::info_span!(
                    "second",
                    otel.status_description = Empty,
                    otel.status_code = Empty
                );
                second.record("otel.status_description", "db down");
                second.record("otel.status_code", code);
            })
            .into_iter()
            .map(|span| span.status)
            .collect::<Vec<_>>()
        };

        let error = Status::Error("db down".to_owned());
        assert_eq!(statuses("ERROR"), [error.clone(), error]);
        assert_eq!(statuses("OK"), [Status::Ok, Status::Ok]);
    }

    #[test]
    fn a_new_trace_is_sampled_by_the_trace_sample_ratio() {
        let none = Sample::parse("0").unwrap();
        let every = Sample::parse("1").unwrap();

        assert_eq!(super::root(none).flags, 0);
        assert_eq!(super::root(every).flags, 1);
    }

    #[test]
    fn a_trace_its_caller_did_not_sample_is_not_exported() {
        let spans = exported(|_| {
            let request = tracing::info_span!("request", trace_id = Empty);
            let own = crate::continue_trace(&request, TraceParent::parse(UNSAMPLED));
            assert_eq!(own.flags, 0);
        });

        assert!(spans.is_empty(), "{spans:?}");
    }

    #[test]
    fn an_exported_span_is_scrubbed_like_a_log_line() {
        let mut spans = exported(|_| {
            let span = tracing::info_span!(
                "request",
                otel.name = "GET /reset?token=abcdefabcdefabcdef for jane@example.com",
                otel.status_description = Empty,
                user.password = "hunter2",
                api_key = 7,
                query = "postgres://app:hunter2@db/shop",
                route = "/orders",
                client.address = "10.0.0.7",
            );
            span.record("otel.status_description", "refused: password=hunter2");
        });
        let [span] = spans.as_mut_slice() else {
            panic!("one exported span: {spans:?}");
        };

        scrub_span(span);

        let masked = Some(Value::from("***"));
        assert!(!span.name.contains("abcdef"), "{}", span.name);
        assert!(span.name.ends_with(" for ***"), "{}", span.name);
        assert_eq!(attribute(span, "client.address"), masked);
        assert_eq!(
            span.status,
            Status::Error("refused: password=***".to_owned())
        );
        assert_eq!(attribute(span, "user.password"), masked);
        assert_eq!(attribute(span, "api_key"), masked);
        let Some(Value::Text(query)) = attribute(span, "query") else {
            panic!("the query stays text");
        };
        assert!(!query.contains("hunter2"), "{query}");
        assert_eq!(attribute(span, "route"), Some(Value::from("/orders")));
    }
}
