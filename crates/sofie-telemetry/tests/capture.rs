#![cfg(feature = "capture")]

use std::fmt;

use sofie_telemetry::{TraceParent, capture, continue_trace, current_traceparent};
use tracing::Level;
use tracing::field::Empty;

const INCOMING: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

struct FailingDebug;

impl fmt::Debug for FailingDebug {
    fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
        Err(fmt::Error)
    }
}

#[test]
fn spans_keep_their_parent_and_their_latest_fields() {
    let telemetry = capture();

    let request = tracing::info_span!("request", route = "/orders", status = Empty);
    request.in_scope(|| {
        let handler = tracing::info_span!("handler", attempt = 2_u64);
        handler.in_scope(|| tracing::warn!(answer = 42, "slow"));
    });
    request.record("status", 200);
    drop(request);

    let spans = telemetry.spans();
    let [request, handler] = spans.as_slice() else {
        panic!("two spans: {spans:?}");
    };
    assert_eq!(request.name, "request");
    assert_eq!(request.parent, None);
    assert_eq!(request.fields["route"], "/orders");
    assert_eq!(request.fields["status"], "200");
    assert_eq!(handler.parent, Some(request.id));
    assert_eq!(handler.fields["attempt"], "2");
    let events = telemetry.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].level, Level::WARN);
    assert_eq!(events[0].fields["message"], "slow");
    assert_eq!(events[0].fields["answer"], "42");
}

#[test]
fn a_continued_trace_reaches_every_span_under_it() {
    let telemetry = capture();
    let incoming = TraceParent::parse(INCOMING).unwrap();

    let request = tracing::info_span!("request", trace_id = Empty);
    let own = continue_trace(&request, Some(incoming));
    let (inner, below_debug) = request.in_scope(|| {
        let handler = tracing::info_span!("handler");
        let inner = handler.in_scope(current_traceparent).unwrap();
        let below_debug = tracing::debug_span!("internal").in_scope(current_traceparent);
        (inner, below_debug)
    });

    assert_eq!(own.trace_id(), incoming.trace_id());
    assert_ne!(own, incoming);
    assert_eq!(inner.trace_id(), incoming.trace_id());
    assert_ne!(inner, own);
    assert_eq!(below_debug, Some(own));
    assert_eq!(telemetry.spans()[0].fields["trace_id"], incoming.trace_id());
}

#[test]
fn a_span_opened_on_another_thread_keeps_its_parent_and_trace() {
    let telemetry = capture();
    let dispatch = tracing::dispatcher::get_default(Clone::clone);
    let request = tracing::info_span!("request", trace_id = Empty);
    let own = continue_trace(&request, None);

    let (handler, inner) = std::thread::scope(|threads| {
        threads
            .spawn(|| {
                tracing::dispatcher::with_default(&dispatch, || {
                    let handler = tracing::info_span!(parent: &request, "handler");
                    let inner = handler.in_scope(current_traceparent).unwrap();
                    (handler, inner)
                })
            })
            .join()
            .unwrap()
    });
    drop(request);
    drop(handler);

    assert_eq!(inner.trace_id(), own.trace_id());
    let spans = telemetry.spans();
    let [request, handler] =
        ["request", "handler"].map(|name| spans.iter().find(|span| span.name == name).unwrap());
    assert_eq!(handler.parent, Some(request.id));
    assert_eq!(spans.len(), 2, "each span closed once: {spans:?}");
}

#[test]
fn spans_are_listed_in_the_order_they_opened() {
    let telemetry = capture();

    for step in 0..100_u64 {
        drop(tracing::info_span!("step", step));
    }

    let steps: Vec<String> = telemetry
        .spans()
        .into_iter()
        .map(|span| span.fields["step"].clone())
        .collect();
    let expected: Vec<String> = (0..100).map(|step: u64| step.to_string()).collect();
    assert_eq!(steps, expected);
}

#[test]
fn a_new_root_starts_its_own_trace() {
    let _telemetry = capture();

    let first = tracing::info_span!("job", trace_id = Empty);
    let second = tracing::info_span!("job", trace_id = Empty);

    let first = continue_trace(&first, None);
    let second = continue_trace(&second, None);
    assert_ne!(first.trace_id(), second.trace_id());
    assert_eq!(current_traceparent(), None);
}

#[test]
fn a_value_that_cannot_be_formatted_is_marked_not_panicked_on() {
    let telemetry = capture();

    tracing::error!(value = ?FailingDebug, text = "tab\there \"quoted\" 東京");

    let fields = &telemetry.events()[0].fields;
    assert_eq!(fields["value"], "<the value's Debug implementation failed>");
    assert_eq!(fields["text"], "tab\there \"quoted\" 東京");
}

#[test]
fn without_a_subscriber_a_traceparent_is_still_made_up() {
    let incoming = TraceParent::parse(INCOMING).unwrap();

    let own = continue_trace(&tracing::Span::none(), Some(incoming));

    assert_eq!(own.trace_id(), incoming.trace_id());
    assert_eq!(current_traceparent(), None);
}
