// `development` rereads templates from disk, so it tags nothing.
#![cfg(not(feature = "development"))]

use http::StatusCode;
use serde_json::json;
use sofie_core::{Cast, ColumnReader, Result, SqlValue, Uuid};
use sofie_db::{Aggregate, Context, Db, Field, Meta, Outcome, ReadOnly, Reply, Resolve};
use sofie_http::{App, Routes, Web};
use sofie_testing::{FakeDriver, TestResponse, TestServer};

struct Order {
    meta: Meta,
    status: String,
}

impl Aggregate for Order {
    const NAME: &'static str = "order";
    const MODULE: &'static str = "shop";
    const FIELDS: &'static [Field] = &[Field::new("status", String::COLUMNS, false)];

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.status.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            status: String::decode(row)?,
        })
    }
}

impl<C: Context> Resolve<C> for Order {
    fn resolve(context: &mut C) -> impl Future<Output = Result<Self>> + Send {
        sofie_db::bind::<Self, C>(context)
    }
}

struct Rows(serde_json::Value);

impl Outcome for Rows {
    fn reply(self) -> Result<Reply> {
        Ok(Reply::Json(self.0))
    }
}

impl ReadOnly for Rows {}

/// A GET replies the order as an action would, but never commits it.
struct Shown(Order);

impl Outcome for Shown {
    fn reply(self) -> Result<Reply> {
        self.0.reply()
    }
}

impl ReadOnly for Shown {}

async fn show_order(order: Order) -> Result<Shown> {
    Ok(Shown(order))
}

async fn order_page(order: Order) -> Result<Rows> {
    Ok(Rows(json!([{ "status": order.status }])))
}

/// As a loader that also asked another app, through sofie-client.
async fn quoted_order(order: Order) -> Result<Rows> {
    sofie_db::read_untracked();
    Ok(Rows(json!({ "status": order.status, "quote": 7 })))
}

async fn cancel_order(mut order: Order) -> Result<Order> {
    order.status = String::from("cancelled");
    Ok(order)
}

const ORDER_SCREEN: &str = r#"<Section title="Order">
  <Table id="order" of={data} columns="status" />
</Section>
"#;

fn order_row(id: Uuid, version: i64) -> Vec<SqlValue> {
    vec![
        SqlValue::Uuid(id),
        SqlValue::Int(version),
        SqlValue::Text("open".to_owned()),
    ]
}

async fn server(id: Uuid, version: i64) -> TestServer {
    let driver = FakeDriver::returning(vec![order_row(id, version)]);
    let app = App::new(Db::new(driver))
        .screens(sofie_http::screens_from_sources(&[("shop/web/order", ORDER_SCREEN)]).unwrap())
        .area("shop", |routes: &mut Routes<Web>| {
            routes.get("/orders/{order}", show_order);
            routes.post("/orders/{order}/cancel", cancel_order);
            routes.get("/orders/{order}/quote", quoted_order);
            routes.screen("/pages/{order}", "order", order_page);
        });
    TestServer::start(app).await
}

async fn revalidated(server: &TestServer, path: &str, tag: &str) -> TestResponse {
    server.get(path).header("if-none-match", tag).await
}

fn tag_of(response: &TestResponse) -> &str {
    response.headers["etag"].to_str().unwrap()
}

#[tokio::test]
async fn a_loaders_answer_is_tagged_private_and_revalidated() {
    let id = Uuid::now_v7().unwrap();
    let server = server(id, 1).await;

    let answer = server.get(&format!("/orders/{id}")).accept_json().await;

    assert_eq!(answer.status, StatusCode::OK);
    assert!(tag_of(&answer).starts_with("W/\""), "{}", tag_of(&answer));
    assert_eq!(answer.headers["cache-control"], "private, no-cache");
    let vary: Vec<&str> = answer
        .headers
        .get_all("vary")
        .iter()
        .map(|value| value.to_str().unwrap())
        .collect();
    assert!(
        vary.iter().any(|value| value.contains("Cookie")),
        "{vary:?}"
    );
}

#[tokio::test]
async fn a_held_tag_is_answered_304_and_a_changed_row_answers_anew() {
    let id = Uuid::now_v7().unwrap();
    let path = format!("/orders/{id}");
    let first = server(id, 1).await;
    let changed = server(id, 2).await;
    let tag = tag_of(&first.get(&path).await).to_owned();

    let unchanged = revalidated(&first, &path, &tag).await;
    let after_change = revalidated(&changed, &path, &tag).await;

    assert_eq!(unchanged.status, StatusCode::NOT_MODIFIED);
    assert_eq!(unchanged.body, "");
    assert_eq!(tag_of(&unchanged), tag);
    assert_eq!(unchanged.headers["cache-control"], "private, no-cache");
    assert_eq!(after_change.status, StatusCode::OK);
    assert_ne!(tag_of(&after_change), tag);
}

#[tokio::test]
async fn a_screen_is_answered_304_without_rendering() {
    let id = Uuid::now_v7().unwrap();
    let path = format!("/pages/{id}");
    let server = server(id, 1).await;
    let page = server.get(&path).await;
    let fragment = server.get(&path).htmx("order").await;

    let unchanged = revalidated(&server, &path, tag_of(&page)).await;

    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("open"), "{}", page.body);
    assert_ne!(tag_of(&fragment), tag_of(&page));
    assert_eq!(unchanged.status, StatusCode::NOT_MODIFIED);
    assert!(!unchanged.headers.contains_key("content-security-policy"));
}

#[tokio::test]
async fn actions_and_failures_are_not_tagged() {
    let id = Uuid::now_v7().unwrap();
    let server = server(id, 1).await;

    let action = server
        .post(&format!("/orders/{id}/cancel"))
        .accept_json()
        .header("sec-fetch-site", "same-origin")
        .await;
    let missing = server.get("/orders/not-an-id").accept_json().await;

    assert_eq!(action.status, StatusCode::OK);
    assert!(!action.headers.contains_key("etag"));
    assert_eq!(action.headers["cache-control"], "no-store");
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert!(!missing.headers.contains_key("etag"));
}

#[tokio::test]
async fn an_answer_using_another_app_s_reply_is_not_tagged() {
    let id = Uuid::now_v7().unwrap();
    let server = server(id, 1).await;

    let answer = server
        .get(&format!("/orders/{id}/quote"))
        .accept_json()
        .await;

    assert_eq!(answer.status, StatusCode::OK);
    assert!(answer.headers.get("etag").is_none(), "{:?}", answer.headers);
}
