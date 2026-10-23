#![cfg(feature = "jobs")]

use http::StatusCode;
use serde_json::json;
use sofie_core::{Result, SqlValue};
use sofie_db::{Db, Step};
use sofie_http::{App, Area, Routes, Web};
use sofie_testing::{FakeDriver, TestServer};

struct Account;

impl sofie_http::AreaPrincipal for Account {
    const ACCESS: sofie_http::Access = sofie_http::Access::Owned;
    type Record = Self;
}

struct Members;

impl Area for Members {
    const NAME: &'static str = "members";
    const PREFIX: &'static str = "/members";
    const SIGN_IN: &'static str = "/sign-in";
    type Principal = Account;
}

async fn export() -> Result<()> {
    Ok(())
}

fn queued_values(transactions: &[Vec<Step>]) -> Vec<SqlValue> {
    transactions
        .iter()
        .flatten()
        .flat_map(|step| match step {
            Step::Statement { params, .. } => params.clone(),
            Step::Script(_) => Vec::new(),
        })
        .flat_map(|value| match value {
            SqlValue::Array(values) => values,
            other => vec![other],
        })
        .collect()
}

#[tokio::test]
async fn a_route_queues_its_job_with_the_input_and_path_parameters() {
    let driver = FakeDriver::default();
    let db = Db::new(driver.clone());
    sofie_db::name_job(&db, "reports.actions.export", export);
    let app = App::new(db).area("reports", |routes: &mut Routes<Web>| {
        routes.queue("/reports/{report}/export", export);
    });
    let server = TestServer::start(app).await;

    let answered = server
        .post("/reports/r1/export")
        .header("sec-fetch-site", "same-origin")
        .json(&json!({ "format": "csv", "report": "posted" }))
        .await;

    assert_eq!(answered.status, StatusCode::NO_CONTENT, "{}", answered.body);
    let values = queued_values(&driver.take_transactions());
    assert!(
        values.contains(&SqlValue::Text("reports.actions.export".to_owned())),
        "{values:?}"
    );
    let payload = values
        .iter()
        .find_map(|value| match value {
            SqlValue::Json(payload) => Some(payload.clone()),
            SqlValue::Text(text) => serde_json::from_str(text).ok(),
            _ => None,
        })
        .expect("the job's payload is written");
    assert_eq!(payload, json!({ "format": "csv", "report": "r1" }));
}

#[tokio::test]
async fn an_area_whose_principals_own_their_records_queues_no_job() {
    let app =
        App::new(Db::new(FakeDriver::default())).area("reports", |routes: &mut Routes<Members>| {
            routes.queue("/reports/export", export);
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();

    let refused = app.serve(listener, async {}).await.unwrap_err();

    assert!(
        refused.to_string().contains("reaches every record"),
        "{refused}"
    );
}
