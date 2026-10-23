mod support;

use order::Order;
use order::actions::PlaceOrder;
use order_contract::{Customer, OrderCancelled, routes};
use serde_json::{Value, json};
use sofie::core::SqlValue;
use sofie::db::Id;
use sofie_identity::testing::TestMember as _;
use sofie_testing::{
    TestApp, TestPrincipal, TestResponse, TestSession, assert_raised, fixture_input, test_app,
};
use support::{PASSWORD, customer};

const ORDERS: &str = "/account/orders";

fn price(minor: i64) -> Value {
    json!({ "minor": minor, "currency": "EUR" })
}

async fn place(app: &TestApp, customer: &TestSession, minor: i64) -> String {
    let order = json!({ "total": price(minor) });
    let placed = app
        .request(&routes::customer::place_order())
        .session(customer)
        .json(&order)
        .await;
    assert_eq!(placed.status, 200, "{}", placed.body);
    placed.json()["id"].as_str().unwrap().to_owned()
}

fn order_id(id: &str) -> Id<Order> {
    Id::from_uuid(id.parse().unwrap())
}

async fn listed(app: &TestApp, customer: &TestSession) -> Value {
    app.request(&routes::customer::list_orders())
        .session(customer)
        .accept_json()
        .await
        .json()
}

#[test_app]
async fn a_customer_places_lists_and_cancels_an_order_once(app: TestApp) {
    let ada = customer(&app, "ada@example.com").await;

    let refused = app
        .post(ORDERS)
        .session(&ada)
        .json(&json!({ "total": "lots", "coupon": "X" }))
        .await;
    assert_eq!(refused.status, 422);
    let body = refused.json();
    let fields: Vec<&str> = body["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|error| error["field"].as_str().unwrap())
        .collect();
    assert!(
        fields.contains(&"total") && fields.contains(&"coupon"),
        "{body}"
    );

    let id = place(&app, &ada, 14_990).await;
    assert_eq!(
        listed(&app, &ada).await,
        json!([{
            "id": id,
            "status": "PENDING",
            "status_to": { "PAID": true, "CANCELLED": true, "EXPIRED": true },
            "total": price(14_990),
        }])
    );

    let cancel = routes::customer::cancel_order(order_id(&id));
    let cancelled = app.request(&cancel).session(&ada).accept_json().await;
    assert_eq!(cancelled.status, 200, "{}", cancelled.body);
    assert_eq!(cancelled.json()["version"], 2);

    let again = app.request(&cancel).session(&ada).accept_json().await;
    assert_eq!(again.status, 422);
    assert_eq!(
        again.json()["errors"],
        json!([{
            "field": "order_status",
            "key": "validation.move",
            "arguments": { "from": "CANCELLED", "to": "CANCELLED" },
            "message": "cannot move from CANCELLED to CANCELLED",
        }])
    );

    let unknown = format!("{ORDERS}/0190a7d2-0000-7000-8000-000000000000/cancel");
    let missing = app.post(&unknown).session(&ada).accept_json().await;
    assert_eq!(missing.status, 404);
}

#[test_app]
async fn a_customer_cancels_their_own_order(app: TestApp) {
    let customer = TestPrincipal::<Customer>::member(&app).await;
    let order = app
        .as_(&customer)
        .request(&routes::customer::place_order())
        .json(&fixture_input::<PlaceOrder>(&app))
        .await
        .record_id();

    let cancel = routes::customer::cancel_order(Id::<Order>::from_uuid(order));
    let long = json!({ "cancel_reason": "x".repeat(201) });
    let refused = app.as_(&customer).request(&cancel).json(&long).await;
    assert_eq!(refused.status, 422, "{}", refused.body);
    assert_eq!(refused.json()["errors"][0]["field"], "cancel_reason");

    let reason = json!({ "cancel_reason": "found it cheaper" });
    app.as_(&customer)
        .request(&cancel)
        .json(&reason)
        .await
        .assert_ok();

    let cancelled = assert_raised::<OrderCancelled>(&app);
    assert_eq!(cancelled[0].order, order);
    assert_eq!(cancelled[0].reason.as_deref(), Some("found it cheaper"));
}

#[test_app]
async fn another_customers_order_is_not_found_and_not_listed(app: TestApp) {
    let ada = customer(&app, "ada@example.com").await;
    let grace = customer(&app, "grace@example.com").await;
    let adas = place(&app, &ada, 1_000).await;
    let graces = place(&app, &grace, 2_000).await;

    let reached = app
        .request(&routes::customer::cancel_order(order_id(&adas)))
        .session(&grace)
        .accept_json()
        .await;

    assert_eq!(reached.status, 404, "{}", reached.body);
    assert_eq!(listed(&app, &grace).await[0]["id"], graces);
    assert_eq!(listed(&app, &grace).await.as_array().unwrap().len(), 1);
    assert_eq!(listed(&app, &ada).await[0]["status"], "PENDING");
}

#[test_app]
async fn without_a_session_the_area_sends_each_caller_to_sign_in(app: TestApp) {
    let page = app.get(ORDERS).await;
    let fragment = app.get(ORDERS).htmx("orders").await;
    let api = app.get(ORDERS).accept_json().await;
    let write = app.post(ORDERS).json(&json!({})).await;

    assert_eq!(page.status, 303);
    assert_eq!(page.header("location"), "/sign-in?next=/account/orders");
    assert_eq!(fragment.status, 401);
    assert_eq!(
        fragment.header("hx-redirect"),
        "/sign-in?next=/account/orders"
    );
    assert_eq!(api.status, 401);
    assert_eq!(api.json()["error"], "Sign in to continue.");
    assert_eq!(write.status, 401);
}

#[test_app]
async fn signing_in_issues_a_host_only_secure_cookie_and_signing_out_clears_it(app: TestApp) {
    let ada = customer(&app, "ada@example.com").await;
    let credentials = [("email", "ada@example.com"), ("password", PASSWORD)];

    let signed_in = app.post("/sign-in").form(&credentials).await;
    let signed_out = app.post("/account/sign-out").session(&ada).form(&[]).await;

    assert_eq!(signed_in.status, 303, "{}", signed_in.body);
    assert_eq!(signed_in.header("location"), "/account/orders");
    let cookie = signed_in.header("set-cookie");
    assert!(
        cookie.starts_with("__Host-shop-session="),
        "a session cookie"
    );
    for attribute in [
        "; Path=/",
        "; Secure",
        "; HttpOnly",
        "; SameSite=Lax",
        "; Max-Age=2592000",
    ] {
        assert!(cookie.contains(attribute), "{attribute}");
    }
    assert!(!cookie.contains("Domain"));
    assert_eq!(signed_out.status, 303);
    assert_eq!(signed_out.header("location"), "/sign-in");
    assert!(signed_out.header("set-cookie").contains("Max-Age=0"));
    assert!(signed_out.session().is_none());
}

#[test_app]
async fn an_htmx_sign_in_navigates_to_the_area(app: TestApp) {
    customer(&app, "ada@example.com").await;

    let signed_in = app
        .post("/sign-in")
        .form(&[("email", "ada@example.com"), ("password", PASSWORD)])
        .htmx("sign-in")
        .trigger("sign-in")
        .await;

    assert_eq!(signed_in.status, 204, "{}", signed_in.body);
    assert_eq!(signed_in.header("hx-redirect"), "/account/orders");
    assert!(signed_in.session().is_some());
}

#[test_app]
async fn a_wrong_password_and_an_unknown_email_are_refused_alike(app: TestApp) {
    customer(&app, "ada@example.com").await;

    let wrong = json!({ "email": "ada@example.com", "password": "not the password" });
    let unknown = json!({ "email": "eve@example.com", "password": PASSWORD });
    let refusals = [
        app.post("/sign-in").json(&wrong).await,
        app.post("/sign-in").json(&unknown).await,
    ];

    for refused in refusals {
        assert_eq!(refused.status, 422);
        assert_eq!(
            refused.json()["error"],
            "The email or password is not right."
        );
        assert!(refused.session().is_none());
    }
}

#[test_app]
async fn a_taken_email_signs_up_as_a_new_one_would_and_a_short_password_is_refused(app: TestApp) {
    customer(&app, "ada@example.com").await;

    let taken = json!({ "email": "Ada@Example.com", "password": "another passphrase" });
    let new = json!({ "email": "linus@example.com", "password": PASSWORD });
    let short = json!({ "email": "grace@example.com", "password": "too short" });
    let again = app.post("/sign-up").json(&taken).await;
    let fresh = app.post("/sign-up").json(&new).await;
    let weak = app.post("/sign-up").json(&short).await;

    assert_eq!((again.status, &again.body), (fresh.status, &fresh.body));
    assert_eq!(again.status, 204);
    assert!(again.session().is_none());
    assert_eq!(
        app.post("/sign-in").json(&taken).await.status,
        422,
        "the account keeps its password"
    );
    assert_eq!(weak.status, 422);
    assert_eq!(weak.json()["errors"][0]["field"], "password");
    assert!(!weak.body.contains("too short"), "{}", weak.body);
}

#[test_app]
async fn the_sign_in_and_sign_up_pages_render_their_forms(app: TestApp) {
    let sign_in = app.get("/sign-in").await.body;
    let sign_up = app.get("/sign-up").await.body;

    assert!(sign_in.contains("<title>Sign in</title>"), "{sign_in}");
    assert!(sign_in.contains("action=\"/sign-in\""), "{sign_in}");
    assert!(
        sign_in.contains("<input type=\"email\" autocomplete=\"email\" id=\"sign-in-email\""),
        "{sign_in}"
    );
    assert!(
        sign_in.contains("type=\"password\" id=\"sign-in-password\""),
        "{sign_in}"
    );
    assert!(sign_in.contains("href=\"/sign-up\""), "{sign_in}");
    assert!(sign_up.contains("action=\"/sign-up\""), "{sign_up}");
}

#[test_app]
async fn a_cross_site_post_with_a_session_is_refused_and_changes_nothing(app: TestApp) {
    let ada = customer(&app, "ada@example.com").await;
    let cancel = format!("{ORDERS}/{}/cancel", place(&app, &ada, 4_200).await);

    let cross_site = app
        .post(&cancel)
        .session(&ada)
        .header("sec-fetch-site", "cross-site")
        .await;

    assert_eq!(cross_site.status, 403, "{}", cross_site.body);
    assert_eq!(listed(&app, &ada).await[0]["status"], "PENDING");
}

#[test_app]
async fn the_orders_endpoint_answers_a_page_a_fragment_or_json(app: TestApp) {
    let ada = customer(&app, "ada@example.com").await;
    place(&app, &ada, 2_500).await;

    assert_eq!(listed(&app, &ada).await[0]["status"], "PENDING");

    let page = app
        .get(ORDERS)
        .session(&ada)
        .header("accept", "text/html")
        .await;
    assert_eq!(page.header("content-type"), "text/html; charset=utf-8");
    let nonce = page
        .header("content-security-policy")
        .split("'nonce-")
        .nth(1)
        .and_then(|rest| rest.split('\'').next())
        .unwrap();
    assert!(
        page.body.contains(&format!("nonce=\"{nonce}\"")),
        "{}",
        page.body
    );
    assert!(page.body.contains("PENDING"), "{}", page.body);

    let fragment = app.get(ORDERS).session(&ada).htmx("orders").await.body;
    assert!(fragment.contains("id=\"orders\""), "{fragment}");
    assert!(fragment.contains("PENDING"), "{fragment}");
    assert!(!fragment.to_lowercase().contains("<html"), "{fragment}");
}

#[test_app]
async fn the_page_cancels_an_order_and_swaps_in_the_refreshed_table(app: TestApp) {
    let ada = customer(&app, "ada@example.com").await;
    let empty = app.get(ORDERS).session(&ada).await.body;
    assert!(empty.contains("No entries"), "{empty}");
    let id = place(&app, &ada, 4_200).await;
    let cancel = format!("{ORDERS}/{id}/cancel");

    let page = app.get(ORDERS).session(&ada).await.body;
    assert!(page.contains("<title>Orders</title>"), "{page}");
    assert!(
        page.contains(
            "name=\"total.amount\" aria-describedby=\"place-order-total-error\" required>"
        ),
        "the place-order form takes its fields from `PlaceOrder`: {page}"
    );
    assert!(
        page.contains("<div id=\"sofie-notice\" role=\"status\" aria-live=\"polite\"></div>"),
        "{page}"
    );
    assert!(
        page.contains(&format!("hx-post=\"{cancel}\" hx-target=\"#orders\"")),
        "{page}"
    );

    let refreshed = app.post(&cancel).session(&ada).htmx("orders").await;
    let fragment = &refreshed.body;
    assert_eq!(refreshed.status, 200, "{fragment}");
    assert_eq!(refreshed.header("content-type"), "text/html; charset=utf-8");
    assert!(fragment.starts_with("<table id=\"orders\">"), "{fragment}");
    assert!(fragment.contains(&id), "{fragment}");
    assert!(fragment.contains("<td>CANCELLED</td>"), "{fragment}");
    assert!(!fragment.contains("PENDING"), "{fragment}");
    assert!(fragment.contains("42.00 EUR"), "{fragment}");
    assert!(
        !fragment.contains("hx-post"),
        "a cancelled order offers no cancel: {fragment}"
    );
}

fn assert_notice(answer: &TestResponse) {
    assert_eq!(answer.header("hx-retarget"), "#sofie-notice");
    assert_eq!(answer.header("hx-reswap"), "innerHTML");
}

#[test_app]
async fn a_failed_htmx_action_shows_its_message_in_the_notice_region(app: TestApp) {
    let ada = customer(&app, "ada@example.com").await;
    let cancel = format!("{ORDERS}/{}/cancel", place(&app, &ada, 4_200).await);
    let cancelled = app.post(&cancel).session(&ada).accept_json().await;
    assert_eq!(cancelled.status, 200);

    let notice = app.post(&cancel).session(&ada).htmx("orders").await;

    assert_eq!(notice.status, 422, "{}", notice.body);
    assert_notice(&notice);
    assert_eq!(
        notice.body,
        "<p>Order status: cannot move from CANCELLED to CANCELLED</p>"
    );
}

const PLACE_ORDER: [(&str, &str); 2] = [("total.amount", "12.50"), ("total.currency", "EUR")];

const MISTYPED_ORDER: [(&str, &str); 2] = [("total.amount", "1.234"), ("total.currency", "EUR")];

#[test_app]
async fn an_order_is_sent_to_the_email_of_the_account_that_placed_it(app: TestApp) {
    let ada = customer(&app, "ada@example.com").await;

    let page = app.get(ORDERS).session(&ada).await.body;
    let id = place(&app, &ada, 1_000).await;

    assert!(!page.contains("name=\"email\""), "{page}");
    let emails = app
        .query(
            "SELECT email FROM \"order\".\"order\" WHERE id = $1",
            &[SqlValue::Uuid(id.parse().unwrap())],
        )
        .await;
    assert_eq!(
        emails,
        vec![vec![SqlValue::Text("ada@example.com".to_owned())]]
    );
}

#[test_app]
async fn the_place_order_form_refreshes_the_table_and_comes_back_empty(app: TestApp) {
    let ada = customer(&app, "ada@example.com").await;

    let placed = app
        .post(ORDERS)
        .session(&ada)
        .form(&PLACE_ORDER)
        .htmx("orders")
        .trigger("place-order")
        .await;

    let html = &placed.body;
    assert_eq!(placed.status, 200, "{html}");
    assert!(html.starts_with("<table id=\"orders\">"), "{html}");
    assert!(html.contains("<td>PENDING</td>"), "{html}");
    assert!(html.contains("12.50 EUR"), "{html}");
    let form = &html[html.find("<form").unwrap()..];
    assert!(
        form.starts_with("<form hx-swap-oob=\"true\" id=\"place-order\""),
        "{html}"
    );
    assert!(
        form.contains("id=\"place-order-total-error\" aria-live=\"polite\"></p>"),
        "the total's error is empty: {html}"
    );
    assert!(!form.contains("value=\"12.50\""), "{html}");
}

#[test_app]
async fn a_mistyped_order_puts_each_message_under_its_field(app: TestApp) {
    let ada = customer(&app, "ada@example.com").await;

    let refused = app
        .post(ORDERS)
        .session(&ada)
        .form(&MISTYPED_ORDER)
        .htmx("orders")
        .trigger("place-order")
        .await;

    assert_eq!(refused.status, 422, "{}", refused.body);
    assert_notice(&refused);
    assert_eq!(
        refused.body,
        "<p>Please correct the highlighted field.</p>\
<div hx-swap-oob=\"innerHTML:#place-order .sofie-field-error\"></div>\
<div hx-swap-oob=\"innerHTML:#place-order-total-error\">EUR amounts have at most 2 decimals</div>"
    );
    assert_eq!(listed(&app, &ada).await, json!([]));
}

#[test_app]
async fn without_javascript_the_form_redirects_or_shows_the_page_again(app: TestApp) {
    let ada = customer(&app, "ada@example.com").await;

    let placed = app.post(ORDERS).session(&ada).form(&PLACE_ORDER).await;
    let refused = app.post(ORDERS).session(&ada).form(&MISTYPED_ORDER).await;

    assert_eq!(placed.status, 303, "{}", placed.body);
    assert_eq!(placed.header("location"), ORDERS);
    assert_eq!(refused.status, 422);
    let page = &refused.body;
    assert!(page.starts_with("<!doctype html>"), "{page}");
    assert!(
        page.contains(
            "aria-live=\"polite\"><p>Total: EUR amounts have at most 2 decimals</p></div>"
        ),
        "{page}"
    );
    assert!(
        page.contains("<td>PENDING</td>"),
        "the placed order is listed: {page}"
    );
}

#[test_app]
async fn a_zero_total_is_refused_by_the_factory(app: TestApp) {
    let ada = customer(&app, "ada@example.com").await;

    let refused = app
        .post(ORDERS)
        .session(&ada)
        .json(&json!({ "total": price(0) }))
        .await;

    assert_eq!(refused.status, 422);
    assert_eq!(refused.json()["errors"][0]["field"], "total");
}

#[test]
fn a_route_builder_fills_its_path_with_the_typed_id() {
    let cancel = routes::customer::cancel_order(order_id("0190a7d2-0000-7000-8000-000000000001"));

    assert_eq!(*cancel.method(), sofie::http::Method::POST);
    assert_eq!(
        cancel.path(),
        "/account/orders/0190a7d2-0000-7000-8000-000000000001/cancel"
    );
}
