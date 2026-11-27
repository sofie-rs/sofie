#![allow(dead_code, reason = "each test file uses its own part of this module")]

use order_contract::area::Customer;
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
    sofie_identity::testing::signed_up::<Customer>(shop, email, PASSWORD).await
}

pub async fn confirmed(shop: &TestApp, email: &str) {
    sofie_identity::testing::confirm_where_asked::<Customer>(shop, email).await;
}
