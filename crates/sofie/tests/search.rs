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
