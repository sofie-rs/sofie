//! A derived aggregate's instant filter in Berlin's repeated hour, through a resource's list.
#![cfg(feature = "resources")]

use sofie::db::{Db, File, Migrations};
use sofie::prelude::*;
use sofie_testing::TestApp;

#[derive(Aggregate)]
pub struct Booking {
    meta: Meta,
    starts_at: Instant,
}

const MIGRATIONS: &[File] = &[File::new(
    "0001_booking.sql",
    b"CREATE TABLE booking (id uuid PRIMARY KEY, version bigint NOT NULL, \
     starts_at timestamptz NOT NULL, created_at timestamptz NOT NULL, \
     updated_at timestamptz NOT NULL);",
)];

const REPEATED: &str = "/bookings?starts_at=2026-10-25T02:30";

async fn started() -> TestApp {
    sofie_testing::set_display_zone("Europe/Berlin");
    sofie_testing::start(
        |db: Db| async move {
            db.schema()
                .migrate(&[Migrations::new("listing_fold", MIGRATIONS)])
                .await?;
            Ok(App::new(db).area::<Web>("listing_fold", |routes| {
                routes.resource::<Booking>();
            }))
        },
        sofie::worker::Worker::new,
    )
    .await
}

#[tokio::test]
async fn a_page_shows_a_refused_instant_filter_beside_it_listing_nothing() {
    let app = started().await;

    let listed = app.get(REPEATED).await;

    assert_eq!(listed.status, 200, "{}", listed.body);
    assert!(
        listed
            .body
            .contains("<span class=\"sofie-field-error\">2026-10-25T02:30:00 happens twice"),
        "{}",
        listed.body
    );
    assert!(listed.body.contains("No entries"), "{}", listed.body);
}

#[tokio::test]
async fn a_json_caller_gets_a_refused_instant_filter_as_a_422_naming_both_offsets() {
    let app = started().await;

    let refused = app.get(REPEATED).accept_json().await;

    assert_eq!(refused.status, 422, "{}", refused.body);
    let body = refused.json().to_string();
    assert!(body.contains("starts_at"), "{body}");
    assert!(body.contains("+02:00") && body.contains("+01:00"), "{body}");
}
