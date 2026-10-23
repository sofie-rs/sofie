use std::collections::BTreeMap;

use order::Order;
use order_contract::routes;
use serde_json::json;
use sofie::db::Id;
use sofie::postgres::Postgres;
use sofie::telemetry::{Capture, CapturedSpan};
use sofie_testing::{TestApp, TestDatabase, test_app};
use tracing::{Instrument, Level};

mod support;

const INCOMING_TRACE: &str = "4bf92f3577b34da6a3ce929d0e0e4736";
const EMAIL: &str = "grace@example.com";

fn every_value(capture: &Capture) -> Vec<String> {
    let spans = capture.spans().into_iter().map(|span| span.fields);
    let events = capture.events().into_iter().map(|event| event.fields);
    spans
        .chain(events)
        .flat_map(BTreeMap::into_values)
        .collect()
}

fn the_span<'s>(spans: &'s [CapturedSpan], name: &str, otel_name: &str) -> &'s CapturedSpan {
    let mut found = spans
        .iter()
        .filter(|span| span.name == name && span.fields["otel.name"] == otel_name);
    let span = found
        .next()
        .unwrap_or_else(|| panic!("no {name} span {otel_name}"));
    assert!(found.next().is_none(), "several {name} spans {otel_name}");
    span
}

fn descends_from(spans: &[CapturedSpan], span: &CapturedSpan, ancestor: u64) -> bool {
    let mut parent = span.parent;
    while let Some(parent_id) = parent {
        if parent_id == ancestor {
            return true;
        }
        let Some(found) = spans.iter().find(|span| span.id == parent_id) else {
            return false;
        };
        parent = found.parent;
    }
    false
}

#[test_app]
async fn a_request_and_the_listener_it_starts_share_one_trace_without_personal_data(app: TestApp) {
    // One thread runs the server, the client and the worker, so the capture sees all of them.
    let telemetry = sofie::telemetry::capture();
    let credentials = json!({ "email": EMAIL, "password": support::PASSWORD });
    app.post("/sign-up").json(&credentials).await;
    let signed_in = app.post("/sign-in").json(&credentials).await;
    let customer = signed_in.session().unwrap();
    let cookie = signed_in.header("set-cookie").split(';').next().unwrap();
    let sealed = cookie.split_once('=').unwrap().1.to_owned();
    let incoming = format!("00-{INCOMING_TRACE}-00f067aa0ba902b7-01");
    let order = json!({ "total": { "minor": 1_000, "currency": "EUR" } });

    let placed = app
        .request(&routes::customer::place_order())
        .session(&customer)
        .json(&order)
        .header("traceparent", &incoming)
        .await;
    let id = placed.json()["id"].as_str().unwrap().to_owned();
    let cancelled = app
        .request(&routes::customer::cancel_order(Id::<Order>::from_uuid(
            id.parse().unwrap(),
        )))
        .session(&customer)
        .accept_json()
        .await;
    app.drain().await;

    assert_eq!(placed.status, 200, "{}", placed.body);
    assert_eq!(cancelled.status, 200, "{}", cancelled.body);
    // The app trusts no proxy, so a client's traceparent starts no trace of its own.
    let traceparent = placed.header("traceparent");
    let trace_id = traceparent.split('-').nth(1).unwrap();
    assert_ne!(trace_id, INCOMING_TRACE, "{traceparent}");
    let spans = telemetry.spans();
    let place = the_span(&spans, "request", "POST order.customer.place_order");
    assert_eq!(place.fields["trace_id"], trace_id);
    assert_eq!(place.fields["http.route"], "/account/orders");
    assert_eq!(place.fields["area"], "customer");
    assert_eq!(place.fields["module"], "order");
    assert_eq!(place.fields["http.response.status_code"], "200");
    let cancel = the_span(&spans, "request", "POST order.customer.cancel_order");

    let handler = the_span(&spans, "handler", "order::actions::place_order");
    assert_eq!(handler.parent, Some(place.id));
    assert_eq!(handler.fields["module"], "order");
    let commit = spans
        .iter()
        .find(|span| span.name == "commit" && span.parent == Some(handler.id))
        .expect("the handler's commit");
    assert_eq!(commit.fields["db.statements"], "2");
    let insert = spans
        .iter()
        .find(|span| {
            span.name == "statement"
                && span.fields["db.query.text"].starts_with("INSERT INTO \"order\".\"order\"")
        })
        .expect("the order's insert");
    assert!(descends_from(&spans, insert, commit.id));
    assert_eq!(insert.fields["db.rows"], "1");

    let placement = the_span(
        &spans,
        "message",
        "listener order.listeners.audit_placement",
    );
    let cancellation = the_span(
        &spans,
        "message",
        "listener order.listeners.audit_cancellation",
    );
    assert_eq!(placement.fields["trace_id"], trace_id);
    assert_eq!(placement.fields["module"], "order");
    assert_eq!(cancellation.fields["trace_id"], cancel.fields["trace_id"]);
    assert_ne!(cancel.fields["trace_id"], trace_id);

    for value in every_value(&telemetry) {
        assert!(!value.contains(EMAIL), "{value}");
        assert!(!value.contains(&id), "{value}");
        assert!(
            !value.contains(support::PASSWORD),
            "a password was recorded"
        );
        assert!(!value.contains(&sealed), "a session was recorded");
    }
}

// Only a database, which `#[test_app]` would build with a whole app around it.
#[tokio::test]
async fn a_slow_statement_is_logged_at_warning_without_its_parameters() {
    let telemetry = sofie::telemetry::capture();
    let database = TestDatabase::create().await;
    let postgres = Postgres::connect(database.url()).unwrap();
    let pause = sofie::core::SqlValue::Float(0.31);

    sofie::db::query_values(
        &postgres,
        "SELECT 1 FROM pg_sleep($1)",
        std::slice::from_ref(&pause),
    )
    .instrument(tracing::info_span!("work"))
    .await
    .unwrap();

    let events = telemetry.events();
    let slow = events
        .iter()
        .find(|event| event.fields["message"] == "slow statement")
        .expect("the slow statement is logged");
    assert_eq!(slow.level, Level::WARN);
    let slow = &slow.fields;
    assert_eq!(slow["db.query.text"], "SELECT 1 FROM pg_sleep($1)");
    assert!(slow["duration_ms"].parse::<u64>().unwrap() >= 300);
    assert!(
        every_value(&telemetry)
            .iter()
            .all(|value| !value.contains("0.31"))
    );
}

#[test_app]
async fn a_refused_cross_site_request_is_logged_at_warning_without_its_headers(app: TestApp) {
    let telemetry = sofie::telemetry::capture();
    let customer = support::customer(&app, EMAIL).await;

    let refused = app
        .post("/account/orders/0190a7d2-0000-7000-8000-000000000000/cancel")
        .session(&customer)
        .header("sec-fetch-site", "cross-site")
        .header("origin", "https://evil.test")
        .await;

    assert_eq!(refused.status, 403, "{}", refused.body);
    let events = telemetry.events();
    let logged = events
        .iter()
        .find(|event| event.fields["message"] == "refused a cross-site request")
        .expect("the refusal is logged");
    assert_eq!(logged.level, Level::WARN);
    let logged = &logged.fields;
    assert_eq!(logged["http.request.method"], "POST");
    assert_eq!(logged["http.route"], "/account/orders/{order}/cancel");
    assert_eq!(logged["csrf.check"], "sec-fetch-site");
    for value in every_value(&telemetry) {
        assert!(!value.contains("evil.test"), "{value}");
        assert_ne!(value, "cross-site");
    }
}
