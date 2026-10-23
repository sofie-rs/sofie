use loco_rs::testing::prelude::*;
use loco_shop::app::App;
use serde_json::{json, Value};
use serial_test::serial;

use super::prepare_data;

#[tokio::test]
#[serial]
async fn orders_need_a_signed_in_user() {
    request::<App, _, _>(|request, _ctx| async move {
        assert_eq!(request.get("/api/orders").await.status_code(), 401);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn an_order_is_placed_listed_and_cancelled_once() {
    request::<App, _, _>(|request, ctx| async move {
        let user = prepare_data::init_user_login(&request, &ctx).await;
        let (name, value) = prepare_data::auth_header(&user.token);

        let order = json!({ "email": "buyer@example.com", "amount": "149.90", "currency": "EUR" });
        let placed = request
            .post("/api/orders")
            .add_header(name.clone(), value.clone())
            .json(&order)
            .await;
        assert_eq!(placed.status_code(), 200, "{}", placed.text());
        let placed: Value = placed.json();
        assert_eq!(
            placed["total"],
            json!({ "minor": 14990, "currency": "EUR" })
        );

        let listed: Value = request
            .get("/api/orders")
            .add_header(name.clone(), value.clone())
            .await
            .json();
        assert_eq!(listed[0]["status"], "PENDING");

        let cancel = format!("/api/orders/{}/cancel", placed["id"]);
        let cancelled = request
            .post(&cancel)
            .add_header(name.clone(), value.clone())
            .await;
        assert_eq!(cancelled.status_code(), 200);
        let again = request.post(&cancel).add_header(name, value).await;
        assert_eq!(again.status_code(), 400);
    })
    .await;
}
