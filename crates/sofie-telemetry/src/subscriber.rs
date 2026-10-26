use std::time::SystemTime;

use tracing::level_filters::LevelFilter;
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};
use tracing_core::span::Current;

use crate::context::{Exporter, Trace};
use crate::entered;
use crate::fields::Fields;
use crate::filter::LogFilter;
use crate::line::PendingLine;
use crate::sample::Sample;
use crate::store::{SpanEntry, SpanStore};
use crate::writer::LogWriter;

pub(crate) trait Export {
    fn closed(&self, _: &SpanEntry) {}
}

pub(crate) enum Output {
    Lines {
        filter: LogFilter,
        sample: Sample,
        writer: LogWriter,
    },
    #[cfg(feature = "capture")]
    Recording(std::sync::Arc<crate::capture::Recording>),
}

pub(crate) struct Telemetry {
    pub(crate) spans: SpanStore,
    pub(crate) output: Output,
    exporter: Exporter,
    #[cfg(feature = "development")]
    pub(crate) development: crate::development::Recorder,
}

impl Telemetry {
    pub(crate) fn new(output: Output, exporter: Exporter) -> Self {
        Self {
            spans: SpanStore::new(),
            output,
            exporter,
            #[cfg(feature = "development")]
            development: crate::development::Recorder::default(),
        }
    }

    /// A parent this store does not hold, such as one closed already, is dropped.
    fn parent_of(&self, attributes: &Attributes<'_>) -> Option<Id> {
        self.explicit_or_entered(attributes.is_contextual(), attributes.parent())
            .filter(|parent| self.spans.add_reference(parent))
    }

    fn explicit_or_entered(&self, contextual: bool, explicit: Option<&Id>) -> Option<Id> {
        if contextual {
            return entered::current(self.spans.owner());
        }
        explicit.cloned()
    }

    /// Only the fields are read here, since the event borrows them; the writer's thread formats.
    fn queue_line(&self, writer: &LogWriter, sample: Sample, event: &Event<'_>) {
        let innermost = self.explicit_or_entered(event.is_contextual(), event.parent());
        let metadata = event.metadata();
        let level = *metadata.level();
        if level > Level::WARN
            && sample != Sample::EVERY
            && !is_product(metadata)
            && innermost
                .as_ref()
                .and_then(|id| self.spans.nearest_trace(id))
                .is_some_and(|trace| !sample.keeps(trace.traceparent().trace_id))
        {
            return;
        }
        let (scope, trace) = self.spans.scope(innermost.as_ref());
        let fields = self.event_fields(event, innermost.as_ref());
        writer.send(PendingLine {
            level,
            target: metadata.target(),
            fields,
            scope,
            trace,
            time: SystemTime::now(),
        });
    }

    pub(crate) fn flush(&self) {
        match &self.output {
            Output::Lines { writer, .. } => writer.flush(),
            #[cfg(feature = "capture")]
            Output::Recording(_) => {}
        }
    }

    #[cfg_attr(
        not(any(feature = "capture", feature = "development")),
        expect(unused_variables, reason = "only a capture records which span closed")
    )]
    fn closed(&self, id: u64, entry: SpanEntry) -> Option<Id> {
        self.exporter.closed(&entry);
        #[cfg(feature = "development")]
        if let Some(tree) = self.development.closed(id, &entry) {
            self.print_tree(tree);
        }
        #[cfg(feature = "capture")]
        if let Output::Recording(recording) = &self.output {
            recording.span_closed(crate::capture::CapturedSpan::of(id, &entry));
        }
        entry.parent
    }

    #[cfg_attr(
        not(feature = "product-events"),
        expect(
            unused_variables,
            clippy::unused_self,
            reason = "only product events read their spans"
        )
    )]
    fn event_fields(&self, event: &Event<'_>, innermost: Option<&Id>) -> Fields {
        let mut fields = Fields::default();
        event.record(&mut fields);
        #[cfg(feature = "log-fields")]
        if let Some(logged) = crate::logged::take() {
            fields.merge(logged);
        }
        #[cfg(feature = "product-events")]
        if is_product(event.metadata()) {
            self.add_feature_flags(innermost, &mut fields);
        }
        fields
    }
}

#[cfg(feature = "product-events")]
use crate::product::is_product;

#[cfg(not(feature = "product-events"))]
fn is_product(_: &Metadata<'_>) -> bool {
    false
}

/// Libraries open debug and trace spans for their internals; the framework's are at info.
fn is_traced(metadata: &Metadata<'_>) -> bool {
    metadata.is_span() && *metadata.level() <= Level::INFO
}

impl Subscriber for Telemetry {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        match &self.output {
            Output::Lines { filter, .. } => {
                is_traced(metadata) || filter.allows(metadata) || is_product(metadata)
            }
            #[cfg(feature = "capture")]
            Output::Recording(_) => true,
        }
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        match &self.output {
            Output::Lines { filter, .. } => Some(filter.max_level().max(LevelFilter::INFO)),
            #[cfg(feature = "capture")]
            Output::Recording(_) => Some(LevelFilter::TRACE),
        }
    }

    fn new_span(&self, attributes: &Attributes<'_>) -> Id {
        let parent = self.parent_of(attributes);
        let metadata = attributes.metadata();
        let trace = is_traced(metadata).then(|| {
            let inherited = parent
                .as_ref()
                .and_then(|parent| self.spans.nearest_trace(parent));
            Trace::open(inherited.as_ref())
        });
        let mut fields = Fields::default();
        attributes.record(&mut fields);
        self.spans
            .insert(SpanEntry::new(metadata, parent, fields, trace))
    }

    fn record(&self, id: &Id, values: &Record<'_>) {
        let mut recorded = Fields::default();
        values.record(&mut recorded);
        self.spans.record(id, recorded);
    }

    fn record_follows_from(&self, _: &Id, _: &Id) {}

    fn event(&self, event: &Event<'_>) {
        #[cfg(feature = "product-events")]
        if is_product(event.metadata()) {
            let innermost = self.explicit_or_entered(event.is_contextual(), event.parent());
            self.keep_product(event, innermost.as_ref());
        }
        match &self.output {
            Output::Lines { writer, sample, .. } => self.queue_line(writer, *sample, event),
            #[cfg(feature = "capture")]
            Output::Recording(recording) => {
                let innermost = self.explicit_or_entered(event.is_contextual(), event.parent());
                recording.event(
                    event.metadata(),
                    &self.event_fields(event, innermost.as_ref()),
                );
            }
        }
    }

    fn enter(&self, id: &Id) {
        entered::push(self.spans.owner(), id);
    }

    fn exit(&self, id: &Id) {
        entered::pop(self.spans.owner(), id);
    }

    fn clone_span(&self, id: &Id) -> Id {
        self.spans.add_reference(id);
        id.clone()
    }

    /// Entries are exported outside the store's lock: the exporter may log.
    fn try_close(&self, id: Id) -> bool {
        let Some(entry) = self.spans.release(&id) else {
            return false;
        };
        let mut parent = self.closed(id.into_u64(), entry);
        while let Some(id) = parent {
            parent = self
                .spans
                .release(&id)
                .and_then(|entry| self.closed(id.into_u64(), entry));
        }
        true
    }

    fn current_span(&self) -> Current {
        entered::current(self.spans.owner())
            .and_then(|id| Some(Current::new(id.clone(), self.spans.metadata(&id)?)))
            .unwrap_or_else(Current::none)
    }
}
