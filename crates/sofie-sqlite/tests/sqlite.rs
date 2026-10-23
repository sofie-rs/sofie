use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use sofie_core::{Cast, ColumnReader, Error, Result, SqlType, SqlValue, Uuid};
use sofie_db::worker::Worker;
use sofie_db::{
    Aggregate, Catalog, Column, Condition, Db, Driver, Field, Id, Load, Meta, Migrations, Reach,
    Read, Select, Step, owned_through, refuse_parent_beyond_reach,
};
use sofie_sqlite::Sqlite;

const NOTES: &str = "CREATE TABLE notes_note (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    title text NOT NULL,
    author uuid NOT NULL
);
CREATE UNIQUE INDEX note_title ON notes_note (title)";

struct Note {
    meta: Meta,
    title: String,
    author: Uuid,
}

impl Note {
    const TITLE: Column<Self, String> = Column::new("title");

    fn new(title: &str) -> Self {
        Self {
            meta: Meta::new().unwrap(),
            title: title.to_owned(),
            author: Uuid::from_u128(7),
        }
    }
}

impl Aggregate for Note {
    const NAME: &'static str = "note";
    const MODULE: &'static str = "notes";
    const FIELDS: &'static [Field] = &[
        Field::new("title", String::COLUMNS, false),
        Field::new("author", Uuid::COLUMNS, false),
    ];
    const UNIQUE: &'static [&'static str] = &["title"];

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.title.encode(out);
        self.author.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            title: String::decode(row)?,
            author: Uuid::decode(row)?,
        })
    }
}

/// A file, so a second connection reads what the app's wrote.
struct File(std::path::PathBuf);

impl File {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("sofie-sqlite-{name}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        Self(path)
    }

    fn open(&self) -> Sqlite {
        Sqlite::open(&self.0).unwrap()
    }
}

impl Drop for File {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
        }
    }
}

/// Postgres's file would fail on SQLite, so passing proves SQLite ran its own.
const MIGRATIONS: Migrations = Migrations::new(
    "notes",
    &[sofie_db::File::new(
        "0001_notes.sql",
        b"CREATE SCHEMA notes",
    )],
)
.sqlite(&[sofie_db::File::new("0001_notes.sql", NOTES.as_bytes())]);

async fn migrated_in(sqlite: Sqlite) -> Db {
    let db = Db::new(sqlite);
    db.schema().migrate(&[MIGRATIONS]).await.unwrap();
    db
}

async fn migrated() -> Db {
    migrated_in(Sqlite::memory().unwrap()).await
}

#[tokio::test]
async fn a_record_is_written_and_read_back_from_its_module_s_table() {
    let db = migrated().await;
    let mut note = Note::new("Groceries");
    db.commit(&mut note).await.unwrap();
    note.title = "Groceries, done".to_owned();
    db.commit(&mut note).await.unwrap();

    let read = Note::query()
        .where_(Note::TITLE.eq("Groceries, done".to_owned()))
        .all(&db.read())
        .await
        .unwrap();

    assert_eq!(read.len(), 1);
    assert_eq!(read[0].id(), note.id());
    assert_eq!(read[0].author, Uuid::from_u128(7));
    assert_eq!(read[0].meta().version(), 2);
}

#[tokio::test]
async fn an_escaped_wildcard_in_like_matches_only_itself() {
    let db = migrated().await;
    db.commit(&mut Note::new("50% off")).await.unwrap();
    db.commit(&mut Note::new("500 off")).await.unwrap();

    let read = Note::query()
        .where_(Note::TITLE.like("50\\%%"))
        .all(&db.read())
        .await
        .unwrap();

    let titles: Vec<&str> = read.iter().map(|note| note.title.as_str()).collect();
    assert_eq!(titles, ["50% off"]);
}

#[tokio::test]
async fn like_tells_capitals_apart_as_on_postgres() {
    let db = migrated().await;
    db.commit(&mut Note::new("Dune")).await.unwrap();

    let lower = Note::query()
        .where_(Note::TITLE.like("d%"))
        .count(&db.read())
        .await;

    assert_eq!(lower.unwrap(), 0);
}

#[tokio::test]
async fn a_taken_unique_value_is_a_field_error() {
    let db = migrated().await;
    db.commit(&mut Note::new("Plans")).await.unwrap();

    let refused = db.commit(&mut Note::new("Plans")).await.unwrap_err();

    assert!(
        matches!(&refused, Error::Invalid(fields) if fields[0].field == "title"),
        "{refused:?}"
    );
}

#[tokio::test]
async fn migrations_apply_once_and_the_catalog_reads_a_module_s_columns() {
    let file = File::new("catalog");
    let db = migrated_in(file.open()).await;
    db.schema().migrate(&[MIGRATIONS]).await.unwrap();

    let columns = file.open().columns("notes", "note").await.unwrap();

    let read: Vec<(&str, Option<SqlType>, bool)> = columns
        .iter()
        .map(|column| (column.name.as_str(), column.ty, column.nullable))
        .collect();
    assert_eq!(
        read,
        [
            ("id", Some(SqlType::Uuid), true),
            ("version", Some(SqlType::Int), false),
            ("title", Some(SqlType::Text), false),
            ("author", Some(SqlType::Uuid), false),
        ]
    );
}

#[tokio::test]
async fn fresh_drops_every_table_even_one_another_references_and_migrates_again() {
    let file = File::new("fresh");
    let db = migrated_in(file.open()).await;
    let mut note = Note::new("Old");
    db.commit(&mut note).await.unwrap();
    let tags = format!(
        "CREATE TABLE notes_tag (note uuid NOT NULL REFERENCES notes_note (id));
         INSERT INTO notes_tag VALUES ('{}')",
        note.id()
    );
    file.open()
        .transaction(vec![Step::Script(tags)])
        .await
        .unwrap();

    db.schema().fresh(&[MIGRATIONS]).await.unwrap();

    let tables = sofie_db::query_values(
        &file.open(),
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name LIKE 'notes%'",
        &[],
    )
    .await
    .unwrap();
    assert_eq!(tables, [vec![SqlValue::Text("notes_note".to_owned())]]);
    assert_eq!(Note::query().all(&db.read()).await.unwrap().len(), 0);
}

static RUNS: AtomicUsize = AtomicUsize::new(0);

async fn tick() -> Result<()> {
    RUNS.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

#[tokio::test]
async fn a_new_schedule_fires_once_until_it_is_due_again() {
    let file = File::new("schedule");
    let db = migrated_in(file.open()).await;
    let worker = Worker::new(db).every("notes.tick", Duration::from_secs(3600), tick);

    assert_eq!(worker.drain().await.unwrap(), 1);
    assert_eq!(worker.drain().await.unwrap(), 0);
    assert_eq!(RUNS.load(Ordering::SeqCst), 1);
    let queued = sofie_db::query_values(&file.open(), "SELECT count(*) FROM sofie_messages", &[])
        .await
        .unwrap();
    assert_eq!(queued, [[SqlValue::Int(0)]]);
}

#[tokio::test]
async fn whether_a_run_still_waits_is_read_from_an_index() {
    let file = File::new("waiting-index");
    migrated_in(file.open()).await;
    let plan = "EXPLAIN QUERY PLAN SELECT 1 FROM sofie_messages \
                WHERE name = 'notes.tick' AND kind = 'job' AND dead_at IS NULL";

    let steps = sofie_db::query_values(&file.open(), plan, &[])
        .await
        .unwrap();

    let detail = format!("{steps:?}");
    assert!(
        detail.contains("USING INDEX sofie_messages_waiting"),
        "{detail}"
    );
}

#[tokio::test]
async fn read_inbox_entries_and_sent_digest_items_are_pruned_through_an_index() {
    let file = File::new("pruned-indexes");
    migrated_in(file.open()).await;
    let pruned = [
        ("sofie_inbox", "read_at", "sofie_inbox_read"),
        (
            "sofie_notify_digest_items",
            "sent_at",
            "sofie_notify_digest_items_sent",
        ),
    ];

    for (table, column, index) in pruned {
        let plan = format!(
            "EXPLAIN QUERY PLAN SELECT id FROM {table} \
             WHERE {column} < '2026-01-01T00:00:00Z' ORDER BY {column} NULLS LAST LIMIT 1000"
        );
        let steps = sofie_db::query_values(&file.open(), &plan, &[])
            .await
            .unwrap();

        let detail = format!("{steps:?}");
        assert!(detail.contains(&format!("USING INDEX {index}")), "{detail}");
    }
}

async fn left_messages(file: &File) -> Vec<Vec<SqlValue>> {
    let sql = "SELECT attempts, dead_at IS NOT NULL, last_error FROM sofie_messages";
    sofie_db::query_values(&file.open(), sql, &[])
        .await
        .unwrap()
}

async fn explode() -> Result<()> {
    panic!("the printer caught fire")
}

#[tokio::test]
async fn a_panicking_job_fails_as_an_attempt_and_the_worker_goes_on() {
    let file = File::new("panic");
    let db = migrated_in(file.open()).await;
    let worker = Worker::new(db).every("notes.explode", Duration::from_secs(3600), explode);

    assert_eq!(worker.drain().await.unwrap(), 0);

    assert_eq!(
        left_messages(&file).await,
        [[
            SqlValue::Int(1),
            SqlValue::Int(0),
            SqlValue::Text("the job panicked: the printer caught fire".to_owned())
        ]]
    );
}

async fn refuse() -> Result<()> {
    Err(Error::Rule("a note cannot be filed twice".to_owned()))
}

#[tokio::test]
async fn a_refused_job_is_dead_at_once_instead_of_retried() {
    let file = File::new("refused");
    let db = migrated_in(file.open()).await;
    let worker = Worker::new(db).every("notes.refuse", Duration::from_secs(3600), refuse);

    assert_eq!(worker.drain().await.unwrap(), 0);

    let left = left_messages(&file).await;
    assert_eq!(left[0][..2], [SqlValue::Int(1), SqlValue::Int(1)]);
}

async fn every_note(read: sofie_db::Read) -> Result<Vec<Note>> {
    Note::query().all(&read).await
}

#[tokio::test]
async fn a_record_whose_job_died_gets_a_new_one_on_the_next_run() {
    let file = File::new("each");
    let clock = sofie_core::TestClock::starting_at(
        sofie_core::Timestamp::from_second(1_700_000_000).unwrap(),
    );
    let db = migrated_in(file.open()).await.with_clock(clock.clock());
    db.commit(&mut Note::new("Plans")).await.unwrap();
    let hour = Duration::from_secs(3600);
    let worker = Worker::new(db).each("notes.each", hour, every_note, "notes.file", refuse);

    worker.drain().await.unwrap();
    clock.advance(2 * hour).unwrap();
    worker.drain().await.unwrap();

    let sql =
        "SELECT count(*) FROM sofie_messages WHERE name = 'notes.file' AND dead_at IS NOT NULL";
    let dead = sofie_db::query_values(&file.open(), sql, &[])
        .await
        .unwrap();
    assert_eq!(dead, [[SqlValue::Int(2)]]);
}

#[tokio::test]
async fn a_local_schedule_that_fails_to_fire_waits_before_it_is_tried_again() {
    let file = File::new("local");
    let clock = sofie_core::TestClock::starting_at(
        sofie_core::Timestamp::from_second(1_700_000_000).unwrap(),
    );
    let db = migrated_in(file.open()).await.with_clock(clock.clock());
    sofie_db::name_job(&db, "notes.tick", tick);
    let at = sofie_values::LocalDateTime::parse(
        "2023-01-01T09:00",
        sofie_values::TimeZone::parse("UTC").unwrap(),
    )
    .unwrap();
    let mut scheduled = sofie_db::LocalJobs::within(&db, sofie_db::Reach::All)
        .set("broken", tick, &(), at, sofie_db::Repeat::Once)
        .await
        .unwrap();
    db.commit(&mut scheduled).await.unwrap();
    let broken = "UPDATE sofie_local_schedules SET payload = 'not json'";
    sofie_db::query_values(&file.open(), broken, &[])
        .await
        .unwrap();

    Worker::new(db.clone()).drain().await.unwrap();

    let sql = "SELECT count(*) FROM sofie_local_schedules WHERE next_at > ?1";
    let now = SqlValue::Timestamp(db.clock().now().timestamp());
    let waiting = sofie_db::query_values(&file.open(), sql, &[now])
        .await
        .unwrap();
    assert_eq!(waiting, [[SqlValue::Int(1)]]);
}

struct Visit {
    meta: Meta,
    at: sofie_core::Instant,
}

impl Visit {
    const AT: Column<Self, sofie_core::Instant> = Column::new("at");
}

impl Aggregate for Visit {
    const NAME: &'static str = "visit";
    const MODULE: &'static str = "notes";
    const FIELDS: &'static [Field] = &[Field::new("at", sofie_core::Instant::COLUMNS, false)];

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.at.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            at: sofie_core::Instant::decode(row)?,
        })
    }
}

async fn forget_visits(prune: sofie_db::Prune, clock: sofie_core::Clock) -> Result<()> {
    prune.before(Visit::AT, clock.now()).await.map(drop)
}

#[tokio::test]
async fn pruned_records_are_announced_as_a_commit_s_are() {
    let file = File::new("prune");
    let db = migrated_in(file.open()).await;
    let visits = "CREATE TABLE notes_visit (id uuid PRIMARY KEY, version bigint NOT NULL, at timestamptz NOT NULL)";
    sofie_db::query_values(&file.open(), visits, &[])
        .await
        .unwrap();
    let past = sofie_core::Clock::system()
        .now()
        .checked_sub(Duration::from_secs(60))
        .unwrap();
    db.commit(&mut Visit {
        meta: Meta::new().unwrap(),
        at: past,
    })
    .await
    .unwrap();
    let mut heard = sofie_db::committed_changes(&db);

    let worker = Worker::new(db).every("notes.forget", Duration::from_secs(3600), forget_visits);
    assert_eq!(worker.drain().await.unwrap(), 1);

    let Ok(sofie_db::Announced::Changed(changes)) = heard.try_recv() else {
        panic!("pruning announced nothing");
    };
    assert_eq!(changes[0].topic, sofie_db::Topic::of::<Visit>());
}

struct Cover(Vec<u8>);

impl Cast for Cover {
    const COLUMNS: &'static [sofie_core::ColumnSpec] =
        &[sofie_core::ColumnSpec::new("", SqlType::Bytes)];

    fn encode(&self, out: &mut Vec<SqlValue>) {
        out.push(SqlValue::Bytes(self.0.clone()));
    }

    fn decode(row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self(row.bytes()?.to_vec()))
    }
}

struct Book {
    id: Id<Book>,
    cover: Cover,
}

impl sofie_db::Child for Book {
    const TABLE: sofie_db::ChildTable = sofie_db::ChildTable {
        module: "notes",
        name: "book",
        fields: &[Field::new("cover", Cover::COLUMNS, false)],
    };

    fn id(&self) -> Uuid {
        self.id.uuid()
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.cover.encode(out);
    }

    fn decode(id: Uuid, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            id: Id::from_uuid(id),
            cover: Cover::decode(row)?,
        })
    }
}

struct Shelf {
    meta: Meta,
    books: sofie_db::Children<Book>,
}

impl Aggregate for Shelf {
    const NAME: &'static str = "shelf";
    const MODULE: &'static str = "notes";
    const FIELDS: &'static [Field] = &[];
    const CHILDREN: &'static [&'static sofie_db::ChildTable] = &[&<Book as sofie_db::Child>::TABLE];

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, _out: &mut Vec<SqlValue>) {}

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            books: sofie_db::Children::decode(row)?,
        })
    }

    fn children(&self) -> Vec<&dyn sofie_db::ChildRows> {
        vec![&self.books]
    }

    fn children_mut(&mut self) -> Vec<&mut dyn sofie_db::ChildRows> {
        vec![&mut self.books]
    }
}

#[tokio::test]
async fn a_child_s_bytes_load_back() {
    let file = File::new("bytes");
    let db = migrated_in(file.open()).await;
    let tables = "CREATE TABLE notes_shelf (id uuid PRIMARY KEY, version bigint NOT NULL);\n\
        CREATE TABLE notes_book (id uuid PRIMARY KEY, shelf uuid NOT NULL, cover blob NOT NULL)";
    sofie_db::query_values(&file.open(), tables, &[])
        .await
        .unwrap();
    let mut shelf = Shelf {
        meta: Meta::new().unwrap(),
        books: sofie_db::Children::default(),
    };
    shelf.books.push(Book {
        id: Id::from_uuid(Uuid::now_v7().unwrap()),
        cover: Cover(vec![0x0a, 0xff]),
    });
    db.commit(&mut shelf).await.unwrap();

    let loaded = Shelf::find(&db.read(), shelf.id()).await.unwrap();

    assert_eq!(loaded.books[0].cover.0, [0x0a, 0xff]);
}

async fn refusal_of(name: &'static str, interval: Duration) -> String {
    Worker::new(Db::new(Sqlite::memory().unwrap()))
        .every(name, interval, tick)
        .drain()
        .await
        .expect_err("the schedule is refused")
        .to_string()
}

#[tokio::test]
async fn a_schedule_whose_next_time_cannot_be_computed_is_refused() {
    let refusal = refusal_of("notes.never", Duration::MAX).await;
    assert!(
        refusal.contains("schedule `notes.never` runs every 18446744073709551615.999999999s"),
        "{refusal}"
    );
}

#[tokio::test]
async fn a_schedule_more_often_than_once_a_second_is_refused() {
    let refusal = refusal_of("notes.spin", Duration::ZERO).await;
    assert!(
        refusal.contains(
            "schedule `notes.spin` runs every 0ns; a schedule runs at most once a second"
        ),
        "{refusal}"
    );
}

const CRM_TABLES: &str = "CREATE TABLE crm_member (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    account uuid NOT NULL
);
CREATE TABLE crm_basket (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    member uuid NOT NULL REFERENCES crm_member (id),
    label text NOT NULL
)";

const CRM: Migrations = Migrations::new("crm", &[])
    .sqlite(&[sofie_db::File::new("0001_crm.sql", CRM_TABLES.as_bytes())]);

struct Member {
    meta: Meta,
    account: Uuid,
}

impl Aggregate for Member {
    const NAME: &'static str = "member";
    const MODULE: &'static str = "crm";
    const FIELDS: &'static [Field] = &[Field::new("account", Uuid::COLUMNS, false)];
    const OWNER: Option<&'static str> = Some("account");

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.account.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            account: Uuid::decode(row)?,
        })
    }

    fn owner(&self) -> Option<Uuid> {
        Some(self.account)
    }
}

struct Basket {
    meta: Meta,
    member: Id<Member>,
    label: String,
}

impl Basket {
    const MEMBER: Column<Self, Id<Member>> = Column::new("member");

    fn new(member: Id<Member>, label: &str) -> Self {
        Self {
            meta: Meta::new().unwrap(),
            member,
            label: label.to_owned(),
        }
    }
}

impl Aggregate for Basket {
    const NAME: &'static str = "basket";
    const MODULE: &'static str = "crm";
    const FIELDS: &'static [Field] = &[
        Field::new("member", Uuid::COLUMNS, false),
        Field::new("label", String::COLUMNS, false),
    ];
    const OWNER: Option<&'static str> = Some("member");

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.member.encode(out);
        self.label.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            member: Id::decode(row)?,
            label: String::decode(row)?,
        })
    }

    fn owned_by(owner: Uuid) -> Option<Condition<Self>> {
        owned_through(Self::MEMBER, owner)
    }
}

async fn member(db: &Db, account: Uuid) -> Id<Member> {
    let mut member = Member {
        meta: Meta::new().unwrap(),
        account,
    };
    db.commit(&mut member).await.unwrap();
    member.id()
}

#[tokio::test]
async fn a_record_owned_through_its_parent_is_kept_to_the_parents_owner() {
    let db = Db::new(Sqlite::memory().unwrap());
    db.schema().migrate(&[CRM]).await.unwrap();
    let (ada_account, bob_account) = (Uuid::now_v7().unwrap(), Uuid::now_v7().unwrap());
    let ada = member(&db, ada_account).await;
    let bob = member(&db, bob_account).await;

    let mut forged = Basket::new(bob, "forged");
    let refused = db
        .commit_with(&mut forged, Vec::new(), Reach::OwnedBy(ada_account))
        .await;
    let mut own = Basket::new(ada, "own");
    let allowed = db
        .commit_with(&mut own, Vec::new(), Reach::OwnedBy(ada_account))
        .await;

    assert!(matches!(refused, Err(Error::NotFound)), "{refused:?}");
    assert!(allowed.is_ok(), "{allowed:?}");
    let labels: Vec<String> = Basket::query()
        .all(&db.read())
        .await
        .unwrap()
        .into_iter()
        .map(|basket| basket.label)
        .collect();
    assert_eq!(labels, ["own"]);
}

#[tokio::test]
async fn a_list_under_another_principal_s_parent_is_not_found() {
    let db = Db::new(Sqlite::memory().unwrap());
    db.schema().migrate(&[CRM]).await.unwrap();
    let (ada_account, bob_account) = (Uuid::now_v7().unwrap(), Uuid::now_v7().unwrap());
    let ada = member(&db, ada_account).await;
    let bob = member(&db, bob_account).await;
    let as_ada = Read::within(&db, Reach::OwnedBy(ada_account));

    let own = refuse_parent_beyond_reach::<Basket>(&as_ada, "member", ada.uuid()).await;
    let other = refuse_parent_beyond_reach::<Basket>(&as_ada, "member", bob.uuid()).await;
    let unowned = refuse_parent_beyond_reach::<Basket>(&db.read(), "member", bob.uuid()).await;

    assert!(own.is_ok(), "{own:?}");
    assert!(matches!(other, Err(Error::NotFound)), "{other:?}");
    assert!(unowned.is_ok(), "{unowned:?}");
}

const LIBRARY: Migrations = Migrations::new("library", &[]).sqlite(&[sofie_db::File::new(
    "0001_library.sql",
    b"CREATE TABLE library_novel (
        id uuid PRIMARY KEY,
        version bigint NOT NULL,
        search_text text NOT NULL,
        title text NOT NULL
    )",
)]);

struct Novel {
    meta: Meta,
    title: String,
}

impl Aggregate for Novel {
    const NAME: &'static str = "novel";
    const MODULE: &'static str = "library";
    const FIELDS: &'static [Field] = &[Field::new("title", String::COLUMNS, false)];
    const BEHAVIORS: &'static [&'static dyn sofie_db::Behavior] = &[&sofie_db::Searched {
        fields: &[("title", String::COLUMNS, sofie_values::Search::Text)],
    }];

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.title.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            title: String::decode(row)?,
        })
    }
}

impl sofie_db::Uses<sofie_db::Searched> for Novel {}

#[tokio::test]
async fn a_search_matches_words_whatever_their_case_beyond_ascii() {
    use sofie_db::Searches;

    let db = Db::new(Sqlite::memory().unwrap());
    db.schema().migrate(&[LIBRARY]).await.unwrap();
    let mut book = Novel {
        meta: Meta::new().unwrap(),
        title: "Öğle Yemeği by REŞAT Nuri".to_owned(),
    };
    db.commit(&mut book).await.unwrap();

    for words in ["öğle", "ÖĞLE", "reşat", "Reşat"] {
        let found = Novel::query()
            .where_(Novel::matching(words))
            .count(&db.read())
            .await
            .unwrap();
        assert_eq!(found, 1, "{words}");
    }
}

struct Errand {
    meta: Meta,
    due: Option<i64>,
}

impl Errand {
    const DUE: Column<Self, Option<i64>> = Column::new("due");
}

impl Aggregate for Errand {
    const NAME: &'static str = "errand";
    const MODULE: &'static str = "notes";
    const FIELDS: &'static [Field] = &[Field::new("due", i64::COLUMNS, true)];

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.due.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            due: Option::<i64>::decode(row)?,
        })
    }
}

async fn errands(dues: &[Option<i64>]) -> Db {
    let sqlite = Sqlite::memory().unwrap();
    let errands =
        "CREATE TABLE notes_errand (id uuid PRIMARY KEY, version bigint NOT NULL, due bigint)";
    sqlite
        .transaction(vec![Step::Script(errands.to_owned())])
        .await
        .unwrap();
    let db = Db::new(sqlite);
    let mut errands: Vec<Errand> = dues
        .iter()
        .map(|due| Errand {
            meta: Meta::new().unwrap(),
            due: *due,
        })
        .collect();
    db.commit(&mut errands).await.unwrap();
    db
}

#[tokio::test]
async fn empty_values_sort_last_ascending_and_first_descending_as_on_postgres() {
    let db = errands(&[Some(2), None, Some(1)]).await;
    let sorted = |sort| async {
        let errands = Errand::query().order_by(sort).all(&db.read()).await;
        errands
            .unwrap()
            .iter()
            .map(|errand| errand.due)
            .collect::<Vec<_>>()
    };

    assert_eq!(sorted(Errand::DUE.asc()).await, [Some(1), Some(2), None]);
    assert_eq!(sorted(Errand::DUE.desc()).await, [None, Some(2), Some(1)]);
}

#[tokio::test]
async fn a_list_holding_none_matches_the_empty_values_too() {
    let db = errands(&[Some(2), None, Some(1)]).await;

    let listed = Errand::query()
        .where_(Errand::DUE.in_([None, Some(1)]))
        .count(&db.read())
        .await;

    assert_eq!(listed.unwrap(), 2);
}

#[tokio::test]
async fn a_broken_check_is_a_refusal_naming_it() {
    let sqlite = Sqlite::memory().unwrap();
    let seats = "CREATE TABLE seats (id integer PRIMARY KEY, count integer NOT NULL \
                 CONSTRAINT seats_positive CHECK (count > 0))";
    sqlite
        .transaction(vec![Step::Script(seats.to_owned())])
        .await
        .unwrap();

    let refused = sqlite
        .transaction(vec![Step::Script(
            "INSERT INTO seats (id, count) VALUES (1, 0)".to_owned(),
        )])
        .await;

    assert!(
        matches!(&refused, Err(Error::Rule(message)) if message.contains("seats_positive")),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_broken_reference_is_a_refusal() {
    let sqlite = Sqlite::memory().unwrap();
    let tables = "CREATE TABLE shelves (id integer PRIMARY KEY); \
                  CREATE TABLE books (id integer PRIMARY KEY, \
                  shelf integer NOT NULL REFERENCES shelves (id))";
    sqlite
        .transaction(vec![Step::Script(tables.to_owned())])
        .await
        .unwrap();

    let refused = sqlite
        .transaction(vec![Step::statement(
            "INSERT INTO books (id, shelf) VALUES (1, 9)",
            Vec::new(),
            None,
        )])
        .await;

    assert!(matches!(&refused, Err(Error::Rule(_))), "{refused:?}");
}

async fn falter() -> Result<()> {
    Err(Error::internal_message("the printer is offline"))
}

#[tokio::test]
async fn a_schedule_queues_no_second_job_while_its_first_still_retries() {
    let file = File::new("pile");
    let clock = sofie_core::TestClock::starting_at(
        sofie_core::Timestamp::from_second(1_700_000_000).unwrap(),
    );
    let db = migrated_in(file.open()).await.with_clock(clock.clock());
    let hour = Duration::from_secs(3600);
    let worker = Worker::new(db).every("notes.falter", hour, falter);

    worker.drain().await.unwrap();
    clock.advance(2 * hour).unwrap();
    worker.drain().await.unwrap();

    let sql = "SELECT count(*) FROM sofie_messages WHERE name = 'notes.falter'";
    let waiting = sofie_db::query_values(&file.open(), sql, &[])
        .await
        .unwrap();
    assert_eq!(waiting, [[SqlValue::Int(1)]]);
}

async fn raise_restocked(file: &File, raised_at: sofie_core::Timestamp) {
    let sql = "INSERT INTO sofie_messages (id, kind, name, payload, available_at) \
               VALUES (?1, 'event', 'shop.Restocked', '{}', ?2)";
    let params = [
        SqlValue::Uuid(Uuid::now_v7().unwrap()),
        SqlValue::Timestamp(raised_at),
    ];
    sofie_db::query_values(&file.open(), sql, &params)
        .await
        .unwrap();
}

#[tokio::test]
async fn an_event_no_listener_hears_waits_an_hour_before_it_is_dropped() {
    let file = File::new("unheard");
    let db = migrated_in(file.open()).await;
    let now = sofie_core::Clock::system().now().timestamp();
    raise_restocked(&file, now).await;
    raise_restocked(&file, now.checked_sub(Duration::from_mins(61)).unwrap()).await;

    Worker::new(db).drain().await.unwrap();

    let sql = "SELECT available_at, attempts FROM sofie_messages";
    let waiting = sofie_db::query_values(&file.open(), sql, &[])
        .await
        .unwrap();
    assert_eq!(waiting, [[SqlValue::Timestamp(now), SqlValue::Int(0)]]);
}

async fn queue_job(file: &File, name: &str, queued_at: sofie_core::Timestamp) {
    let sql = "INSERT INTO sofie_messages (id, kind, name, payload, available_at) \
               VALUES (?1, 'job', ?2, '{}', ?3)";
    let params = [
        SqlValue::Uuid(Uuid::now_v7().unwrap()),
        SqlValue::Text(name.to_owned()),
        SqlValue::Timestamp(queued_at),
    ];
    sofie_db::query_values(&file.open(), sql, &params)
        .await
        .unwrap();
}

/// As on Postgres.
#[tokio::test]
async fn a_worker_holds_a_job_it_does_not_know_an_hour_though_another_worker_knows_it() {
    let file = File::new("held");
    let db = migrated_in(file.open()).await;
    sofie_db::worker::publish_heard(&Worker::new(db.clone()).every(
        "notes.reprice",
        Duration::from_secs(3600),
        tick,
    ));
    let now = sofie_core::Clock::system().now().timestamp();
    let long_ago = now.checked_sub(Duration::from_mins(61)).unwrap();
    queue_job(&file, "notes.reprice", long_ago).await;
    queue_job(&file, "notes.recount", now).await;
    queue_job(&file, "notes.retitle", long_ago).await;

    Worker::new(db).drain().await.unwrap();

    let sql = "SELECT name, attempts, dead_at IS NOT NULL FROM sofie_messages ORDER BY name";
    let left = sofie_db::query_values(&file.open(), sql, &[])
        .await
        .unwrap();
    let row = |name: &str, attempts, dead| {
        [
            SqlValue::Text(name.to_owned()),
            SqlValue::Int(attempts),
            SqlValue::Int(dead),
        ]
    };
    assert_eq!(
        left,
        [
            row("notes.recount", 0, 0),
            row("notes.reprice", 1, 1),
            row("notes.retitle", 1, 1)
        ]
    );
}

struct Restocked(i64);

impl sofie_core::value::serde::Serialize for Restocked {
    fn serialize<S: sofie_core::value::serde::Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_i64(self.0)
    }
}

impl sofie_db::Input for Restocked {
    fn from_json(_: serde_json::Value) -> Result<Self> {
        Ok(Self(0))
    }
}

impl sofie_db::Event for Restocked {
    const NAME: &'static str = "notes.Restocked";
}

/// Joined pair by pair, these took seconds and held the write lock throughout.
#[tokio::test]
async fn a_commit_fans_thousands_of_heard_events_out_to_their_jobs_at_once() {
    const EVENTS: i64 = 2000;
    let file = File::new("fanned");
    let db = migrated_in(file.open()).await;
    sofie_db::worker::publish_heard(
        &Worker::new(db.clone()).on::<Restocked, _>("notes.restock", tick),
    );
    let mut note = Note::new("Stock");
    for count in 0..EVENTS {
        note.meta_mut().raise(Restocked(count)).unwrap();
    }

    let started = std::time::Instant::now();
    db.commit(&mut note).await.unwrap();
    let took = started.elapsed();

    let sql = "SELECT count(*), sum(payload), min(event_id IS NOT NULL) FROM sofie_messages
        WHERE kind = 'job' AND name = 'notes.restock'";
    let jobs = sofie_db::query_values(&file.open(), sql, &[])
        .await
        .unwrap();
    let total = (0..EVENTS).sum::<i64>();
    assert_eq!(
        jobs,
        [[
            SqlValue::Int(EVENTS),
            SqlValue::Int(total),
            SqlValue::Int(1)
        ]]
    );
    assert!(took < Duration::from_secs(1), "{took:?}");
}

static JOB_REACH: std::sync::Mutex<Option<Reach>> = std::sync::Mutex::new(None);

struct JobReach(Reach);

impl sofie_db::Resolve<sofie_db::worker::JobContext> for JobReach {
    fn resolve(
        context: &mut sofie_db::worker::JobContext,
    ) -> impl Future<Output = Result<Self>> + Send {
        use sofie_db::Context;
        std::future::ready(Ok(Self(context.reach())))
    }
}

async fn note_reach(reach: JobReach) -> Result<()> {
    *JOB_REACH.lock().unwrap() = Some(reach.0);
    Ok(())
}

#[tokio::test]
async fn a_job_queued_by_a_member_s_action_runs_as_that_member() {
    let file = File::new("actor");
    let db = migrated_in(file.open()).await;
    let member = Uuid::now_v7().unwrap();
    let now = db.clock().now();
    let mut queued =
        sofie_db::Jobs::queue_named_at("notes.note_reach", &serde_json::json!({}), now).unwrap();
    db.commit_with(&mut queued, Vec::new(), Reach::AllAs(member))
        .await
        .unwrap();
    let stored = sofie_db::query_values(&file.open(), "SELECT actor FROM sofie_messages", &[])
        .await
        .unwrap();

    Worker::new(db)
        .job("notes.note_reach", note_reach)
        .drain()
        .await
        .unwrap();

    assert_eq!(stored, [[SqlValue::Uuid(member)]]);
    assert_eq!(*JOB_REACH.lock().unwrap(), Some(Reach::AllAs(member)));
}

static NIGHTLY_RUNS: AtomicUsize = AtomicUsize::new(0);

async fn nightly() -> Result<()> {
    NIGHTLY_RUNS.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

#[tokio::test]
async fn a_wall_clock_schedule_runs_once_per_occurrence_across_workers() {
    let file = File::new("wall-clock");
    let clock = sofie_core::TestClock::starting_at(
        "2026-10-24T12:00:00Z"
            .parse::<sofie_core::Timestamp>()
            .unwrap(),
    );
    let at = sofie_db::worker::WallClock::daily(
        sofie_values::TimeOfDay::constant("02:30"),
        "Europe/Berlin",
    );
    let worker =
        |db: Db| Worker::new(db.with_clock(clock.clock())).every("notes.nightly", at, nightly);
    let first = worker(migrated_in(file.open()).await);
    let second = worker(Db::new(file.open()));

    assert_eq!(
        first.drain().await.unwrap() + second.drain().await.unwrap(),
        0
    );
    clock.advance(Duration::from_mins(13 * 60 + 45)).unwrap();
    let ran = first.drain().await.unwrap() + second.drain().await.unwrap();

    assert_eq!(
        ran, 1,
        "02:30 shows twice in Berlin on 2026-10-25 and runs once"
    );
    assert_eq!(NIGHTLY_RUNS.load(Ordering::SeqCst), 1);
    let sql = "SELECT next_at FROM sofie_schedules WHERE name = 'notes.nightly'";
    let next = sofie_db::query_values(&file.open(), sql, &[])
        .await
        .unwrap();
    let expected = "2026-10-26T01:30:00Z".parse().unwrap();
    assert_eq!(next, [[SqlValue::Timestamp(expected)]]);
}

#[tokio::test]
async fn a_dead_job_is_replayed_once_with_its_attempts_reset() {
    let file = File::new("replay");
    let db = migrated_in(file.open()).await;
    let messages = sofie_db::DeadMessages::new(db.clone());
    let id = Uuid::now_v7().unwrap();
    let queue = "INSERT INTO sofie_messages (id, kind, name, payload, available_at) \
                 VALUES (?1, 'job', 'notes.file', '{}', ?2)";
    let now = sofie_core::Clock::system().now().timestamp();
    sofie_db::query_values(
        &file.open(),
        queue,
        &[SqlValue::Uuid(id), SqlValue::Timestamp(now)],
    )
    .await
    .unwrap();
    Worker::new(db.clone())
        .job("notes.file", refuse)
        .drain()
        .await
        .unwrap();

    let dead = messages.newest(10).await.unwrap();
    let replayed = messages.replay(id).await.unwrap();
    let again = messages.replay(id).await.unwrap_err();

    assert_eq!(dead.len(), 1);
    assert_eq!((replayed.id, replayed.attempts), (id, 1));
    assert!(again.to_string().contains("is not dead"), "{again}");
    assert_eq!(
        left_messages(&file).await[0][..2],
        [SqlValue::Int(0), SqlValue::Int(0)]
    );
}

async fn report() -> Result<()> {
    Ok(())
}

#[tokio::test]
async fn a_schedule_whose_cadence_changed_starts_over_from_its_new_first_time() {
    let file = File::new("cadence");
    let clock = sofie_core::TestClock::starting_at(
        "2026-10-24T12:00:00Z"
            .parse::<sofie_core::Timestamp>()
            .unwrap(),
    );
    let worker = |db: Db, every: Duration| {
        Worker::new(db.with_clock(clock.clock())).every("notes.report", every, report)
    };
    let daily = worker(migrated_in(file.open()).await, Duration::from_hours(24));
    assert_eq!(daily.drain().await.unwrap(), 1);
    clock.advance(Duration::from_hours(2)).unwrap();

    let hourly = worker(Db::new(file.open()), Duration::from_hours(1));
    let ran_hourly = hourly.drain().await.unwrap();
    let restarted = worker(Db::new(file.open()), Duration::from_hours(1));
    let ran_again = restarted.drain().await.unwrap();

    assert_eq!((ran_hourly, ran_again), (1, 0));
}
