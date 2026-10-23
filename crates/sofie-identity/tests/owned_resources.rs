#![cfg(feature = "permitted-screens")]

use serde_json::json;
use sofie::db::{Db, File, Migrations};
use sofie::prelude::*;
use sofie_identity::{AccountId, Memberships};
use sofie_testing::{TestApp, TestSession};
use support::Member;

mod support;

#[derive(Aggregate)]
#[aggregate(behaviors(RecordsChanges))]
pub struct Note {
    meta: Meta,
    #[owner]
    account: AccountId,
    #[required]
    #[expose]
    title: String,
}

const MIGRATIONS: &[File] = &[File::new(
    "0001_note.sql",
    b"CREATE TABLE note (id uuid PRIMARY KEY, version bigint NOT NULL, account uuid NOT NULL, \
     title text NOT NULL, created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL);",
)];

async fn started() -> TestApp {
    sofie_testing::start(
        |db: Db| async move {
            db.schema()
                .migrate(&[
                    sofie_identity::migrations(),
                    Migrations::new("owned_resources", MIGRATIONS),
                ])
                .await?;
            let mut screens = sofie::http::Screens::default();
            screens.add_builtin(sofie_identity::module(&[]).screens)?;
            Ok(App::new(db)
                .screens(screens)
                .membership(Memberships)
                .area::<Web>("sofie_identity", sofie_identity::routes::<Member>)
                .area::<Member>("owned_resources", |routes| {
                    routes.resource::<Note>();
                }))
        },
        sofie::worker::Worker::new,
    )
    .await
}

async fn member(app: &TestApp, email: &str) -> TestSession {
    sofie_identity::testing::signed_up::<Member>(app, email, support::PASSWORD).await
}

#[tokio::test]
async fn a_member_writes_and_reads_only_their_own_records() {
    let app = started().await;
    let ada = member(&app, "ada@example.com").await;
    let grace = member(&app, "grace@example.com").await;

    let created = app
        .post("/account/notes/new")
        .session(&ada)
        .json(&json!({ "title": "milk" }))
        .await;
    assert_eq!(created.status, 200, "{}", created.body);
    let id = created.json()["id"].as_str().unwrap().to_owned();
    let changed = app
        .post(&format!("/account/notes/{id}/edit"))
        .session(&ada)
        .json(&json!({ "title": "oat milk" }))
        .await;
    assert_eq!(changed.status, 200, "{}", changed.body);

    let shown = app.get(&format!("/account/notes/{id}")).session(&ada).await;
    assert!(shown.body.contains("<dd>oat milk</dd>"), "{}", shown.body);
    assert!(shown.body.contains("<td>Created</td>"), "{}", shown.body);
    assert!(
        shown.body.contains("<td>Updated</td><td>title</td>"),
        "{}",
        shown.body
    );

    let elsewhere = app
        .get(&format!("/account/notes/{id}"))
        .session(&grace)
        .accept_json()
        .await;
    assert_eq!(elsewhere.status, 404, "{}", elsewhere.body);
    let listed = app
        .get("/account/notes")
        .session(&grace)
        .accept_json()
        .await
        .json();
    assert_eq!(listed["list"]["items"], json!([]));
    let blank = app
        .post("/account/notes/new")
        .session(&grace)
        .json(&json!({ "title": " " }))
        .await;
    assert_eq!(blank.status, 422, "{}", blank.body);
}

#[test]
fn a_contract_reads_the_fields_it_declares_by_name() {
    #[derive(sofie::serde::Deserialize)]
    #[serde(crate = "sofie::serde")]
    struct NoteView {
        title: String,
    }

    let note = Note {
        meta: Meta::new().unwrap(),
        account: AccountId::from_uuid(sofie::db::Uuid::now_v7().unwrap()),
        title: "milk".to_owned(),
    };
    let view: NoteView = sofie::db::exposed(&note).unwrap();

    assert_eq!(view.title, "milk");
}
