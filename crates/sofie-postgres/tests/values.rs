use jiff::Timestamp;
use jiff::civil::date;
use sofie_core::{Cast, ColumnReader, Error, SqlType, SqlValue, Uuid};
use sofie_db::{Driver, ExpectedRows, Step, query_decoded};
use sofie_postgres::Postgres;
use sofie_testing::TestDatabase;

const CREATE_TABLE: &str = "CREATE TABLE every_type (
    small int2, regular int4, big int8,
    single real, double float8,
    text text, varchar varchar(20), bpchar char(3), name name,
    flag bool, blob bytea, uuid uuid, at timestamptz, day date, local timestamp
)";

const INSERT: &str = "INSERT INTO every_type VALUES
    ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)";

const SELECT: &str = "SELECT small, regular, big, single, double, text, varchar, bpchar, name,
    flag, blob, uuid, at, day, local FROM every_type";

async fn database_with_table() -> (TestDatabase, Postgres) {
    let database = TestDatabase::create().await;
    let postgres = Postgres::connect(database.url()).unwrap();
    postgres
        .transaction(vec![Step::Script(CREATE_TABLE.to_owned())])
        .await
        .unwrap();
    (database, postgres)
}

async fn insert(postgres: &Postgres, params: Vec<SqlValue>) -> sofie_core::Result<()> {
    postgres
        .transaction(vec![Step::Statement {
            sql: INSERT.to_owned(),
            params,
            expect_rows: Some(ExpectedRows::or_conflict(1)),
        }])
        .await
}

fn row_of_every_type(days: (i16, i8, i8), at: &str) -> Vec<SqlValue> {
    vec![
        SqlValue::Int(i64::from(i16::MIN)),
        SqlValue::Int(i64::from(i32::MAX)),
        SqlValue::Int(i64::MIN),
        SqlValue::Float(1.5),
        SqlValue::Float(std::f64::consts::PI),
        SqlValue::Text("it's \"quoted\" ✓".to_owned()),
        SqlValue::Text("varying".to_owned()),
        SqlValue::Text("abc".to_owned()),
        SqlValue::Text("a_name".to_owned()),
        SqlValue::Bool(true),
        SqlValue::Bytes(vec![0, 1, 254, 255]),
        SqlValue::Uuid(Uuid::now_v7().unwrap()),
        SqlValue::Timestamp(at.parse::<Timestamp>().unwrap()),
        SqlValue::Date(date(days.0, days.1, days.2)),
        SqlValue::DateTime(date(days.0, days.1, days.2).at(9, 30, 15, 500_000_000)),
    ]
}

#[tokio::test]
async fn every_value_round_trips_through_its_column_type() {
    let (_database, postgres) = database_with_table().await;
    let after_epoch = row_of_every_type((2026, 10, 10), "2026-10-10T12:34:56.123456Z");
    let before_epoch = row_of_every_type((1999, 12, 31), "1969-07-20T20:17:40.000001Z");

    insert(&postgres, after_epoch.clone()).await.unwrap();
    insert(&postgres, before_epoch.clone()).await.unwrap();

    let rows = sofie_db::query_values(&postgres, &format!("{SELECT} ORDER BY day DESC"), &[])
        .await
        .unwrap();
    assert_eq!(rows, vec![after_epoch, before_epoch]);
}

#[tokio::test]
async fn nulls_round_trip_in_every_column_type() {
    let (_database, postgres) = database_with_table().await;
    let nulls = vec![SqlValue::Null; 15];

    insert(&postgres, nulls.clone()).await.unwrap();

    assert_eq!(
        sofie_db::query_values(&postgres, SELECT, &[])
            .await
            .unwrap(),
        vec![nulls]
    );
}

#[tokio::test]
async fn parameters_bind_in_conditions() {
    let (_database, postgres) = database_with_table().await;
    let row = row_of_every_type((2026, 1, 2), "2026-01-02T03:04:05Z");
    insert(&postgres, row.clone()).await.unwrap();

    let found = sofie_db::query_values(
        &postgres,
        &format!("{SELECT} WHERE uuid = $1 AND day = $2 AND at = $3"),
        &[row[11].clone(), row[13].clone(), row[12].clone()],
    )
    .await
    .unwrap();

    assert_eq!(found, vec![row]);
}

#[tokio::test]
async fn an_integer_out_of_the_column_range_is_refused() {
    let (_database, postgres) = database_with_table().await;
    let mut row = vec![SqlValue::Null; 15];
    row[0] = SqlValue::Int(i64::from(i16::MAX) + 1);

    let result = insert(&postgres, row).await;

    assert!(matches!(result, Err(Error::Internal { .. })), "{result:?}");
}

#[tokio::test]
async fn a_value_of_the_wrong_type_for_its_column_is_refused() {
    let (_database, postgres) = database_with_table().await;
    let mut row = vec![SqlValue::Null; 15];
    row[11] = SqlValue::Text("not a uuid".to_owned());

    let result = insert(&postgres, row).await;

    assert!(matches!(result, Err(Error::Internal { .. })), "{result:?}");
}

#[tokio::test]
async fn an_unsupported_column_type_is_an_error_naming_it() {
    let database = TestDatabase::create().await;
    let postgres = Postgres::connect(database.url()).unwrap();

    let error = sofie_db::query_values(&postgres, "SELECT 1.5::numeric AS amount", &[])
        .await
        .unwrap_err();

    assert!(error.to_string().contains("numeric"), "{error}");
}

type Decoded = (
    (i64, i32, i64, f64, f64),
    (String, String, String, String),
    (
        bool,
        Vec<u8>,
        Uuid,
        Timestamp,
        jiff::civil::Date,
        jiff::civil::DateTime,
    ),
);

fn decode_every_type(row: &mut dyn ColumnReader) -> sofie_core::Result<Option<Decoded>> {
    if row.is_null()? {
        for _ in 0..15 {
            row.skip()?;
        }
        return Ok(None);
    }
    let numbers = (
        i64::decode(row)?,
        i32::decode(row)?,
        i64::decode(row)?,
        f64::decode(row)?,
        f64::decode(row)?,
    );
    let texts = (
        String::decode(row)?,
        String::decode(row)?,
        String::decode(row)?,
        String::decode(row)?,
    );
    let rest = (
        bool::decode(row)?,
        row.bytes()?.to_vec(),
        Uuid::decode(row)?,
        Timestamp::decode(row)?,
        jiff::civil::Date::decode(row)?,
        jiff::civil::DateTime::decode(row)?,
    );
    Ok(Some((numbers, texts, rest)))
}

#[tokio::test]
async fn every_column_type_decodes_straight_into_its_target() {
    let (_database, postgres) = database_with_table().await;
    let row = row_of_every_type((2026, 10, 10), "2026-10-10T12:34:56.123456Z");
    insert(&postgres, row.clone()).await.unwrap();
    insert(&postgres, vec![SqlValue::Null; 15]).await.unwrap();
    let types = vec![
        Some(SqlType::Int),
        Some(SqlType::Int),
        Some(SqlType::Int),
        Some(SqlType::Float),
        Some(SqlType::Float),
        Some(SqlType::Text),
        Some(SqlType::Text),
        Some(SqlType::Text),
        Some(SqlType::Text),
        Some(SqlType::Bool),
        Some(SqlType::Bytes),
        Some(SqlType::Uuid),
        Some(SqlType::Timestamp),
        Some(SqlType::Date),
        Some(SqlType::DateTime),
    ];

    let sql = format!("{SELECT} ORDER BY small NULLS LAST");
    let decoded = query_decoded(&postgres, &sql, &[], types, decode_every_type)
        .await
        .unwrap();

    let SqlValue::Uuid(uuid) = row[11] else {
        unreachable!()
    };
    let expected = (
        (
            i64::from(i16::MIN),
            i32::MAX,
            i64::MIN,
            1.5,
            std::f64::consts::PI,
        ),
        (
            "it's \"quoted\" ✓".to_owned(),
            "varying".to_owned(),
            "abc".to_owned(),
            "a_name".to_owned(),
        ),
        (
            true,
            vec![0, 1, 254, 255],
            uuid,
            "2026-10-10T12:34:56.123456Z".parse().unwrap(),
            date(2026, 10, 10),
            date(2026, 10, 10).at(9, 30, 15, 500_000_000),
        ),
    );
    assert_eq!(decoded, [Some(expected), None]);
}

#[tokio::test]
async fn options_decode_null_as_none_and_a_value_as_some() {
    let database = TestDatabase::create().await;
    let postgres = Postgres::connect(database.url()).unwrap();
    let types = vec![Some(SqlType::Text), Some(SqlType::Int)];

    let decoded = query_decoded(&postgres, "SELECT NULL::text, 7::int8", &[], types, |row| {
        Ok((Option::<String>::decode(row)?, Option::<i64>::decode(row)?))
    })
    .await
    .unwrap();

    assert_eq!(decoded, [(None, Some(7))]);
}

#[tokio::test]
async fn a_column_of_another_type_is_refused_before_any_row_is_read() {
    let database = TestDatabase::create().await;
    let postgres = Postgres::connect(database.url()).unwrap();
    let mut decoded_rows = 0;

    let refused = query_decoded(
        &postgres,
        "SELECT $1::text AS label FROM generate_series(1, 3)",
        &[SqlValue::Text("secret".to_owned())],
        vec![Some(SqlType::Int)],
        |row| {
            decoded_rows += 1;
            i64::decode(row)
        },
    )
    .await
    .unwrap_err();

    assert_eq!(decoded_rows, 0);
    let message = refused.to_string();
    assert!(matches!(refused, Error::Internal { .. }), "{message}");
    assert!(
        message.contains("label") && message.contains("text") && message.contains("Int"),
        "{message}"
    );
    assert!(!message.contains("secret"), "{message}");
}

#[tokio::test]
async fn a_statement_with_fewer_columns_than_its_reader_expects_is_refused() {
    let database = TestDatabase::create().await;
    let postgres = Postgres::connect(database.url()).unwrap();
    let types = vec![Some(SqlType::Int), Some(SqlType::Int)];

    let refused = query_decoded(&postgres, "SELECT 1::int8", &[], types, |row| {
        Ok((i64::decode(row)?, i64::decode(row)?))
    })
    .await;

    assert!(
        matches!(refused, Err(Error::Internal { .. })),
        "{refused:?}"
    );
}

#[tokio::test]
async fn reading_past_the_last_column_is_refused() {
    let database = TestDatabase::create().await;
    let postgres = Postgres::connect(database.url()).unwrap();

    let refused = query_decoded(&postgres, "SELECT 1::int8", &[], Vec::new(), |row| {
        Ok((i64::decode(row)?, i64::decode(row)?))
    })
    .await;

    assert!(
        matches!(refused, Err(Error::Internal { .. })),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_case_insensitive_text_column_reads_back_as_text() {
    let database = TestDatabase::create().await;
    let postgres = Postgres::connect(database.url()).unwrap();
    postgres
        .transaction(vec![Step::Script(
            "CREATE EXTENSION IF NOT EXISTS citext; CREATE TABLE emails (address citext)"
                .to_owned(),
        )])
        .await
        .unwrap();
    postgres
        .transaction(vec![Step::Statement {
            sql: "INSERT INTO emails VALUES ($1)".to_owned(),
            params: vec![SqlValue::Text("Ada@Example.com".to_owned())],
            expect_rows: Some(ExpectedRows::or_conflict(1)),
        }])
        .await
        .unwrap();

    let read = query_decoded(
        &postgres,
        "SELECT address FROM emails WHERE address = 'ada@example.com'",
        &[],
        vec![Some(SqlType::Text)],
        |row| row.text().map(str::to_owned),
    )
    .await
    .unwrap();

    assert_eq!(read, ["Ada@Example.com"]);
    let columns = sofie_db::Catalog::columns(&postgres, "public", "emails")
        .await
        .unwrap();
    assert_eq!(columns[0].ty, Some(SqlType::Text));
}
