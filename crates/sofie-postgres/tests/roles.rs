#![cfg(feature = "module-roles")]

use sofie_core::{ColumnReader, Result, SqlValue};
use sofie_db::{Aggregate, Db, Field, File, Meta, Migrations};
use sofie_postgres::Postgres;
use sofie_testing::TestDatabase;

const MODULE_ROLE: &str = include_str!("../../sofie-cli/module_role.sql");

const ITEMS: File = File::new(
    "0001_items.sql",
    b"CREATE TABLE items (id uuid PRIMARY KEY, version int8 NOT NULL);
     INSERT INTO items VALUES (gen_random_uuid(), 1);",
);

/// Role migration first, table in a later one, as in an app that had tables before roles.
fn module(name: &'static str) -> Migrations {
    let role = MODULE_ROLE.replace("{module}", name);
    Migrations::new(
        name,
        Vec::leak(vec![
            File::new("0000_module_role.sql", String::leak(role).as_bytes()),
            ITEMS,
        ]),
    )
}

fn module_without_role(name: &'static str) -> Migrations {
    Migrations::new(name, Vec::leak(vec![ITEMS]))
}

/// Stamps each row with the role that inserted it.
const WRITER: File = File::new(
    "0002_writer.sql",
    b"ALTER TABLE items ADD COLUMN written_by text;
     CREATE FUNCTION stamp_writer() RETURNS trigger LANGUAGE plpgsql AS $$
     BEGIN NEW.written_by := current_user; RETURN NEW; END $$;
     CREATE TRIGGER stamp_writer BEFORE INSERT ON items
         FOR EACH ROW EXECUTE FUNCTION stamp_writer();",
);

fn module_with_writer(name: &'static str) -> Migrations {
    let role = MODULE_ROLE.replace("{module}", name);
    Migrations::new(
        name,
        Vec::leak(vec![
            File::new("0000_module_role.sql", String::leak(role).as_bytes()),
            ITEMS,
            WRITER,
        ]),
    )
}

trait Module: Send + Sync + 'static {
    const NAME: &'static str;
}

struct Alpha;

impl Module for Alpha {
    const NAME: &'static str = "roles_alpha";
}

struct Beta;

impl Module for Beta {
    const NAME: &'static str = "roles_beta";
}

struct Gamma;

impl Module for Gamma {
    const NAME: &'static str = "roles_gamma";
}

struct Item<M = Alpha> {
    meta: Meta,
    module: std::marker::PhantomData<M>,
}

impl<M: Module> Item<M> {
    fn new() -> Self {
        Self {
            meta: Meta::new().unwrap(),
            module: std::marker::PhantomData,
        }
    }
}

impl<M: Module> Aggregate for Item<M> {
    const NAME: &'static str = "items";
    const MODULE: &'static str = M::NAME;
    const FIELDS: &'static [Field] = &[];

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, _: &mut Vec<SqlValue>) {}

    fn decode(meta: Meta, _: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            module: std::marker::PhantomData,
        })
    }
}

#[tokio::test]
async fn raw_sql_runs_as_its_module_s_role_which_reaches_no_other_module() {
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    db.schema()
        .migrate(&[module("roles_alpha"), module("roles_beta")])
        .await
        .unwrap();

    assert_roles_hold(&db, &database).await;
}

/// The migrator applies every file it has no record of, whatever its number.
#[tokio::test]
async fn a_role_migration_added_after_the_tables_were_migrated_is_applied() {
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    db.schema()
        .migrate(&[
            module_without_role("roles_alpha"),
            module_without_role("roles_beta"),
        ])
        .await
        .unwrap();

    db.schema()
        .migrate(&[module("roles_alpha"), module("roles_beta")])
        .await
        .unwrap();

    assert_roles_hold(&db, &database).await;
}

#[tokio::test]
async fn a_commit_writes_each_module_s_records_as_that_module_s_role() {
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    db.schema()
        .migrate(&[
            module_with_writer("roles_alpha"),
            module_with_writer("roles_beta"),
        ])
        .await
        .unwrap();
    let mut both = (Item::<Alpha>::new(), Item::<Beta>::new());

    db.commit(&mut both).await.unwrap();

    let writers = sofie_db::query_values(
        &Postgres::connect(database.url()).unwrap(),
        "SELECT written_by FROM roles_alpha.items WHERE written_by IS NOT NULL
         UNION ALL
         SELECT written_by FROM roles_beta.items WHERE written_by IS NOT NULL",
        &[],
    )
    .await
    .unwrap();
    assert_eq!(
        writers,
        vec![
            vec![SqlValue::Text("roles_alpha".to_owned())],
            vec![SqlValue::Text("roles_beta".to_owned())],
        ]
    );
}

/// A raw step on `M`'s items, planned after another module's write.
struct Bumped<M>(std::marker::PhantomData<M>);

impl<M: Module> sofie_db::Outcome for Bumped<M> {
    fn plan(&self, plan: &mut sofie_db::Plan<'_>) -> Result<()> {
        let sql = format!("UPDATE {}.items SET version = version + 1", M::NAME);
        plan.push_as::<Item<M>>(sofie_db::Step::statement(sql, Vec::new(), Some(1)));
        Ok(())
    }
}

#[tokio::test]
async fn a_raw_step_runs_as_its_own_module_after_another_module_s_write() {
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    db.schema()
        .migrate(&[module("roles_alpha"), module("roles_beta")])
        .await
        .unwrap();

    db.commit(&mut (
        Item::<Alpha>::new(),
        Bumped::<Beta>(std::marker::PhantomData),
    ))
    .await
    .unwrap();
}

/// The module without a role is written as the login, not as the module before it.
#[tokio::test]
async fn a_module_without_its_role_is_written_as_the_login_after_one_with_a_role() {
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    db.schema()
        .migrate(&[module("roles_alpha"), module_without_role("roles_gamma")])
        .await
        .unwrap();

    db.commit(&mut (Item::<Alpha>::new(), Item::<Gamma>::new()))
        .await
        .unwrap();
}

struct Restocked;

impl serde::Serialize for Restocked {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_unit()
    }
}

impl sofie_db::Input for Restocked {
    fn from_json(_: serde_json::Value) -> Result<Self> {
        Ok(Self)
    }
}

impl sofie_db::Event for Restocked {
    const NAME: &'static str = "roles_alpha.Restocked";
}

/// The event is queued and dropped in the commit; a module role may only queue it.
#[tokio::test]
async fn a_commit_raising_an_event_no_worker_hears_writes_as_its_module_s_role() {
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    db.schema().migrate(&[module("roles_alpha")]).await.unwrap();
    sofie_db::worker::Worker::new(db.clone())
        .drain()
        .await
        .unwrap();
    let mut item = Item::<Alpha>::new();
    item.meta_mut().raise(Restocked).unwrap();

    db.commit(&mut item).await.unwrap();

    let postgres = Postgres::connect(database.url()).unwrap();
    assert_eq!(
        sofie_db::query_values(
            &postgres,
            "SELECT (SELECT count(*) FROM roles_alpha.items), (SELECT count(*) FROM sofie.messages)",
            &[]
        )
        .await
        .unwrap(),
        vec![vec![SqlValue::Int(2), SqlValue::Int(0)]]
    );
}

#[tokio::test]
async fn a_commit_raising_an_event_a_worker_hears_queues_its_listener_job() {
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    db.schema().migrate(&[module("roles_alpha")]).await.unwrap();
    sofie_db::worker::publish_heard(
        &sofie_db::worker::Worker::new(db.clone())
            .on::<Restocked, _>("roles_alpha.restock", restock),
    );
    let mut item = Item::<Alpha>::new();
    item.meta_mut().raise(Restocked).unwrap();

    db.commit(&mut item).await.unwrap();

    let postgres = Postgres::connect(database.url()).unwrap();
    assert_eq!(
        sofie_db::query_values(&postgres, "SELECT kind, name FROM sofie.messages", &[])
            .await
            .unwrap(),
        vec![vec![
            SqlValue::Text("job".to_owned()),
            SqlValue::Text("roles_alpha.restock".to_owned())
        ]]
    );
}

async fn restock() -> Result<()> {
    Ok(())
}

#[tokio::test]
async fn a_module_role_remembers_the_records_its_mirrors_drop() {
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    db.schema().migrate(&[module("roles_gamma")]).await.unwrap();

    let granted = sofie_db::query_values(
        &Postgres::connect(database.url()).unwrap(),
        "SELECT has_table_privilege('roles_gamma', 'sofie.mirror_forgotten', 'INSERT, UPDATE')",
        &[],
    )
    .await
    .unwrap();

    assert_eq!(granted, vec![vec![SqlValue::Bool(true)]]);
}

/// Roles belong to the whole server, so another database's module of the same name has one.
#[tokio::test]
async fn a_role_this_database_never_set_up_is_not_entered() {
    let elsewhere = TestDatabase::create().await;
    Db::new(Postgres::connect(elsewhere.url()).unwrap())
        .schema()
        .migrate(&[module("roles_gamma")])
        .await
        .unwrap();
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    db.schema()
        .migrate(&[module_without_role("roles_gamma")])
        .await
        .unwrap();

    db.commit(&mut Item::<Gamma>::new()).await.unwrap();
    let read = db
        .read()
        .sql::<Item<Gamma>>("SELECT id, version FROM roles_gamma.items")
        .all()
        .await
        .unwrap();

    assert_eq!(read.len(), 2);
    let postgres = Postgres::connect(database.url()).unwrap();
    assert_eq!(
        sofie_db::query_values(&postgres, "SELECT count(*) FROM roles_gamma.items", &[])
            .await
            .unwrap(),
        vec![vec![SqlValue::Int(2)]]
    );
}

async fn assert_roles_hold(db: &Db, database: &TestDatabase) {
    let read = db.read();

    let own = read
        .sql::<Item>("SELECT id, version FROM roles_alpha.items")
        .all()
        .await
        .unwrap();
    let other = read
        .sql::<Item>("SELECT id, version FROM roles_beta.items")
        .all()
        .await
        .err()
        .unwrap();

    assert_eq!(own.len(), 1);
    assert!(
        format!("{other:?}").contains("permission denied for schema roles_beta"),
        "{other:?}"
    );
    let postgres = Postgres::connect(database.url()).unwrap();
    assert_eq!(
        sofie_db::query_values(&postgres, "SELECT count(*) FROM roles_beta.items", &[])
            .await
            .unwrap(),
        vec![vec![SqlValue::Int(1)]],
        "the app's own login reads every module"
    );
}
