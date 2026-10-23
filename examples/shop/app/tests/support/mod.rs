#![allow(dead_code, reason = "each test file uses its own part of this module")]

use serde_json::json;
use sofie::db::Db;
use sofie::http::App;
use sofie_testing::{TestApp, TestSession};

pub const PASSWORD: &str = "correct horse battery";

pub async fn shop() -> TestApp {
    sofie_testing::start(migrated, shop::worker).await
}

/// The binary migrates only when told to; a test's database starts empty.
async fn migrated(db: Db) -> sofie::core::Result<App> {
    db.schema()
        .migrate(&sofie::http::migrations(shop::MODULES))
        .await?;
    shop::app(db).await
}

pub async fn customer(shop: &TestApp, email: &str) -> TestSession {
    let credentials = json!({ "email": email, "password": PASSWORD });
    let signed_up = shop.post("/sign-up").json(&credentials).await;
    assert_eq!(signed_up.status, 204, "{}", signed_up.body);
    let signed_in = shop.post("/sign-in").json(&credentials).await;
    assert_eq!(signed_in.status, 204, "{}", signed_in.body);
    signed_in
        .session()
        .expect("signing in sets the session cookie")
}
