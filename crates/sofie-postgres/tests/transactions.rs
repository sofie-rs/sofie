use sofie_core::{Error, SqlValue};
use sofie_db::{Driver, Step, UniqueViolation};
use sofie_postgres::Postgres;
use sofie_testing::TestDatabase;

async fn database_with_items() -> (TestDatabase, Postgres) {
    let database = TestDatabase::create().await;
    let postgres = Postgres::connect(database.url()).unwrap();
    postgres
        .transaction(vec![Step::Script(
            "CREATE TABLE items (id int8 PRIMARY KEY, label text NOT NULL)".to_owned(),
        )])
        .await
        .unwrap();
    (database, postgres)
}

fn insert_item(id: i64) -> Step {
    Step::Statement {
        sql: "INSERT INTO items (id, label) VALUES ($1, $2)".to_owned(),
        params: vec![SqlValue::Int(id), SqlValue::Text(format!("item {id}"))],
        expect_rows: None,
    }
}

fn rename_item(id: i64, expect_rows: Option<u64>) -> Step {
    Step::Statement {
        sql: "UPDATE items SET label = 'renamed' WHERE id = $1".to_owned(),
        params: vec![SqlValue::Int(id)],
        expect_rows: expect_rows.map(sofie_db::ExpectedRows::or_conflict),
    }
}

async fn item_count(postgres: &Postgres) -> Vec<Vec<SqlValue>> {
    sofie_db::query_values(postgres, "SELECT count(*) FROM items", &[])
        .await
        .unwrap()
}

#[tokio::test]
async fn every_step_commits_together() {
    let (_database, postgres) = database_with_items().await;

    postgres
        .transaction(vec![
            insert_item(1),
            insert_item(2),
            rename_item(2, Some(1)),
            Step::Script("UPDATE items SET label = label || '!'; SELECT 1".to_owned()),
        ])
        .await
        .unwrap();

    let rows = sofie_db::query_values(&postgres, "SELECT id, label FROM items ORDER BY id", &[])
        .await
        .unwrap();
    assert_eq!(
        rows,
        vec![
            vec![SqlValue::Int(1), SqlValue::Text("item 1!".to_owned())],
            vec![SqlValue::Int(2), SqlValue::Text("renamed!".to_owned())],
        ]
    );
}

#[tokio::test]
async fn a_row_count_other_than_expected_is_a_conflict_and_rolls_back() {
    let (_database, postgres) = database_with_items().await;

    let result = postgres
        .transaction(vec![insert_item(1), rename_item(99, Some(1))])
        .await;

    assert!(matches!(result, Err(Error::Conflict)), "{result:?}");
    assert_eq!(item_count(&postgres).await, vec![vec![SqlValue::Int(0)]]);
}

#[tokio::test]
async fn a_statement_without_an_expected_count_accepts_any() {
    let (_database, postgres) = database_with_items().await;

    postgres
        .transaction(vec![insert_item(1), rename_item(99, None)])
        .await
        .unwrap();

    assert_eq!(item_count(&postgres).await, vec![vec![SqlValue::Int(1)]]);
}

#[tokio::test]
async fn a_failing_statement_rolls_back_the_steps_before_it() {
    let (_database, postgres) = database_with_items().await;

    let result = postgres
        .transaction(vec![insert_item(1), insert_item(1)])
        .await;

    assert!(matches!(result, Err(Error::Internal { .. })), "{result:?}");
    assert_eq!(item_count(&postgres).await, vec![vec![SqlValue::Int(0)]]);
}

#[tokio::test]
async fn a_server_error_keeps_its_code_and_leaves_out_the_values_it_quotes() {
    let (_database, postgres) = database_with_items().await;

    let error = postgres
        .transaction(vec![insert_item(7), insert_item(7)])
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains("23505"), "{error}");
    assert!(!error.contains("(7)"), "{error}");
}

#[tokio::test]
async fn a_unique_violation_names_the_index_that_refused_it() {
    let (_database, postgres) = database_with_items().await;

    let error = postgres
        .transaction(vec![insert_item(3), insert_item(3)])
        .await
        .unwrap_err();

    let violation = std::error::Error::source(&error)
        .and_then(|source| source.downcast_ref::<UniqueViolation>())
        .unwrap_or_else(|| panic!("no unique violation in {error}"));
    assert_eq!(violation.index, "items_pkey");
}

#[tokio::test]
async fn a_parameter_that_does_not_encode_rolls_back_the_steps_sent_with_it() {
    let (_database, postgres) = database_with_items().await;
    let text_for_an_id = Step::Statement {
        sql: "INSERT INTO items (id, label) VALUES ($1, 'unsent')".to_owned(),
        params: vec![SqlValue::Text("one".to_owned())],
        expect_rows: None,
    };

    let result = postgres
        .transaction(vec![insert_item(1), text_for_an_id, insert_item(2)])
        .await;

    assert!(matches!(result, Err(Error::Internal { .. })), "{result:?}");
    assert_eq!(item_count(&postgres).await, vec![vec![SqlValue::Int(0)]]);
}

#[tokio::test]
async fn a_transaction_dropped_midway_neither_commits_nor_lends_its_connection() {
    // A counted step holds `COMMIT` back until its count is checked.
    let (_database, postgres) = database_with_items().await;
    let slow = Step::Script("SELECT pg_sleep(0.3)".to_owned());
    let dropped = tokio::time::timeout(
        std::time::Duration::from_millis(50),
        postgres.transaction(vec![insert_item(1), slow, rename_item(1, Some(1))]),
    )
    .await;

    postgres.transaction(vec![insert_item(2)]).await.unwrap();

    assert!(dropped.is_err(), "{dropped:?}");
    let rows = sofie_db::query_values(&postgres, "SELECT id FROM items", &[])
        .await
        .unwrap();
    assert_eq!(rows, vec![vec![SqlValue::Int(2)]]);
}

fn script(sql: &str) -> Step {
    Step::Script(sql.to_owned())
}

#[tokio::test]
async fn a_serialization_failure_or_a_deadlock_is_a_conflict() {
    let (_database, postgres) = database_with_items().await;

    for code in ["40001", "40P01"] {
        let raised = format!("DO $$ BEGIN RAISE EXCEPTION 'lost' USING ERRCODE = '{code}'; END $$");

        let result = postgres.transaction(vec![script(&raised)]).await;

        assert!(matches!(result, Err(Error::Conflict)), "{code}: {result:?}");
    }
}

#[tokio::test]
async fn a_server_error_leaves_out_its_detail() {
    let (_database, postgres) = database_with_items().await;
    // The detail is built by the server, so only the error, not the statement, could carry it.
    let raised = "DO $$ BEGIN RAISE EXCEPTION 'refused' \
                  USING DETAIL = 'card ' || (4000 + 111)::text; END $$";

    let error = postgres
        .transaction(vec![script(raised), rename_item(1, Some(1))])
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains("refused"), "{error}");
    assert!(!error.contains("4111"), "{error}");
}

#[tokio::test]
async fn a_unique_violation_deferred_to_commit_names_its_index() {
    let (_database, postgres) = database_with_items().await;
    postgres
        .transaction(vec![script(
            "CREATE TABLE codes (code text, CONSTRAINT codes_code UNIQUE (code) \
             DEFERRABLE INITIALLY DEFERRED)",
        )])
        .await
        .unwrap();

    let error = postgres
        .transaction(vec![script(
            "INSERT INTO codes VALUES ('secret'); INSERT INTO codes VALUES ('secret')",
        )])
        .await
        .unwrap_err();

    let violation = std::error::Error::source(&error)
        .and_then(|source| source.downcast_ref::<UniqueViolation>())
        .unwrap_or_else(|| panic!("no unique violation in {error}"));
    assert_eq!(violation.index, "codes_code");
    assert!(!error.to_string().contains("secret"), "{error}");
}

#[tokio::test]
async fn a_failed_rollback_does_not_hide_the_step_s_failure() {
    let (_database, postgres) = database_with_items().await;
    let lose_the_connection = Step::Statement {
        sql: "SELECT pg_terminate_backend(pg_backend_pid())".to_owned(),
        params: Vec::new(),
        expect_rows: None,
    };

    let result = postgres
        .transaction(vec![rename_item(99, Some(1)), lose_the_connection])
        .await;

    assert!(matches!(result, Err(Error::Conflict)), "{result:?}");
    postgres.transaction(vec![insert_item(1)]).await.unwrap();
}

#[tokio::test]
async fn a_nul_character_the_database_cannot_store_is_a_bad_request() {
    let (_database, postgres) = database_with_items().await;
    let nul = Step::Statement {
        sql: "SELECT $1::jsonb".to_owned(),
        params: vec![SqlValue::Json(serde_json::json!({ "note": "a\u{0}b" }))],
        expect_rows: None,
    };

    let result = postgres.transaction(vec![nul]).await;

    assert!(matches!(result, Err(Error::BadRequest(_))), "{result:?}");
}

#[tokio::test]
async fn an_overlap_an_exclusion_refuses_and_a_broken_check_are_refusals_naming_them() {
    let database = TestDatabase::create().await;
    let postgres = Postgres::connect(database.url()).unwrap();
    let bookings = "CREATE TABLE bookings (
        id int8 PRIMARY KEY,
        seats int8 NOT NULL CONSTRAINT bookings_seats_positive CHECK (seats > 0),
        during tstzrange NOT NULL,
        CONSTRAINT bookings_no_overlap EXCLUDE USING gist (during WITH &&)
    )";
    postgres
        .transaction(vec![Step::Script(bookings.to_owned())])
        .await
        .unwrap();
    let book = |id: i64, seats: i64| Step::Statement {
        sql: "INSERT INTO bookings (id, seats, during) VALUES ($1, $2, \
              tstzrange('2026-10-20 09:00Z', '2026-10-20 10:00Z'))"
            .to_owned(),
        params: vec![SqlValue::Int(id), SqlValue::Int(seats)],
        expect_rows: None,
    };
    postgres.transaction(vec![book(1, 2)]).await.unwrap();

    let overlapping = postgres.transaction(vec![book(2, 2)]).await;
    let no_seats = postgres.transaction(vec![book(3, 0)]).await;

    assert!(
        matches!(&overlapping, Err(Error::Rule(message)) if message.contains("bookings_no_overlap")),
        "{overlapping:?}"
    );
    assert!(
        matches!(&no_seats, Err(Error::Rule(message)) if message.contains("bookings_seats_positive")),
        "{no_seats:?}"
    );
}

#[tokio::test]
async fn a_broken_reference_is_a_refusal() {
    let database = TestDatabase::create().await;
    let postgres = Postgres::connect(database.url()).unwrap();
    let tables = "CREATE TABLE shelves (id int8 PRIMARY KEY);
        CREATE TABLE books (id int8 PRIMARY KEY, shelf int8 NOT NULL REFERENCES shelves (id))";
    postgres
        .transaction(vec![Step::Script(tables.to_owned())])
        .await
        .unwrap();

    let refused = postgres
        .transaction(vec![Step::statement(
            "INSERT INTO books (id, shelf) VALUES (1, 9)",
            Vec::new(),
            None,
        )])
        .await;

    assert!(matches!(&refused, Err(Error::Rule(_))), "{refused:?}");
}
