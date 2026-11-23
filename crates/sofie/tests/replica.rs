#![cfg(feature = "replica")]

use sofie::db::{Db, File, Migrations};
use sofie::postgres::Postgres;
use sofie::prelude::*;
use sofie_testing::{TestDatabase, TestServer};

const MIGRATIONS: &[File] = &[File::new(
    "0001_note.sql",
    b"CREATE TABLE note (
        id uuid PRIMARY KEY,
        version bigint NOT NULL,
        created_at timestamptz NOT NULL,
        updated_at timestamptz NOT NULL,
        text text NOT NULL
    );",
)];

#[derive(Aggregate)]
pub struct Note {
    meta: Meta,
    text: String,
}

#[derive(ReadModel)]
#[read(from = Note)]
pub struct NoteRow {
    text: String,
}

async fn show(note: NoteRow) -> Result<NoteRow> {
    Ok(note)
}

async fn edit(mut note: Note) -> Result<Note> {
    "edited".clone_into(&mut note.text);
    Ok(note)
}

/// The same note in two databases with different text, so a read tells which one answered.
async fn databases() -> (TestDatabase, TestDatabase, Db, Id<Note>) {
    let (primary, replica) = (TestDatabase::create().await, TestDatabase::create().await);
    let id = Meta::new().unwrap();
    for (database, text) in [(&primary, "on the primary"), (&replica, "on the replica")] {
        let db = Db::new(Postgres::connect(database.url()).unwrap());
        db.schema()
            .migrate(&[Migrations::new("replica", MIGRATIONS)])
            .await
            .unwrap();
        let mut note = Note {
            meta: id.clone(),
            text: text.to_owned(),
        };
        db.commit(&mut note).await.unwrap();
    }
    let db = sofie::db::with_replica(
        Db::new(Postgres::connect(primary.url()).unwrap()),
        Postgres::connect(replica.url()).unwrap(),
    );
    (primary, replica, db, Id::from_uuid(id.id()))
}

#[tokio::test]
async fn a_get_reads_the_replica_until_the_client_writes() {
    let (_primary, _replica, db, id) = databases().await;
    let server = TestServer::start(App::new(db).area("replica", |routes: &mut Routes<Web>| {
        routes.get("/notes/{note}", show);
        routes.post("/notes/{note}", edit);
    }))
    .await;
    let path = format!("/notes/{id}");

    let read = server.get(&path).accept_json().await;
    assert_eq!(read.json()["text"], "on the replica");
    assert!(!read.headers.contains_key("set-cookie"));

    let written = server
        .post(&path)
        .accept_json()
        .header("sec-fetch-site", "same-origin")
        .await;
    written.assert_ok();
    let cookie = written.header("set-cookie");
    assert!(cookie.contains("-primary=1; Max-Age=5;"), "{cookie}");

    let marked = cookie.split(';').next().unwrap();
    let after = server
        .get(&path)
        .accept_json()
        .header("cookie", marked)
        .await;
    assert_eq!(after.json()["text"], "edited");
    let unmarked = server.get(&path).accept_json().await;
    assert_eq!(unmarked.json()["text"], "on the replica");
}
