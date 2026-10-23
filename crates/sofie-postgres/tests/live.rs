#![cfg(feature = "live")]

use std::collections::BTreeSet;
use std::time::Duration;

use sofie_core::{Cast, ColumnReader, Result, SqlValue, Uuid};
use sofie_db::{
    Aggregate, Announced, Column, Db, Driver, Field, Meta, Reach, Readers, Select, Step,
    TableChange, Topic, Topics, committed_changes, in_select, track_topics,
};
use sofie_postgres::Postgres;
use sofie_testing::TestDatabase;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::TryRecvError;

const CREATE_TABLES: &str = "CREATE SCHEMA notes;
CREATE TABLE notes.note (id uuid PRIMARY KEY, version int8 NOT NULL, author uuid NOT NULL);
CREATE TABLE notes.tag (id uuid PRIMARY KEY, version int8 NOT NULL, note uuid NOT NULL)";

const AUTHOR: Uuid = Uuid::from_u128(0x0190_a7d2_0000_7000_8000_0000_0000_0001);

struct Note {
    meta: Meta,
    author: Uuid,
}

impl Note {
    const ID_COLUMN: Column<Self, Uuid> = Column::new("id");
}

impl Aggregate for Note {
    const NAME: &'static str = "note";
    const MODULE: &'static str = "notes";
    const FIELDS: &'static [Field] = &[Field::new("author", Uuid::COLUMNS, false)];
    const OWNER: Option<&'static str> = Some("author");

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.author.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            author: Uuid::decode(row)?,
        })
    }

    fn owner(&self) -> Option<Uuid> {
        Some(self.author)
    }
}

struct Tag {
    meta: Meta,
    note: Uuid,
}

impl Tag {
    const NOTE: Column<Self, Uuid> = Column::new("note");
}

impl Aggregate for Tag {
    const NAME: &'static str = "tag";
    const MODULE: &'static str = "notes";
    const FIELDS: &'static [Field] = &[Field::new("note", Uuid::COLUMNS, false)];

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.note.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            note: Uuid::decode(row)?,
        })
    }
}

async fn database() -> (TestDatabase, Postgres) {
    let database = TestDatabase::create().await;
    let driver = Postgres::connect(database.url()).unwrap();
    driver
        .transaction(vec![Step::Script(CREATE_TABLES.to_owned())])
        .await
        .unwrap();
    (database, driver)
}

fn note() -> Note {
    Note {
        meta: Meta::new().unwrap(),
        author: AUTHOR,
    }
}

fn pending(heard: &mut Receiver<sofie_db::Announced>) -> Vec<TableChange> {
    let mut changes = Vec::new();
    loop {
        match heard.try_recv() {
            Ok(Announced::Changed(batch)) => changes.extend(batch.iter().copied()),
            Ok(Announced::MaybeMissed) => panic!("listening went down"),
            Err(TryRecvError::Empty) => return changes,
            Err(error) => panic!("{error}"),
        }
    }
}

#[tokio::test]
async fn a_commit_reaches_this_instance_at_once_and_another_through_notify() {
    let (database, driver) = database().await;
    let writing = Db::new(driver);
    let other = Db::new(Postgres::connect(database.url()).unwrap());
    let mut here = committed_changes(&writing);
    let mut elsewhere = committed_changes(&other);
    let expected = TableChange {
        topic: Topic::of::<Note>(),
        readers: Readers::Owner(AUTHOR),
    };

    let mut commits = 0;
    let heard = loop {
        writing.commit(&mut note()).await.unwrap();
        commits += 1;
        if let Ok(Ok(heard)) =
            tokio::time::timeout(Duration::from_millis(200), elsewhere.recv()).await
        {
            break heard;
        }
        assert!(commits < 25, "another instance never heard of the commit");
    };
    tokio::time::sleep(Duration::from_millis(300)).await;

    assert_eq!(heard, Announced::Changed([expected].into()));
    assert_eq!(
        pending(&mut here),
        vec![expected; commits],
        "each commit is heard here once, never again through the notification"
    );
    assert!(expected.reaches(Reach::OwnedBy(AUTHOR)));
    assert!(!expected.reaches(Reach::OwnedBy(Uuid::from_u128(2))));
}

#[tokio::test]
async fn a_listener_that_listens_again_tells_its_screens_they_may_have_missed_changes() {
    let (database, driver) = database().await;
    let listening = Db::new(driver);
    let mut heard = committed_changes(&listening);
    let admin = Postgres::connect(database.url()).unwrap();
    let listeners = "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
        WHERE datname = current_database() AND query LIKE 'LISTEN%' AND pid <> pg_backend_pid()";

    let mut tries = 0;
    let missed = loop {
        admin
            .transaction(vec![Step::Script(listeners.to_owned())])
            .await
            .unwrap();
        if let Ok(Ok(missed)) = tokio::time::timeout(Duration::from_secs(1), heard.recv()).await {
            break missed;
        }
        tries += 1;
        assert!(tries < 15, "the listener never listened again");
    };

    assert_eq!(missed, Announced::MaybeMissed);
    assert!(missed.concerns(&Topics::Of(BTreeSet::new()), Reach::OwnedBy(AUTHOR)));
}

#[tokio::test]
async fn a_read_names_its_table_and_those_its_conditions_select_from() {
    let (_database, driver) = database().await;
    let db = Db::new(driver);
    let read = db.read();

    let (notes, plain) = track_topics(Note::query().all(&read)).await;
    let tagged = in_select(Note::ID_COLUMN, Tag::NOTE, Tag::NOTE.ne(Uuid::from_u128(1)));
    let (count, selecting) = track_topics(Note::query().where_(tagged).count(&read)).await;

    assert!(notes.unwrap().is_empty());
    assert_eq!(count.unwrap(), 0);
    assert_eq!(plain, Topics::Of(BTreeSet::from([Topic::of::<Note>()])));
    assert_eq!(
        selecting,
        Topics::Of(BTreeSet::from([Topic::of::<Note>(), Topic::of::<Tag>()]))
    );
}
