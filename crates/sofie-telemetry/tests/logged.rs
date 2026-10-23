#![cfg(all(feature = "capture", feature = "log-fields"))]

use sofie_telemetry::capture;
use sofie_values::{Currency, Email, Money};
use tracing::Level;

struct Order {
    total: Money,
    email: Email,
    lines: i64,
}

fn order() -> Order {
    Order {
        total: Money::new(14_990, Currency::parse("EUR").unwrap()),
        email: Email::parse("ada@example.com").unwrap(),
        lines: 3,
    }
}

#[test]
fn values_are_logged_as_their_members_and_sensitive_ones_masked() {
    let telemetry = capture();
    let order = order();

    sofie_telemetry::info!(order.total, order.email, order.lines, "order placed");

    let events = telemetry.events();
    let [placed] = events.as_slice() else {
        panic!("one event: {events:?}");
    };
    assert_eq!(placed.level, Level::INFO);
    assert_eq!(placed.target, module_path!());
    assert_eq!(placed.fields["message"], "order placed");
    assert_eq!(placed.fields["total.minor"], "14990");
    assert_eq!(placed.fields["total.currency"], "EUR");
    assert_eq!(placed.fields["lines"], "3");
    assert_eq!(placed.fields["email"], "a***@example.com");
}

#[test]
fn a_level_below_the_filter_gathers_nothing_and_a_message_alone_is_logged() {
    let telemetry = capture();
    let order = order();

    sofie_telemetry::warn!("nothing to say");
    sofie_telemetry::error!(order.lines, "failed");

    let events = telemetry.events();
    assert_eq!(events.len(), 2, "{events:?}");
    assert_eq!(events[0].fields["message"], "nothing to say");
    assert_eq!(events[1].fields["lines"], "3");
}

#[cfg(feature = "product-events")]
#[test]
fn a_product_event_is_named_by_its_path_and_carries_its_values() {
    let telemetry = capture();
    let order = order();

    tracing::info!("not a product event");
    sofie_telemetry::product_event!(order.placed, order.total);
    sofie_telemetry::product_event!(checkout.started);

    let events = telemetry.product_events();
    assert_eq!(events.len(), 2, "{events:?}");
    assert_eq!(events[0].target, sofie_telemetry::PRODUCT_TARGET);
    assert_eq!(events[0].fields["product.event"], "order.placed");
    assert_eq!(events[0].fields["total.minor"], "14990");
    assert_eq!(events[1].fields["product.event"], "checkout.started");
}

struct Payer {
    name: String,
    tax_number: String,
}

impl sofie_values::SensitiveFields for Payer {
    const SENSITIVE_FIELDS: &'static [&'static str] = &["tax_number"];
}

struct Checkout<'a> {
    payer: &'a Payer,
}

#[test]
fn a_field_its_owner_marks_sensitive_logs_only_a_mask() {
    let telemetry = capture();
    let payer = Payer {
        name: "Ada".to_owned(),
        tax_number: "12345678901".to_owned(),
    };
    let checkout = Checkout { payer: &payer };
    let borrowed = &payer;

    sofie_telemetry::info!(payer.tax_number, payer.name, "paid");
    sofie_telemetry::info!(checkout.payer.tax_number, borrowed.tax_number, "paid");

    let events = telemetry.events();
    assert_eq!(events.len(), 2, "{events:?}");
    assert_eq!(events[0].fields["tax_number"], "***");
    assert_eq!(events[0].fields["name"], "Ada");
    assert_eq!(events[1].fields["tax_number"], "***");
}
