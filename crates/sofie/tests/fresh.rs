use sofie::core::SqlValue;
use sofie::db::{Db, File, Migrations, query_values};
use sofie::postgres::Postgres;
use sofie_testing::TestDatabase;

const CREATE_ITEMS: File = File::new(
    "0001_create_items.sql",
    b"CREATE TABLE items (id int8 PRIMARY KEY);",
);
const SHOP: Migrations = Migrations::new("shop", &[CREATE_ITEMS]);

#[tokio::test]
async fn fresh_drops_what_the_migrations_made_and_migrates_again() {
    let database = TestDatabase::create().await;
    let postgres = Postgres::connect(database.url()).unwrap();
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    db.schema().migrate(&[SHOP]).await.unwrap();
    query_values(&postgres, "INSERT INTO shop.items VALUES (1)", &[])
        .await
        .unwrap();
    query_values(&postgres, "CREATE TABLE public.kept (id int8)", &[])
        .await
        .unwrap();

    db.schema().fresh(&[SHOP]).await.unwrap();

    let count = |sql: &'static str| {
        let postgres = &postgres;
        async move { query_values(postgres, sql, &[]).await.unwrap() }
    };
    assert_eq!(
        count("SELECT count(*) FROM shop.items").await,
        [[SqlValue::Int(0)]]
    );
    assert_eq!(
        count("SELECT count(*) FROM sofie.migrations WHERE module = 'shop'").await,
        [[SqlValue::Int(1)]]
    );
    assert_eq!(
        count("SELECT count(*) FROM public.kept").await,
        [[SqlValue::Int(0)]],
        "a schema no migration made is left alone"
    );
}
