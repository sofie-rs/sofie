use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use sofie_core::unpoisoned;
use tracing::span::Id;
use tracing::{Level, Span};

use crate::fields::{FieldValue, Fields, NAME_FIELD, push_formatted};
use crate::line::{PendingLine, push_readable_value};
use crate::scrub::{self, SpanFields};
use crate::store::SpanEntry;
use crate::subscriber::{Output, Telemetry};

/// A runaway trace keeps its first spans.
const SPANS_PER_TRACE: usize = 512;
/// Spans closing after their root, such as a spawned task's, are never collected otherwise.
const OPEN_TRACES: usize = 1024;
const TARGET: &str = "sofie.dev";

struct Closed {
    id: u64,
    parent: Option<u64>,
    name: &'static str,
    fields: Arc<SpanFields>,
    opened: Instant,
    duration: Duration,
}

impl Closed {
    fn of(id: u64, entry: &SpanEntry) -> Self {
        Self {
            id,
            parent: entry.parent.as_ref().map(Id::into_u64),
            name: entry.metadata.name(),
            fields: Arc::clone(&entry.fields),
            opened: entry.opened,
            duration: entry.opened.elapsed(),
        }
    }
}

pub(crate) struct ClosedTree {
    trace_id: u128,
    text: String,
}

#[derive(Default)]
pub(crate) struct Recorder {
    traces: Mutex<HashMap<u128, Vec<Closed>>>,
}

impl Recorder {
    pub(crate) fn closed(&self, id: u64, entry: &SpanEntry) -> Option<ClosedTree> {
        let trace = entry.trace?;
        let trace_id = trace.traceparent().trace_id;
        let closed = Closed::of(id, entry);
        let mut traces = unpoisoned::lock(&self.traces);
        if !trace.is_local_root() {
            if traces.len() >= OPEN_TRACES && !traces.contains_key(&trace_id) {
                traces.clear();
            }
            let spans = traces.entry(trace_id).or_default();
            if spans.len() < SPANS_PER_TRACE {
                spans.push(closed);
            }
            return None;
        }
        let descendants = traces.remove(&trace_id).unwrap_or_default();
        Some(ClosedTree {
            trace_id,
            text: render(&closed, &descendants),
        })
    }

    pub(crate) fn open_tree(&self, id: u64, entry: &SpanEntry) -> Option<String> {
        let trace_id = entry.trace?.traceparent().trace_id;
        let root = Closed {
            parent: None,
            ..Closed::of(id, entry)
        };
        let tree = {
            let traces = unpoisoned::lock(&self.traces);
            render(&root, traces.get(&trace_id).map_or(&[][..], Vec::as_slice))
        };
        Some(scrub::scrubbed_text(&tree).unwrap_or(tree))
    }
}

impl Telemetry {
    pub(crate) fn open_tree(&self, id: &Id) -> Option<String> {
        self.spans
            .with(id, |entry| self.development.open_tree(id.into_u64(), entry))?
    }

    pub(crate) fn print_tree(&self, tree: ClosedTree) {
        if !cfg!(debug_assertions) {
            return;
        }
        match &self.output {
            Output::Lines { writer, .. } => writer.send_unexported(tree_line(tree)),
            #[cfg(feature = "capture")]
            Output::Recording(_) => {}
        }
    }
}

/// `sofie mcp` finds a request's trees by the `trace_id` leading the line.
fn tree_line(tree: ClosedTree) -> PendingLine {
    let mut fields = Fields::default();
    fields.set(
        "trace_id",
        FieldValue::Text(format!("{:032x}", tree.trace_id)),
    );
    fields.set("message", FieldValue::Composed(tree.text));
    PendingLine {
        level: Level::INFO,
        target: TARGET,
        fields,
        scope: Vec::new(),
        trace: None,
        time: SystemTime::now(),
    }
}

#[must_use]
pub fn span_tree(span: &Span) -> Option<String> {
    crate::with_telemetry(span, Telemetry::open_tree)
}

/// A span whose parent was not kept, such as a library's debug span, hangs from the root.
fn render(root: &Closed, descendants: &[Closed]) -> String {
    let mut children: HashMap<u64, Vec<&Closed>> = HashMap::new();
    for span in descendants {
        let known = span
            .parent
            .filter(|parent| descendants.iter().any(|other| other.id == *parent));
        children
            .entry(known.unwrap_or(root.id))
            .or_default()
            .push(span);
    }
    let mut text = String::new();
    push_span(&mut text, root, &children, 0);
    text
}

fn push_span(
    text: &mut String,
    span: &Closed,
    children: &HashMap<u64, Vec<&Closed>>,
    depth: usize,
) {
    if !text.is_empty() {
        text.push('\n');
    }
    let name = match span.fields.iter().find(|(name, _)| *name == NAME_FIELD) {
        Some((_, value)) => format!("{} {value}", span.name),
        None => span.name.to_owned(),
    };
    push_formatted(
        text,
        format_args!(
            "{:indent$}{name} {:.2} ms",
            "",
            span.duration.as_secs_f64() * 1000.0,
            indent = depth * 2
        ),
    );
    for (name, value) in span
        .fields
        .iter()
        .filter(|(name, _)| !name.starts_with("otel."))
    {
        push_formatted(text, format_args!(" {name}="));
        push_readable_value(text, value);
    }
    let mut own = children.get(&span.id).cloned().unwrap_or_default();
    // Span ids name a shard before a count, so only the start time orders siblings.
    own.sort_by_key(|child| child.opened);
    for child in own {
        push_span(text, child, children, depth + 1);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use super::{Closed, ClosedTree, render, tree_line};
    use crate::line::LineFormat;
    use crate::scrub::SpanFields;

    fn span(id: u64, parent: Option<u64>, name: &'static str, opened: Instant) -> Closed {
        Closed {
            id,
            parent,
            name,
            fields: Arc::new(SpanFields::default()),
            opened,
            duration: Duration::from_millis(1),
        }
    }

    #[test]
    fn siblings_are_listed_in_the_order_they_started_whatever_their_ids() {
        let start = Instant::now();
        let root = span(1, None, "request", start);
        let render_span = span(2, Some(1), "render", start + Duration::from_millis(2));
        let handler = span(9, Some(1), "handler", start + Duration::from_millis(1));

        let tree = render(&root, &[render_span, handler]);

        let names: Vec<&str> = tree
            .lines()
            .map(|line| line.split_whitespace().next().unwrap())
            .collect();
        assert_eq!(names, ["request", "handler", "render"], "{tree}");
    }

    #[test]
    fn a_tree_line_leads_with_its_trace_id() {
        let tree = ClosedTree {
            trace_id: 0x4bf9,
            text: "request GET / 1.00 ms\n  handler 0.50 ms".to_owned(),
        };
        let mut line = String::new();

        LineFormat::Readable.render_into(&mut line, &tree_line(tree));

        let (_, rest) = line.split_once(" INFO ").unwrap();
        assert_eq!(
            rest,
            "sofie.dev: trace_id=\"00000000000000000000000000004bf9\" request GET / 1.00 ms\n  \
             handler 0.50 ms\n"
        );
    }
}
