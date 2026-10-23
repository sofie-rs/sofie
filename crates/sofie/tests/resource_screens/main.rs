//! A module writing its own `factory.rs`, so its inputs leave out fields the record holds.
#![cfg(feature = "resources")]

use serde_json::json;
use sofie::db::{Db, File, Id, Migrations};
use sofie::prelude::*;
use sofie_testing::TestApp;

mod factory;

#[derive(Aggregate)]
pub struct Ticket {
    meta: Meta,
    project: Id<Project>,
    #[required]
    title: String,
    status: String,
}

pub struct Project;

const PROJECT: &str = "0190f1d2-7a3b-7c4d-8e5f-6a7b8c9d0e1f";

const MIGRATIONS: &[File] = &[File::new(
    "0001_ticket.sql",
    b"CREATE TABLE ticket (id uuid PRIMARY KEY, version bigint NOT NULL, project uuid NOT NULL, \
     title text NOT NULL, status text NOT NULL, created_at timestamptz NOT NULL, \
     updated_at timestamptz NOT NULL);",
)];

async fn started() -> TestApp {
    sofie_testing::start(
        |db: Db| async move {
            db.schema()
                .migrate(&[Migrations::new("resource_screens", MIGRATIONS)])
                .await?;
            Ok(App::new(db).area::<Web>("resource_screens", |routes| {
                routes.resource::<Ticket>();
            }))
        },
        sofie::worker::Worker::new,
    )
    .await
}

async fn created(app: &TestApp) -> String {
    let created = app
        .post("/tickets/new")
        .json(&json!({ "project": PROJECT, "title": "Printer jams" }))
        .await;
    assert_eq!(created.status, 200, "{}", created.body);
    created.json()["id"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn a_record_screen_shows_what_the_record_holds_beyond_its_input_and_hides_its_ids() {
    let app = started().await;
    let id = created(&app).await;

    let shown = app.get(&format!("/tickets/{id}")).await;
    let listed = app.get("/tickets").await;

    assert_eq!(shown.status, 200, "{}", shown.body);
    assert!(shown.body.contains("<dd>open</dd>"), "{}", shown.body);
    assert!(!shown.body.contains(PROJECT), "{}", shown.body);
    assert_eq!(listed.status, 200, "{}", listed.body);
    assert!(listed.body.contains(">open<"), "{}", listed.body);
    assert!(!listed.body.contains(PROJECT), "{}", listed.body);
    assert!(!listed.body.contains(&format!(">{id}<")), "{}", listed.body);
    assert!(!listed.body.contains("name=\"project"), "{}", listed.body);
}

#[tokio::test]
async fn a_record_form_keeps_to_its_input() {
    let app = started().await;
    let id = created(&app).await;

    let form = app.get(&format!("/tickets/{id}/edit")).await;

    assert_eq!(form.status, 200, "{}", form.body);
    assert!(form.body.contains("name=\"title\""), "{}", form.body);
    assert!(!form.body.contains("name=\"status\""), "{}", form.body);
}
