#![cfg(feature = "search")]

use sofie::db::{Db, File, Migrations};
use sofie::postgres::Postgres;
use sofie::prelude::*;
use sofie_testing::TestDatabase;

const MIGRATIONS: &[File] = &[File::new(
    "0001_product.sql",
    b"CREATE TABLE product (
        id uuid PRIMARY KEY,
        version bigint NOT NULL,
        created_at timestamptz NOT NULL,
        updated_at timestamptz NOT NULL,
        search_text text NOT NULL DEFAULT '',
        name text NOT NULL,
        contact text NOT NULL,
        price bigint NOT NULL,
        price_currency text NOT NULL,
        note text
    );
    CREATE INDEX product_search ON product USING gin (to_tsvector('simple', search_text));",
)];

#[derive(Aggregate)]
#[aggregate(behaviors(Searched))]
pub struct Product {
    meta: Meta,
    name: String,
    contact: Email,
    price: Money,
    note: Option<String>,
}

fn product(name: &str, contact: &str, minor: i64, note: Option<&str>) -> Product {
    Product {
        meta: Meta::new().unwrap(),
        name: name.to_owned(),
        contact: Email::parse(contact).unwrap(),
        price: Money::new(minor, Currency::parse("EUR").unwrap()),
        note: note.map(str::to_owned),
    }
}

async fn found(db: &Db, words: &str) -> Vec<String> {
    let mut names: Vec<String> = Product::query()
        .where_(Product::matching(words))
        .all(&db.read())
        .await
        .unwrap()
        .into_iter()
        .map(|product| product.name)
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn finds_records_by_the_words_and_whole_values_of_their_searched_fields() {
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    db.schema()
        .migrate(&[Migrations::new("search", MIGRATIONS)])
        .await
        .unwrap();
    let mut products = vec![
        product(
            "Blue wool scarf",
            "knit@example.com",
            4500,
            Some("hand made"),
        ),
        product("Red wool hat", "hats@example.com", 2000, None),
        product("Blue cotton shirt", "knit@example.com", 3000, None),
    ];
    db.commit(&mut products).await.unwrap();

    assert_eq!(
        found(&db, "wool").await,
        ["Blue wool scarf", "Red wool hat"]
    );
    assert_eq!(found(&db, "blue wool").await, ["Blue wool scarf"]);
    assert_eq!(found(&db, "wool -red").await, ["Blue wool scarf"]);
    assert_eq!(found(&db, "\"hand made\"").await, ["Blue wool scarf"]);
    assert_eq!(
        found(&db, "knit@example.com").await,
        ["Blue cotton shirt", "Blue wool scarf"]
    );
    assert!(found(&db, "4500").await.is_empty(), "a price is not a word");

    products[1].name = "Red linen hat".to_owned();
    db.commit(&mut products).await.unwrap();
    assert_eq!(found(&db, "wool").await, ["Blue wool scarf"]);
}

#[cfg(feature = "tasks")]
#[tokio::test]
async fn the_reindex_task_rewrites_every_page_of_text_written_before_its_fields_were_searched() {
    const TABLES: &[sofie::db::AggregateTable] = &[sofie::db::AggregateTable::of::<Product>()];
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    let migrations = [Migrations::new("search", MIGRATIONS).aggregates(TABLES)];
    db.schema().migrate(&migrations).await.unwrap();
    sofie::db::query_values(
        &Postgres::connect(database.url()).unwrap(),
        "INSERT INTO search.product
         SELECT gen_random_uuid(), 1, now(), now(), '', 'Old wool ' || n, 'old@example.com', 100, 'EUR'
         FROM generate_series(1, 600) AS n",
        &[],
    )
    .await
    .unwrap();
    assert_eq!(found(&db, "wool").await, Vec::<String>::new());

    let texts = sofie::db::SearchTexts::of(&migrations);
    sofie::Tasks::new()
        .task(sofie::db::REINDEX_TASK, move |read: Read| {
            let texts = texts.clone();
            async move { texts.reindex(read).await }
        })
        .run(&db, sofie::db::REINDEX_TASK, sofie::serde_json::Map::new())
        .await
        .unwrap();

    assert_eq!(found(&db, "wool").await.len(), 600);
    assert_eq!(found(&db, "old@example.com").await.len(), 600);
}

#[cfg(feature = "listing")]
async fn listed(
    listing: sofie::http::Listing<Product>,
    read: Read,
) -> Result<sofie::serde_json::Value> {
    let listed = listing.page(&read).await?;
    let mut names: Vec<&str> = listed
        .items()
        .iter()
        .map(|product| product.name.as_str())
        .collect();
    names.sort_unstable();
    Ok(sofie::serde_json::json!(names))
}

#[cfg(feature = "listing")]
#[tokio::test]
async fn a_list_of_a_searched_aggregate_matches_the_words_of_q() {
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    db.schema()
        .migrate(&[Migrations::new("search", MIGRATIONS)])
        .await
        .unwrap();
    let mut products = vec![
        product("Blue wool scarf", "knit@example.com", 4500, None),
        product("Red linen hat", "hats@example.com", 2000, None),
    ];
    db.commit(&mut products).await.unwrap();
    let app = sofie::http::App::new(db).area(
        "shop",
        |routes: &mut sofie::http::Routes<sofie::http::Web>| {
            routes.get("/products", listed);
        },
    );
    let server = sofie_testing::TestServer::start(app).await;

    let matched = server.get("/products?q=wool").await;
    let unsearched = server.get("/products?q=").await;

    assert_eq!(
        matched.json(),
        sofie::serde_json::json!(["Blue wool scarf"])
    );
    assert_eq!(
        unsearched.json(),
        sofie::serde_json::json!(["Blue wool scarf", "Red linen hat"])
    );
}
