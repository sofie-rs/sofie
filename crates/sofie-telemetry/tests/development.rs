#![cfg(all(feature = "capture", feature = "development"))]

use sofie_telemetry::development::span_tree;
use sofie_telemetry::{TraceParent, capture, continue_trace};
use tracing::field::Empty;

#[test]
fn the_tree_shows_closed_descendants_indented_under_the_open_root() {
    let _telemetry = capture();
    let request = tracing::info_span!(
        "request",
        otel.name = "GET order.web.show",
        trace_id = Empty
    );
    let _ = continue_trace(&request, None);

    request.in_scope(|| {
        let handler = tracing::info_span!("handler", module = "order");
        handler.in_scope(|| {
            tracing::info_span!("statement", db.query.text = "SELECT 1", db.rows = 1_u64)
                .in_scope(|| ());
        });
    });
    let tree = span_tree(&request).unwrap();

    let lines: Vec<&str> = tree.lines().collect();
    assert_eq!(lines.len(), 3, "{tree}");
    assert!(
        lines[0].starts_with("request GET order.web.show "),
        "{tree}"
    );
    assert!(lines[1].starts_with("  handler "), "{tree}");
    assert!(lines[1].ends_with(" module=\"order\""), "{tree}");
    assert!(lines[2].starts_with("    statement "), "{tree}");
    assert!(
        lines[2].contains(" db.query.text=\"SELECT 1\" db.rows=1"),
        "{tree}"
    );
}

#[test]
fn the_tree_masks_credentials_emails_and_addresses() {
    let _telemetry = capture();
    let request = tracing::info_span!("request", trace_id = Empty);
    let _ = continue_trace(&request, None);
    request.in_scope(|| {
        tracing::info_span!(
            "handler",
            customer = "jane@example.com",
            client.address = "10.0.0.7",
            password = "hunter2"
        )
        .in_scope(|| ());
    });

    let tree = span_tree(&request).unwrap();

    for personal in ["jane@example.com", "10.0.0.7", "hunter2"] {
        assert!(!tree.contains(personal), "{tree}");
    }
    assert!(tree.contains(" customer=\"***\""), "{tree}");
}

#[test]
fn outside_a_traced_span_there_is_no_tree() {
    let _telemetry = capture();

    assert_eq!(span_tree(&tracing::Span::none()), None);
}

#[test]
fn a_request_continuing_a_trace_inside_a_long_lived_span_leaves_nothing_behind() {
    let _telemetry = capture();
    let incoming =
        TraceParent::parse("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01").unwrap();
    let server = tracing::info_span!("server", trace_id = Empty);
    let _ = continue_trace(&server, None);
    let serve = |handler: &'static str| {
        server.in_scope(|| {
            let request = tracing::info_span!("request", trace_id = Empty);
            let _ = continue_trace(&request, Some(incoming));
            request.in_scope(|| tracing::info_span!("handler", name = handler).in_scope(|| ()));
            request
        })
    };

    drop(serve("first"));
    let second = serve("second");
    let tree = span_tree(&second).unwrap();

    assert!(!tree.contains("first"), "{tree}");
    assert!(tree.contains("second"), "{tree}");
}
