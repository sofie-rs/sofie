//! Scaffolded by `sofie scaffold tests` at 2026-10-11 06:40:09 UTC for module `order`,
//! and yours from now on: a later run writes a new file for facts that
//! are new or changed, and names the tests here whose fact changed.
//!
//! fact `value Instant`: `instant_parses_its_samples_and_round_trips`
//! fact `value Email`: `email_parses_its_samples_and_round_trips`
//! fact `value Money`: `money_parses_its_samples_and_round_trips`
//! fact `transitions OrderStatus PENDING>PAID,CANCELLED,EXPIRED PAID>SHIPPED,REFUNDED`: `order_status_moves_only_along_its_transitions`
//! fact `store Order`: `order_loads_as_saved_and_a_stale_save_conflicts`
//! fact `route order.customer.list_orders refuses a caller without a principal`: `list_orders_refuses_a_caller_without_a_principal`
//! fact `route order.customer.list_orders succeeds`: `list_orders_succeeds`
//! fact `screen order/customer/orders renders`: `orders_screen_renders_with_fixtures`
//! fact `route order.customer.place_order refuses a caller without a principal`: `place_order_refuses_a_caller_without_a_principal`
//! fact `route order.customer.place_order succeeds`: `place_order_succeeds`
//! fact `route order.customer.cancel_order refuses a caller without a principal`: `cancel_order_refuses_a_caller_without_a_principal`
//! fact `route order.customer.cancel_order hides another owner's order`: `cancel_order_hides_another_owner_s_order`
//! fact `route order.customer.cancel_order succeeds`: `cancel_order_succeeds`

use sofie_identity::testing::TestMember as _;
use sofie_testing::{TestApp, test_app};

#[test_app]
async fn instant_parses_its_samples_and_round_trips(app: TestApp) {
    sofie_testing::assert_value::<sofie::core::Instant>(&app).await;
}

#[test_app]
async fn email_parses_its_samples_and_round_trips(app: TestApp) {
    sofie_testing::assert_value::<sofie::values::Email>(&app).await;
}

#[test_app]
async fn money_parses_its_samples_and_round_trips(app: TestApp) {
    sofie_testing::assert_value::<sofie::values::Money>(&app).await;
}

#[test]
fn order_status_moves_only_along_its_transitions() {
    sofie_testing::assert_moves::<order::OrderStatus>(&[
        ("PENDING", &["PAID", "CANCELLED", "EXPIRED"]),
        ("PAID", &["SHIPPED", "REFUNDED"]),
    ]);
}

#[test_app]
async fn order_loads_as_saved_and_a_stale_save_conflicts(app: TestApp) {
    let member = sofie_testing::TestPrincipal::<order_contract::area::Customer>::member(&app).await;
    let first = app
        .as_(&member)
        .request(&order_contract::routes::customer::place_order())
        .json(&sofie_testing::fixture_input::<order::actions::PlaceOrder>(
            &app,
        ))
        .await
        .record_id();
    let second = app
        .as_(&member)
        .request(&order_contract::routes::customer::place_order())
        .json(&sofie_testing::fixture_input::<order::actions::PlaceOrder>(
            &app,
        ))
        .await
        .record_id();
    sofie_testing::assert_store::<order::Order>(
        &app,
        sofie::db::Id::from_uuid(first),
        sofie::db::Id::from_uuid(second),
    )
    .await;
}

#[test_app]
async fn list_orders_refuses_a_caller_without_a_principal(app: TestApp) {
    app.request(&order_contract::routes::customer::list_orders())
        .accept_json()
        .await
        .assert_status(401);
}

#[test_app]
async fn list_orders_succeeds(app: TestApp) {
    let member = sofie_testing::TestPrincipal::<order_contract::area::Customer>::member(&app).await;
    app.as_(&member)
        .request(&order_contract::routes::customer::list_orders())
        .accept_json()
        .await
        .assert_ok();
}

#[test_app]
async fn orders_screen_renders_with_fixtures(app: TestApp) {
    let member = sofie_testing::TestPrincipal::<order_contract::area::Customer>::member(&app).await;
    let _shown_1 = app
        .as_(&member)
        .request(&order_contract::routes::customer::place_order())
        .json(&sofie_testing::fixture_input::<order::actions::PlaceOrder>(
            &app,
        ))
        .await
        .record_id();
    let page = app
        .as_(&member)
        .request(&order_contract::routes::customer::list_orders())
        .header("accept", "text/html")
        .await;
    page.assert_ok();
    assert!(page.body.starts_with("<!doctype html>"), "{}", page.body);
}

#[test_app]
async fn place_order_refuses_a_caller_without_a_principal(app: TestApp) {
    app.request(&order_contract::routes::customer::place_order())
        .accept_json()
        .await
        .assert_status(401);
}

#[test_app]
async fn place_order_succeeds(app: TestApp) {
    let member = sofie_testing::TestPrincipal::<order_contract::area::Customer>::member(&app).await;
    app.as_(&member)
        .request(&order_contract::routes::customer::place_order())
        .json(&sofie_testing::fixture_input::<order::actions::PlaceOrder>(
            &app,
        ))
        .await
        .assert_ok();
}

#[test_app]
async fn cancel_order_refuses_a_caller_without_a_principal(app: TestApp) {
    app.request(&order_contract::routes::customer::cancel_order(
        sofie::db::Id::<order::Order>::from_uuid(
            "00000000-0000-4000-8000-000000000000".parse().unwrap(),
        ),
    ))
    .accept_json()
    .await
    .assert_status(401);
}

#[test_app]
async fn cancel_order_hides_another_owner_s_order(app: TestApp) {
    let member = sofie_testing::TestPrincipal::<order_contract::area::Customer>::member(&app).await;
    let other = sofie_testing::TestPrincipal::<order_contract::area::Customer>::member(&app).await;
    let record = app
        .as_(&other)
        .request(&order_contract::routes::customer::place_order())
        .json(&sofie_testing::fixture_input::<order::actions::PlaceOrder>(
            &app,
        ))
        .await
        .record_id();
    app.as_(&member)
        .request(&order_contract::routes::customer::cancel_order(
            sofie::db::Id::<order::Order>::from_uuid(record),
        ))
        .accept_json()
        .await
        .assert_status(404);
}

#[test_app]
async fn cancel_order_succeeds(app: TestApp) {
    let member = sofie_testing::TestPrincipal::<order_contract::area::Customer>::member(&app).await;
    let record_0 = app
        .as_(&member)
        .request(&order_contract::routes::customer::place_order())
        .json(&sofie_testing::fixture_input::<order::actions::PlaceOrder>(
            &app,
        ))
        .await
        .record_id();
    app.as_(&member)
        .request(&order_contract::routes::customer::cancel_order(
            sofie::db::Id::<order::Order>::from_uuid(record_0),
        ))
        .accept_json()
        .await
        .assert_ok();
}
