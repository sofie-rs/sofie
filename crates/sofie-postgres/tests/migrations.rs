use sofie_core::SqlValue;
use sofie_db::{Db, File, MigrationState, MigrationStatus, Migrations};
use sofie_postgres::Postgres;
use sofie_testing::TestDatabase;

const CREATE_ITEMS: File = File::new(
    "0001_create_items.sql",
    b"CREATE TABLE items (id int8 PRIMARY KEY);",
);
const INSERT_ITEM: File = File::new("0002_insert_item.sql", b"INSERT INTO items VALUES (1);");

/// Tests vary the files between runs; the wiring embeds them for good.
fn shop_migrations(files: &[File]) -> Migrations {
    Migrations::new("shop", Vec::leak(files.to_vec()))
}

async fn query(database: &TestDatabase, sql: &str) -> Vec<Vec<SqlValue>> {
    let postgres = Postgres::connect(database.url()).unwrap();
    sofie_db::query_values(&postgres, sql, &[]).await.unwrap()
}

#[tokio::test]
async fn migrations_apply_into_the_module_schema_and_are_recorded() {
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());

    db.schema()
        .migrate(&[shop_migrations(&[INSERT_ITEM, CREATE_ITEMS])])
        .await
        .unwrap();

    assert_eq!(
        query(&database, "SELECT id FROM shop.items").await,
        vec![vec![SqlValue::Int(1)]]
    );
    assert_eq!(
        query(
            &database,
            "SELECT module, name FROM sofie.migrations WHERE module = 'shop' ORDER BY name"
        )
        .await,
        vec![
            vec![
                SqlValue::Text("shop".to_owned()),
                SqlValue::Text("0001_create_items.sql".to_owned())
            ],
            vec![
                SqlValue::Text("shop".to_owned()),
                SqlValue::Text("0002_insert_item.sql".to_owned())
            ],
        ]
    );
}

#[tokio::test]
async fn the_framework_tables_are_migrations_of_module_sofie_applied_first() {
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());

    db.schema().migrate(&[]).await.unwrap();

    assert_eq!(
        query(
            &database,
            "SELECT module, name FROM sofie.migrations ORDER BY name"
        )
        .await,
        [
            "0001_messages.sql",
            "0002_idempotency.sql",
            "0003_history.sql",
            "0004_flags.sql",
            "0005_shield.sql",
            "0006_module_roles.sql",
            "0007_feed.sql",
            "0008_settings.sql",
            "0009_local_schedules.sql",
            "0010_notify.sql",
            "0011_product_events.sql",
            "0012_mail_suppressions.sql",
            "0013_messages_waiting.sql",
            "0014_mirror_forgotten.sql",
            "0015_mirror_forgotten_at.sql",
            "0016_history_action.sql",
            "0017_messages_actor.sql",
            "0018_previous_slug.sql",
            "0019_schedule_cadence.sql",
            "0020_inbox_read.sql",
            "0021_notify_digest_items_sent.sql",
            "0022_forgotten_principals.sql",
            "0023_product_events_principal.sql",
            "0024_forgotten_principals_at.sql",
        ]
        .map(|name| vec![
            SqlValue::Text("sofie".to_owned()),
            SqlValue::Text(name.to_owned())
        ])
    );
    assert_eq!(
        query(&database, "SELECT count(*) FROM sofie.messages").await,
        vec![vec![SqlValue::Int(0)]]
    );
}

#[tokio::test]
async fn applied_migrations_are_skipped_on_the_next_run() {
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    db.schema()
        .migrate(&[shop_migrations(&[CREATE_ITEMS, INSERT_ITEM])])
        .await
        .unwrap();

    let another = File::new("0003_insert_another.sql", b"INSERT INTO items VALUES (2);");
    db.schema()
        .migrate(&[shop_migrations(&[CREATE_ITEMS, INSERT_ITEM, another])])
        .await
        .unwrap();

    assert_eq!(
        query(&database, "SELECT id FROM shop.items ORDER BY id").await,
        vec![vec![SqlValue::Int(1)], vec![SqlValue::Int(2)]]
    );
}

#[tokio::test]
async fn an_applied_migration_that_changed_is_refused() {
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    db.schema()
        .migrate(&[shop_migrations(&[CREATE_ITEMS, INSERT_ITEM])])
        .await
        .unwrap();

    let edited = File::new("0002_insert_item.sql", b"INSERT INTO items VALUES (3);");
    let error = db
        .schema()
        .migrate(&[shop_migrations(&[CREATE_ITEMS, edited])])
        .await
        .unwrap_err();

    assert!(
        error.to_string().contains("0002_insert_item.sql"),
        "{error}"
    );
    assert_eq!(
        query(&database, "SELECT id FROM shop.items").await,
        vec![vec![SqlValue::Int(1)]]
    );
}

#[tokio::test]
async fn the_status_tells_applied_pending_edited_and_missing_migrations_apart() {
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    let gone = File::new("0003_gone.sql", b"INSERT INTO items VALUES (2);");
    db.schema()
        .migrate(&[shop_migrations(&[CREATE_ITEMS, INSERT_ITEM, gone])])
        .await
        .unwrap();
    db.schema()
        .migrate(&[Migrations::new("retired", &[CREATE_ITEMS])])
        .await
        .unwrap();

    let edited = File::new("0002_insert_item.sql", b"INSERT INTO items VALUES (3);");
    let next = File::new("0004_next.sql", b"INSERT INTO items VALUES (4);");
    let modules = db
        .schema()
        .migration_status(&[shop_migrations(&[CREATE_ITEMS, edited, next])])
        .await
        .unwrap();

    let status = |name: &str, state| MigrationStatus {
        name: name.to_owned(),
        state,
    };
    let names: Vec<&str> = modules
        .iter()
        .map(|module| module.module.as_str())
        .collect();
    assert_eq!(names, ["sofie", "shop", "retired"]);
    assert!(modules[0].is_consistent());
    assert!(
        modules[0]
            .migrations
            .iter()
            .all(|migration| migration.state == MigrationState::Applied)
    );
    assert_eq!(
        modules[1].migrations,
        [
            status("0001_create_items.sql", MigrationState::Applied),
            status("0002_insert_item.sql", MigrationState::EditedAfterApplied),
            status("0003_gone.sql", MigrationState::MissingFile),
            status("0004_next.sql", MigrationState::Pending),
        ]
    );
    assert_eq!(
        modules[2].migrations,
        [status("0001_create_items.sql", MigrationState::MissingFile)]
    );
}

#[tokio::test]
async fn a_database_never_migrated_has_every_migration_pending() {
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());

    let modules = db
        .schema()
        .migration_status(&[shop_migrations(&[CREATE_ITEMS])])
        .await
        .unwrap();

    assert!(modules.iter().all(|module| {
        module
            .migrations
            .iter()
            .all(|migration| migration.state == MigrationState::Pending)
    }));
    assert_eq!(modules[1].migrations.len(), 1);
}

#[tokio::test]
async fn the_first_pending_migration_is_named_until_every_one_is_applied() {
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    let first = shop_migrations(&[CREATE_ITEMS]);
    let both = shop_migrations(&[CREATE_ITEMS, INSERT_ITEM]);
    let pending = async |modules: &[Migrations]| {
        let pending = db.schema().first_pending(modules).await.unwrap();
        pending.map(|migration| migration.to_string())
    };

    assert_eq!(
        pending(&[both]).await.as_deref(),
        Some("sofie/0001_messages.sql")
    );
    db.schema().migrate(&[first]).await.unwrap();
    assert_eq!(
        pending(&[both]).await.as_deref(),
        Some("shop/0002_insert_item.sql")
    );
    db.schema().migrate(&[both]).await.unwrap();

    assert_eq!(pending(&[both]).await, None);
}

#[tokio::test]
async fn a_failing_migration_is_not_recorded() {
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());

    let broken = shop_migrations(&[File::new("0001_broken.sql", b"CREATE TABLE broken (;")]);
    assert!(db.schema().migrate(&[broken]).await.is_err());

    assert_eq!(
        query(
            &database,
            "SELECT count(*) FROM sofie.migrations WHERE module = 'shop'"
        )
        .await,
        vec![vec![SqlValue::Int(0)]]
    );
}

#[tokio::test]
async fn processes_migrating_at_once_each_succeed_and_apply_every_file_once() {
    let database = TestDatabase::create().await;
    let first = Db::new(Postgres::connect(database.url()).unwrap());
    let second = Db::new(Postgres::connect(database.url()).unwrap());
    let migrations = [shop_migrations(&[CREATE_ITEMS, INSERT_ITEM])];

    let (first_result, second_result) = tokio::join!(
        first.schema().migrate(&migrations),
        second.schema().migrate(&migrations)
    );

    first_result.unwrap();
    second_result.unwrap();
    assert_eq!(
        query(&database, "SELECT id FROM shop.items").await,
        vec![vec![SqlValue::Int(1)]]
    );
}

#[tokio::test]
async fn a_concurrent_index_build_runs_alone_and_is_recorded_once() {
    let database = TestDatabase::create().await;
    let first = Db::new(Postgres::connect(database.url()).unwrap());
    let second = Db::new(Postgres::connect(database.url()).unwrap());
    let index = File::new(
        "0002_items_index.sql",
        b"-- Built without blocking writes.\nCREATE INDEX CONCURRENTLY IF NOT EXISTS items_id ON items (id);",
    );
    let migrations = [shop_migrations(&[CREATE_ITEMS, index])];

    let (first_result, second_result) = tokio::join!(
        first.schema().migrate(&migrations),
        second.schema().migrate(&migrations)
    );

    first_result.unwrap();
    second_result.unwrap();
    assert_eq!(
        query(
            &database,
            "SELECT indisvalid FROM pg_index WHERE indexrelid = 'shop.items_id'::regclass"
        )
        .await,
        vec![vec![SqlValue::Bool(true)]]
    );
    assert_eq!(
        query(
            &database,
            "SELECT count(*) FROM sofie.migrations WHERE name = '0002_items_index.sql'"
        )
        .await,
        vec![vec![SqlValue::Int(1)]]
    );
}

#[tokio::test]
async fn a_rerun_skipping_the_invalid_index_of_a_failed_build_is_refused_unrecorded() {
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    let tags = File::new(
        "0001_tags.sql",
        b"CREATE TABLE tags (name text); INSERT INTO tags VALUES ('a'), ('a');",
    );
    let unique = File::new(
        "0002_tags_unique.sql",
        b"CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS tags_name ON tags (name);",
    );
    let migrations = [shop_migrations(&[tags, unique])];
    db.schema().migrate(&migrations).await.unwrap_err();

    let rerun = db.schema().migrate(&migrations).await.unwrap_err();

    assert!(
        format!("{rerun:?}").contains("index shop.tags_name is invalid"),
        "{rerun:?}"
    );
    assert_eq!(
        query(
            &database,
            "SELECT count(*) FROM sofie.migrations WHERE name = '0002_tags_unique.sql'"
        )
        .await,
        vec![vec![SqlValue::Int(0)]]
    );
}

/// A process waiting for the lock in a statement holds a snapshot that a concurrent
/// index build waits out, while the build's process holds the lock: a deadlock.
#[tokio::test]
async fn a_process_waiting_for_the_migrations_lock_holds_up_no_concurrent_index_build() {
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    db.schema()
        .migrate(&[shop_migrations(&[CREATE_ITEMS])])
        .await
        .unwrap();
    let (building, connection) = tokio_postgres::connect(database.url(), tokio_postgres::NoTls)
        .await
        .unwrap();
    tokio::spawn(connection);
    building
        .batch_execute("SELECT pg_advisory_lock(7046573208157101413)")
        .await
        .unwrap();
    let build_while_the_other_waits = async {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let built = building
            .batch_execute("CREATE INDEX CONCURRENTLY items_built ON shop.items (id)")
            .await;
        building
            .batch_execute("SELECT pg_advisory_unlock(7046573208157101413)")
            .await
            .unwrap();
        built
    };

    let migrations = [shop_migrations(&[CREATE_ITEMS, INSERT_ITEM])];

    let (migrated, built) = tokio::join!(
        db.schema().migrate(&migrations),
        build_while_the_other_waits
    );

    built.unwrap();
    migrated.unwrap();
    assert_eq!(
        query(&database, "SELECT id FROM shop.items").await,
        vec![vec![SqlValue::Int(1)]]
    );
}

#[tokio::test]
async fn whether_a_run_still_waits_is_read_from_a_valid_index() {
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());

    db.schema().migrate(&[]).await.unwrap();

    assert_eq!(
        query(
            &database,
            "SELECT indisvalid FROM pg_index WHERE indexrelid = 'sofie.messages_waiting'::regclass"
        )
        .await,
        vec![vec![SqlValue::Bool(true)]]
    );
}
