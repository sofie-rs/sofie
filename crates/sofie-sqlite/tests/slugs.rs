use sofie_core::{Cast, ColumnReader, Error, Result, SqlValue};
use sofie_db::{
    Aggregate, Behavior, Db, Field, Meta, Migrations, Select, Slugged, SoftDeletes, Uses,
};
use sofie_sqlite::Sqlite;

const ARTICLES: &str = "CREATE TABLE press_article (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    slug text NOT NULL UNIQUE,
    deleted_at timestamptz,
    title text NOT NULL,
    body text NOT NULL
)";

const MIGRATIONS: Migrations = Migrations::new(
    "press",
    &[sofie_db::File::new(
        "0001_articles.sql",
        ARTICLES.as_bytes(),
    )],
);

const SLUGGED_EARLIER: Migrations = Migrations::new(
    "press",
    &[
        sofie_db::File::new("0001_articles.sql", ARTICLES.as_bytes()),
        sofie_db::File::new(
            "0002_slugged_earlier.sql",
            "UPDATE press_article SET slug = 'al' WHERE title = 'Şal'".as_bytes(),
        ),
    ],
);

struct Article {
    meta: Meta,
    title: String,
    body: String,
}

impl Aggregate for Article {
    const NAME: &'static str = "article";
    const MODULE: &'static str = "press";
    const FIELDS: &'static [Field] = &[
        Field::new("title", String::COLUMNS, false),
        Field::new("body", String::COLUMNS, false),
    ];
    const BEHAVIORS: &'static [&'static dyn Behavior] =
        &[&Slugged { from: "title" }, &sofie_db::SoftDelete];

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.title.encode(out);
        self.body.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            title: String::decode(row)?,
            body: String::decode(row)?,
        })
    }
}

impl Uses<Slugged> for Article {}
impl Uses<sofie_db::SoftDelete> for Article {}

async fn written(db: &Db, title: &str) -> Article {
    let mut article = Article {
        meta: Meta::new().unwrap(),
        title: title.to_owned(),
        body: String::new(),
    };
    db.commit(&mut article).await.unwrap();
    article
}

async fn retitled(db: &Db, article: &mut Article, title: &str) {
    article.title = title.to_owned();
    db.commit(article).await.unwrap();
}

async fn by_slug(db: &Db, slug: &str) -> Result<String> {
    let found = Article::query().by_slug(&db.read(), slug).await?;
    Ok(found.title)
}

async fn press() -> Db {
    let db = Db::new(Sqlite::memory().unwrap());
    db.schema().migrate(&[MIGRATIONS]).await.unwrap();
    db
}

#[tokio::test]
async fn previous_slugs_move_to_the_current_one_until_taken_or_deleted() {
    let db = press().await;
    let mut lunch = written(&db, "Öğle Yemeği").await;
    retitled(&db, &mut lunch, "Akşam Yemeği").await;
    retitled(&db, &mut lunch, "Kahvaltı").await;
    let moved = by_slug(&db, "ogle-yemegi").await;

    let mut taken = written(&db, "Akşam Yemeği").await;
    let reused = by_slug(&db, "aksam-yemegi").await.unwrap();
    taken.delete();
    db.commit(&mut taken).await.unwrap();
    lunch.delete();
    db.commit(&mut lunch).await.unwrap();

    assert!(
        matches!(&moved, Err(Error::Moved { from, to }) if from == "ogle-yemegi" && to == "kahvalti"),
        "{moved:?}"
    );
    assert_eq!(reused, "Akşam Yemeği");
    assert!(matches!(
        by_slug(&db, "aksam-yemegi").await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        by_slug(&db, "ogle-yemegi").await,
        Err(Error::NotFound)
    ));
}

#[tokio::test]
async fn a_title_another_record_slugs_to_is_refused_under_the_title() {
    let db = press().await;
    written(&db, "Lunch").await;
    let mut again = Article {
        meta: Meta::new().unwrap(),
        title: "LUNCH!".to_owned(),
        body: String::new(),
    };

    let refused = db.commit(&mut again).await;

    match refused {
        Err(Error::Invalid(fields)) => assert_eq!(fields[0].field, "title"),
        other => panic!("expected the title refused, got {other:?}"),
    }
}

#[tokio::test]
async fn an_unchanged_title_keeps_its_slug_however_it_would_slug_now() {
    let db = press().await;
    written(&db, "Şal").await;
    db.schema().migrate(&[SLUGGED_EARLIER]).await.unwrap();
    written(&db, "Sal").await;
    let mut shawl = Article::query().by_slug(&db.read(), "al").await.unwrap();

    shawl.body = "Wool".to_owned();
    let committed = db.commit(&mut shawl).await;

    assert!(committed.is_ok(), "{committed:?}");
    assert_eq!(by_slug(&db, "al").await.unwrap(), "Şal");
}

#[tokio::test]
async fn a_title_without_a_letter_or_digit_is_refused_under_the_title() {
    let db = press().await;
    let mut party = Article {
        meta: Meta::new().unwrap(),
        title: "🎉 !?".to_owned(),
        body: String::new(),
    };

    let refused = db.commit(&mut party).await;

    match refused {
        Err(Error::Invalid(fields)) => assert_eq!(fields[0].field, "title"),
        other => panic!("expected the title refused, got {other:?}"),
    }
}
