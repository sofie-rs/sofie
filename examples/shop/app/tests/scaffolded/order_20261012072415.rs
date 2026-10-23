//! Scaffolded by `sofie scaffold tests` at 2026-10-12 07:24:15 UTC for module `order`,
//! and yours from now on: a later run writes a new file for facts that
//! are new or changed, and names the tests here whose fact changed.
//!
//! fact `schema AuditEntry`: `audit_entry_table_matches_its_schema`
//! fact `schema Order`: `order_table_matches_its_schema`

use sofie_testing::{TestApp, test_app};

#[test_app]
async fn audit_entry_table_matches_its_schema(app: TestApp) {
    sofie_testing::assert_schema::<order::AuditEntry>(&app).await;
}

#[test_app]
async fn order_table_matches_its_schema(app: TestApp) {
    sofie_testing::assert_schema::<order::Order>(&app).await;
}
