use sofie_core::{
    Cast, Clock, ColumnReader, Error, Instant, Result, SqlValue, TestClock, Timestamp,
};
use sofie_db::{
    Aggregate, Behavior, Column, Condition, Db, Field, File, Load, Meta, Migrations, Page, Select,
    SoftDelete, SoftDeletes, Timestamped, Timestamps, Uses,
};
use sofie_postgres::Postgres;
use sofie_testing::TestDatabase;
use sofie_values::Operator;
#[cfg(feature = "raw-sql")]
use {
    sofie_core::Uuid,
    sofie_db::{Reach, Read},
};

const CREATE_BOOKS: &str = "CREATE TABLE library.book (
    id uuid PRIMARY KEY,
    version int8 NOT NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    deleted_at timestamptz,
    title text NOT NULL,
    pages int8 NOT NULL
)";

struct Book {
    meta: Meta,
    title: String,
    pages: i64,
}

impl Book {
    const TITLE: Column<Self, String> = Column::new("title");
    const PAGES: Column<Self, i64> = Column::new("pages");

    fn new(title: &str, pages: i64) -> Self {
        Self {
            meta: Meta::new().unwrap(),
            title: title.to_owned(),
            pages,
        }
    }
}

impl Aggregate for Book {
    const NAME: &'static str = "book";
    const MODULE: &'static str = "library";
    const FIELDS: &'static [Field] = &[
        Field::new("title", String::COLUMNS, false),
        Field::new("pages", i64::COLUMNS, false),
    ];
    const BEHAVIORS: &'static [&'static dyn Behavior] = &[&Timestamps, &SoftDelete];

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.title.encode(out);
        self.pages.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            title: String::decode(row)?,
            pages: i64::decode(row)?,
        })
    }
}

impl Uses<Timestamps> for Book {}
impl Uses<SoftDelete> for Book {}

#[cfg(feature = "raw-sql")]
const SELECT_BOOK: &str = "SELECT id, version, created_at, updated_at, deleted_at, title, pages \
                           FROM library.book WHERE pages > $1 ORDER BY pages";

async fn library(clock: Clock) -> (TestDatabase, Db) {
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap()).with_clock(clock);
    // With `module-roles`, raw SQL runs as the module's role.
    let role = include_str!("../module_role.sql").replace("{module}", "library");
    let files = [
        File::new("0000_module_role.sql", String::leak(role).as_bytes()),
        File::new("0001_books.sql", CREATE_BOOKS.as_bytes()),
    ];
    db.schema()
        .migrate(&[Migrations::new("library", Vec::leak(files.to_vec()))])
        .await
        .unwrap();
    (database, db)
}

async fn shelved(db: &Db, books: &[(&str, i64)]) -> Vec<Book> {
    let mut books: Vec<Book> = books
        .iter()
        .map(|(title, pages)| Book::new(title, *pages))
        .collect();
    db.commit(&mut books).await.unwrap();
    books
}

fn titles(books: &[Book]) -> Vec<&str> {
    books.iter().map(|book| book.title.as_str()).collect()
}

#[tokio::test]
async fn timestamps_record_the_commits_that_created_and_last_changed_a_record() {
    let created = Instant::from(Timestamp::from_second(1_000_000).unwrap());
    let clock = TestClock::starting_at(created);
    let (_database, db) = library(clock.clock()).await;
    let book = shelved(&db, &[("Dune", 412)]).await.remove(0);
    clock.advance(std::time::Duration::from_secs(60)).unwrap();

    let mut loaded = Book::find(&db.read(), book.id()).await.unwrap();
    loaded.pages = 413;
    db.commit(&mut loaded).await.unwrap();
    let reloaded = Book::find(&db.read(), book.id()).await.unwrap();

    assert_eq!(book.created_at(), Some(created));
    assert_eq!(reloaded.created_at(), Some(created));
    assert_eq!(reloaded.updated_at(), Some(clock.clock().now()));
    assert_eq!(reloaded.meta().version(), 2);
    let since = Book::query()
        .where_(Book::UPDATED_AT.gt(created))
        .count(&db.read())
        .await;
    assert_eq!(since.unwrap(), 1);
}

#[tokio::test]
async fn a_deleted_record_is_hidden_until_asked_for_and_restored() {
    let (_database, db) = library(Clock::system()).await;
    let book = shelved(&db, &[("Dune", 412), ("Emma", 474)])
        .await
        .remove(0);
    let mut loaded = Book::find(&db.read(), book.id()).await.unwrap();

    loaded.delete();
    db.commit(&mut loaded).await.unwrap();

    let read = db.read();
    assert!(matches!(
        Book::find(&read, book.id()).await,
        Err(Error::NotFound)
    ));
    assert_eq!(Book::query().count(&read).await.unwrap(), 1);
    let mut trashed = Book::query()
        .with_trashed()
        .where_(Book::ID.eq(book.id()))
        .first(&read)
        .await
        .unwrap()
        .unwrap();
    assert!(trashed.is_deleted() && trashed.deleted_at().is_some());
    trashed.restore();
    db.commit(&mut trashed).await.unwrap();
    assert_eq!(Book::find(&read, book.id()).await.unwrap().title, "Dune");
}

#[tokio::test]
async fn conditions_match_ranges_lists_patterns_and_existence() {
    let (_database, db) = library(Clock::system()).await;
    shelved(&db, &[("Dune", 412), ("Emma", 474), ("Dracula", 418)]).await;
    let read = db.read();

    let between = Book::query()
        .where_(Book::PAGES.between(412, 418))
        .order_by(Book::PAGES.asc())
        .all(&read)
        .await
        .unwrap();
    let listed = Book::query()
        .where_(Book::TITLE.in_(["Emma".to_owned(), "Ulysses".to_owned()]))
        .all(&read)
        .await
        .unwrap();
    let none_listed = Book::query().where_(Book::TITLE.in_([])).count(&read).await;
    let by_pages = Book::query()
        .where_(Book::PAGES.in_([412, 418, 1]))
        .count(&read)
        .await;
    let starting_with_d = Book::query()
        .where_(Book::TITLE.like("D%"))
        .count(&read)
        .await;

    assert_eq!(titles(&between), ["Dune", "Dracula"]);
    assert_eq!(titles(&listed), ["Emma"]);
    assert_eq!(none_listed.unwrap(), 0);
    assert_eq!(by_pages.unwrap(), 2);
    assert_eq!(starting_with_d.unwrap(), 2);
    let long = Book::query().where_(Book::PAGES.gt(470));
    assert!(long.exists(&read).await.unwrap());
    let longer = Book::query().where_(Book::PAGES.gt(500));
    assert!(!longer.exists(&read).await.unwrap());
}

#[tokio::test]
async fn pages_follow_one_another_and_tell_whether_another_follows() {
    let (_database, db) = library(Clock::system()).await;
    shelved(&db, &[("A", 1), ("B", 2), ("C", 3), ("D", 4), ("E", 5)]).await;
    let read = db.read();
    let page = |number| {
        Book::query()
            .order_by(Book::PAGES.asc())
            .paginate(&read, Page::new(number, 2).unwrap())
    };

    let second = page(2).await.unwrap();
    let last = page(3).await.unwrap();

    assert_eq!(titles(second.items()), ["C", "D"]);
    assert!(second.has_next() && second.has_previous());
    assert_eq!(titles(last.items()), ["E"]);
    assert!(!last.has_next());
}

#[tokio::test]
async fn pages_sorted_by_a_tied_field_hold_each_record_once() {
    let (_database, db) = library(Clock::system()).await;
    let books: Vec<(String, i64)> = (0..60)
        .map(|number| (format!("B{number}"), number % 2))
        .collect();
    let borrowed: Vec<(&str, i64)> = books
        .iter()
        .map(|(title, pages)| (title.as_str(), *pages))
        .collect();
    shelved(&db, &borrowed).await;

    let mut seen = Vec::new();
    for number in 1..=20 {
        let page = Book::query()
            .order_by(Book::PAGES.asc())
            .paginate(&db.read(), Page::new(number, 3).unwrap())
            .await
            .unwrap();
        seen.extend(page.items().iter().map(|book| book.title.clone()));
    }

    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 60);
}

#[tokio::test]
async fn a_list_filtered_by_contained_text_ignores_capitals() {
    let (_database, db) = library(Clock::system()).await;
    shelved(&db, &[("Dune Messiah", 256), ("Emma", 474)]).await;

    let contained = Condition::on_field("title", Operator::Contains, "messiah").unwrap();
    let found = Book::query()
        .where_(contained)
        .all(&db.read())
        .await
        .unwrap();

    assert_eq!(titles(&found), ["Dune Messiah"]);
}

#[tokio::test]
async fn a_list_filtered_from_and_up_to_a_value_holds_both_ends() {
    let (_database, db) = library(Clock::system()).await;
    shelved(&db, &[("Dune", 412), ("Emma", 474), ("Ulysses", 730)]).await;
    let read = db.read();
    let count = |operator, pages| {
        let condition = Condition::on_field("pages", operator, pages).unwrap();
        Book::query().where_(condition).count(&read)
    };

    assert_eq!(count(Operator::AtLeast, "474").await.unwrap(), 2);
    assert_eq!(count(Operator::AtMost, "474").await.unwrap(), 2);
    assert_eq!(Operator::named("gte"), Some(Operator::AtLeast));
    assert_eq!(Operator::named("lte"), Some(Operator::AtMost));
}

#[tokio::test]
async fn a_stream_reads_every_matching_record_across_batches() {
    let (_database, db) = library(Clock::system()).await;
    let books: Vec<(String, i64)> = (0..1_201)
        .map(|number| (format!("B{number}"), number))
        .collect();
    let borrowed: Vec<(&str, i64)> = books
        .iter()
        .map(|(title, pages)| (title.as_str(), *pages))
        .collect();
    shelved(&db, &borrowed).await;

    let mut stream = Book::query()
        .where_(Book::PAGES.gte(100))
        .stream(&db.read())
        .unwrap();
    let mut pages = Vec::new();
    while let Some(book) = stream.next().await.unwrap() {
        pages.push(book.pages);
    }

    assert_eq!(pages, (100..1_201).collect::<Vec<_>>());
    assert!(Book::query().latest().stream(&db.read()).is_err());
}

#[cfg(feature = "raw-sql")]
#[tokio::test]
async fn raw_sql_binds_its_parameters_and_decodes_like_the_builder() {
    let (_database, db) = library(Clock::system()).await;
    shelved(&db, &[("Dune", 412), ("Emma", 474)]).await;

    let long = db
        .read()
        .sql::<Book>(SELECT_BOOK)
        .bind(&420_i64)
        .all()
        .await;

    assert_eq!(titles(&long.unwrap()), ["Emma"]);
}

#[cfg(feature = "raw-sql")]
#[tokio::test]
async fn raw_sql_cannot_write_or_be_used_in_an_owned_area() {
    let (_database, db) = library(Clock::system()).await;
    shelved(&db, &[("Dune", 412)]).await;
    let owned = Read::within(&db, Reach::OwnedBy(Uuid::now_v7().unwrap()));

    let write = db
        .read()
        .sql::<Book>("DELETE FROM library.book RETURNING id, version, created_at, updated_at, deleted_at, title, pages")
        .all()
        .await;
    let scoped = owned.sql::<Book>(SELECT_BOOK).bind(&0_i64).all().await;

    assert!(
        matches!(&write, Err(Error::Internal { context, .. }) if context.contains("read-only")),
        "{:?}",
        write.err()
    );
    assert!(matches!(scoped, Err(Error::Internal { .. })));
    assert_eq!(Book::query().count(&db.read()).await.unwrap(), 1);
}
