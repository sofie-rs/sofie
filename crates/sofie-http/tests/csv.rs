#![cfg(feature = "csv")]

use serde_json::{Value, json};
use sofie_core::{Error, Result};
use sofie_db::{Db, Input, Outcome, ReadOnly, Reply};
use sofie_http::{App, CsvRows, Media, Routes, Web};
use sofie_testing::{FakeDriver, TestServer};

struct Listed(Value);

impl Outcome for Listed {
    fn reply(self) -> Result<Reply> {
        Ok(Reply::Json(self.0))
    }
}

impl ReadOnly for Listed {}

struct NewItem {
    name: String,
    count: i64,
}

impl Input for NewItem {
    fn from_json(value: Value) -> Result<Self> {
        let name = value["name"].as_str().unwrap_or_default().to_owned();
        if name.is_empty() {
            return Err(Error::invalid("name", "is required"));
        }
        let count = value["count"]
            .as_str()
            .and_then(|count| count.parse().ok())
            .ok_or_else(|| Error::invalid("count", "is not a whole number"))?;
        Ok(Self { name, count })
    }
}

async fn items() -> Result<Listed> {
    Ok(Listed(json!([
        {"name": "Scarf", "count": 2},
        {"name": "=cmd", "count": -1},
    ])))
}

async fn import(rows: CsvRows<NewItem>) -> Result<Listed> {
    let total: i64 = rows.0.iter().map(|item| item.count).sum();
    let names: Vec<&str> = rows.0.iter().map(|item| item.name.as_str()).collect();
    Ok(Listed(json!({ "names": names, "total": total })))
}

fn routes(routes: &mut Routes<Web>) {
    routes.get("/items.csv", items).document(Media::Csv);
    routes.post("/items/import", import);
}

async fn server() -> TestServer {
    TestServer::start(App::new(Db::new(FakeDriver::default())).area("shop", routes)).await
}

#[tokio::test]
async fn a_list_is_exported_as_csv() {
    let server = server().await;

    let exported = server.get("/items.csv").await;

    exported.assert_status(200);
    assert_eq!(exported.header("content-type"), "text/csv; charset=utf-8");
    // A JSON object's keys come sorted; a read model keeps its fields' order.
    assert_eq!(exported.body, "count,name\r\n2,Scarf\r\n-1,\"'=cmd\"\r\n");
}

#[tokio::test]
async fn rows_are_imported_as_inputs_and_a_refused_row_names_its_number() {
    let server = server().await;

    let imported = server
        .post("/items/import")
        .header("content-type", "text/csv")
        .body("name,count\r\nScarf,2\r\n\"Hat, red\",3\r\n")
        .await;
    let refused = server
        .post("/items/import")
        .header("content-type", "text/csv")
        .body("name,count\nScarf,two\n,1\n")
        .await;

    imported.assert_ok();
    assert_eq!(
        imported.json(),
        json!({ "names": ["Scarf", "Hat, red"], "total": 5 })
    );
    refused.assert_status(422);
    assert!(refused.body.contains("2.count"), "{}", refused.body);
    assert!(refused.body.contains("3.name"), "{}", refused.body);
}
