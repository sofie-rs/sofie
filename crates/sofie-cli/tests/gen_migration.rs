use std::fs;

use sofie_check::{Rule, Screens};
use sofie_cli::{GenError, NewCode, new_code};
use sofie_model::testing::{TemporaryDirectory, manifest};

const INVOICE: &str = "use sofie::prelude::*;\n\n\
    #[derive(Aggregate)]\n\
    #[aggregate(behaviors(SoftDelete))]\n\
    pub struct Invoice {\n    meta: Meta,\n    #[owner]\n    account: AccountId,\n    \
    status: InvoiceStatus,\n    total: Money,\n    #[unique]\n    number: String,\n    \
    paid_at: Option<Instant>,\n}\n\n\
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Enum)]\n\
    pub enum InvoiceStatus {\n    Open,\n    Paid,\n}\n";

/// What the first migration of a module holding `INVOICE` would create.
const CREATED: &str = "CREATE TABLE \"invoice\" (\n    \
    \"id\" uuid PRIMARY KEY,\n    \
    \"version\" bigint NOT NULL,\n    \
    \"created_at\" timestamptz NOT NULL,\n    \
    \"updated_at\" timestamptz NOT NULL,\n    \
    \"deleted_at\" timestamptz,\n    \
    \"account\" uuid NOT NULL,\n    \
    \"status\" text NOT NULL CHECK (\"status\" IN ('OPEN', 'PAID')),\n    \
    \"total\" bigint NOT NULL,\n    \
    \"total_currency\" text NOT NULL,\n    \
    \"number\" text NOT NULL,\n    \
    \"paid_at\" timestamptz\n);\n\
    CREATE INDEX \"invoice_account\" ON \"invoice\" (\"account\");\n\
    CREATE UNIQUE INDEX \"invoice_number\" ON \"invoice\" (\"number\");\n";

fn billing_app(migration: &str) -> TemporaryDirectory {
    let directory = TemporaryDirectory::new("sofie-gen-migration");
    directory.write(
        "Cargo.toml",
        "[workspace]\nresolver = \"3\"\nmembers = [\"app\", \"modules/*\"]\n",
    );
    directory.write(
        "app/Cargo.toml",
        &manifest("shop", "billing = { path = \"../modules/billing\" }\n"),
    );
    directory.write("app/src/main.rs", "fn main() {}\n");
    directory.write("modules/billing/Cargo.toml", &manifest("billing", ""));
    directory.write(
        "modules/billing/src/lib.rs",
        "pub mod module;\nmod invoice;\n\npub use invoice::Invoice;\n",
    );
    directory.write(
        "modules/billing/src/module.rs",
        "pub const MODULE: sofie::Module = sofie::Module {\n    name: \"billing\",\n    \
         tier: sofie::Tier::Core,\n    context: None,\n    settings: &[],\n};\n",
    );
    directory.write("modules/billing/src/invoice/mod.rs", INVOICE);
    directory.write("modules/billing/migrations/0001_start.sql", migration);
    directory
}

fn generate(directory: &TemporaryDirectory) -> Result<Vec<String>, GenError> {
    let root = format!("{}/", directory.root().display());
    let code = NewCode::SchemaMigration {
        module: "billing".to_owned(),
        name: "invoices".to_owned(),
    };
    let changes = new_code(directory.root(), &code)?;
    Ok(changes
        .iter()
        .map(|change| change.to_string().replacen(&root, "", 1))
        .collect())
}

fn read(directory: &TemporaryDirectory, migration: &str) -> String {
    fs::read_to_string(directory.path(&format!("modules/billing/migrations/{migration}"))).unwrap()
}

/// The rules `sofie check` holds migrations and schema to.
fn schema_findings(directory: &TemporaryDirectory) -> Vec<String> {
    let app = sofie_model::load(directory.root()).unwrap();
    sofie_check::check(&app, &Screens::read(&app.modules))
        .into_iter()
        .filter(|finding| {
            matches!(
                finding.rule,
                Rule::MigrationLock | Rule::SchemaConstraint | Rule::BehaviorColumn
            )
        })
        .map(|finding| finding.to_string())
        .collect()
}

#[test]
fn a_new_aggregate_gets_its_table_checks_and_indexes() {
    let app = billing_app("");
    assert_ne!(schema_findings(&app), Vec::<String>::new());

    let changes = generate(&app).unwrap();

    assert_eq!(
        changes,
        ["created modules/billing/migrations/0002_invoices.sql"]
    );
    assert_eq!(read(&app, "0002_invoices.sql"), CREATED);
    assert_eq!(schema_findings(&app), Vec::<String>::new());
}

#[test]
fn an_existing_table_gains_columns_checks_and_indexes_in_steps_the_rules_accept() {
    let app = billing_app(
        "CREATE TABLE invoice (\n    id uuid PRIMARY KEY,\n    version bigint NOT NULL,\n    \
         account uuid NOT NULL,\n    total bigint NOT NULL,\n    total_currency char(3) NOT NULL,\n    \
         number text NOT NULL UNIQUE\n);\n",
    );

    let changes = generate(&app).unwrap();

    assert_eq!(
        changes,
        [
            "created modules/billing/migrations/0002_invoices.sql",
            "created modules/billing/migrations/0003_invoices_checks.sql",
            "created modules/billing/migrations/0004_index_invoice_account.sql",
        ]
    );
    assert_eq!(
        read(&app, "0002_invoices.sql"),
        "-- Contract step, once no running version inserts without `invoice.created_at`:\n\
         -- ALTER TABLE \"invoice\" ALTER COLUMN \"created_at\" DROP DEFAULT;\n\
         -- Contract step, once no running version inserts without `invoice.updated_at`:\n\
         -- ALTER TABLE \"invoice\" ALTER COLUMN \"updated_at\" DROP DEFAULT;\n\
         -- Contract step: `invoice.status` is required, yet added nullable, as existing rows \
         have no value. Once every row has one:\n\
         -- ALTER TABLE \"invoice\" ADD CONSTRAINT \"invoice_status_not_null\" CHECK (\"status\" \
         IS NOT NULL) NOT VALID;\n\
         -- then in a later migration:\n\
         -- ALTER TABLE \"invoice\" VALIDATE CONSTRAINT \"invoice_status_not_null\";\n\
         -- ALTER TABLE \"invoice\" ALTER COLUMN \"status\" SET NOT NULL;\n\
         ALTER TABLE \"invoice\" ADD COLUMN \"created_at\" timestamptz NOT NULL DEFAULT now();\n\
         ALTER TABLE \"invoice\" ADD COLUMN \"updated_at\" timestamptz NOT NULL DEFAULT now();\n\
         ALTER TABLE \"invoice\" ADD COLUMN \"deleted_at\" timestamptz;\n\
         ALTER TABLE \"invoice\" ADD COLUMN \"status\" text;\n\
         ALTER TABLE \"invoice\" ADD CONSTRAINT \"invoice_status_check\" \
         CHECK (\"status\" IN ('OPEN', 'PAID')) NOT VALID;\n\
         ALTER TABLE \"invoice\" ADD COLUMN \"paid_at\" timestamptz;\n"
    );
    assert_eq!(
        read(&app, "0003_invoices_checks.sql"),
        "ALTER TABLE \"invoice\" VALIDATE CONSTRAINT \"invoice_status_check\";\n"
    );
    assert_eq!(
        read(&app, "0004_index_invoice_account.sql"),
        "CREATE INDEX CONCURRENTLY IF NOT EXISTS \"invoice_account\" ON \"invoice\" (\"account\");\n"
    );
    assert_eq!(schema_findings(&app), Vec::<String>::new());
}

#[test]
fn a_dropped_field_alone_writes_no_file_and_names_its_contract_step() {
    let mut migration = CREATED.replace(
        "    \"paid_at\" timestamptz\n",
        "    \"paid_at\" timestamptz,\n    \"memo\" text\n",
    );
    migration.push('\n');
    let app = billing_app(&migration);

    let refused = generate(&app);

    let Err(GenError::Usage(message)) = refused else {
        panic!("{refused:?}");
    };
    assert!(
        message.ends_with(
            "nothing to write\n  Contract step, once no running version reads `invoice.memo`:\n\
             ALTER TABLE \"invoice\" DROP COLUMN \"memo\";"
        ),
        "{message}"
    );
    assert!(
        !app.path("modules/billing/migrations/0002_invoices.sql")
            .exists()
    );
}

#[test]
fn nothing_to_write_is_refused_and_unknown_types_are_left_to_the_developer() {
    let app = billing_app(CREATED);
    let invoice = INVOICE.replace(
        "    paid_at: Option<Instant>,\n",
        "    paid_at: Option<Instant>,\n    terms: Terms,\n",
    );
    app.write("modules/billing/src/invoice/mod.rs", &invoice);

    let refused = generate(&app);

    let Err(GenError::Usage(message)) = refused else {
        panic!("{refused:?}");
    };
    assert_eq!(
        message,
        "module `billing`'s migrations already create every table, column, CHECK and index its \
         aggregates declare; nothing to write\n  \
         `invoice.terms` is a `Terms`, whose columns `sofie gen migration` cannot tell: add them \
         by hand."
    );
}

#[test]
fn a_module_with_sqlite_twins_gets_a_twin_of_each_step() {
    let app = billing_app(
        "CREATE TABLE invoice (id uuid PRIMARY KEY, version bigint NOT NULL, account uuid NOT NULL, \
         total bigint NOT NULL, total_currency text NOT NULL, number text NOT NULL UNIQUE);\n",
    );
    app.write("modules/billing/migrations/sqlite/0001_start.sql", "");

    let changes = generate(&app).unwrap();

    assert_eq!(changes.len(), 6, "{changes:?}");
    let twin = |name: &str| read(&app, &format!("sqlite/{name}"));
    assert!(
        twin("0002_invoices.sql").ends_with(
            "-- SQLite adds no NOT NULL column without a constant default, so it stays nullable.\n\
             ALTER TABLE \"billing_invoice\" ADD COLUMN \"created_at\" timestamptz;\n\
             UPDATE \"billing_invoice\" SET \"created_at\" = \
             CAST(unixepoch('subsec') * 1000000 AS INTEGER);\n\
             -- SQLite adds no NOT NULL column without a constant default, so it stays nullable.\n\
             ALTER TABLE \"billing_invoice\" ADD COLUMN \"updated_at\" timestamptz;\n\
             UPDATE \"billing_invoice\" SET \"updated_at\" = \
             CAST(unixepoch('subsec') * 1000000 AS INTEGER);\n\
             ALTER TABLE \"billing_invoice\" ADD COLUMN \"deleted_at\" timestamptz;\n\
             -- SQLite adds no NOT NULL column without a constant default, so it stays nullable.\n\
             ALTER TABLE \"billing_invoice\" ADD COLUMN \"status\" text \
             CHECK (\"status\" IN ('OPEN', 'PAID'));\n\
             ALTER TABLE \"billing_invoice\" ADD COLUMN \"paid_at\" timestamptz;\n"
        ),
        "{}",
        twin("0002_invoices.sql")
    );
    assert_eq!(
        twin("0003_invoices_checks.sql"),
        "-- SQLite needs no such step.\n"
    );
    assert_eq!(
        twin("0004_index_invoice_account.sql"),
        "CREATE INDEX \"billing_invoice_account\" ON \"billing_invoice\" (\"account\");\n"
    );
}

#[test]
fn a_sqlite_twin_fills_a_required_flag_it_adds_with_its_constant() {
    let app = billing_app(CREATED);
    app.write("modules/billing/migrations/sqlite/0001_start.sql", "");
    app.write(
        "modules/billing/src/invoice/mod.rs",
        &INVOICE.replace("behaviors(SoftDelete)", "behaviors(SoftDelete, Active)"),
    );

    generate(&app).unwrap();

    assert_eq!(
        read(&app, "0002_invoices.sql"),
        "-- Contract step, once no running version inserts without `invoice.is_active`:\n\
         -- ALTER TABLE \"invoice\" ALTER COLUMN \"is_active\" DROP DEFAULT;\n\
         ALTER TABLE \"invoice\" ADD COLUMN \"is_active\" boolean NOT NULL DEFAULT true;\n"
    );
    assert_eq!(
        read(&app, "sqlite/0002_invoices.sql"),
        "-- Contract step, once no running version inserts without `invoice.is_active`:\n\
         -- ALTER TABLE \"invoice\" ALTER COLUMN \"is_active\" DROP DEFAULT;\n\
         ALTER TABLE \"billing_invoice\" ADD COLUMN \"is_active\" boolean NOT NULL DEFAULT true;\n"
    );
}

#[test]
fn a_field_made_unique_over_its_plain_index_gets_a_unique_index_of_its_own() {
    let app = billing_app(&CREATED.replace(
        "CREATE UNIQUE INDEX \"invoice_number\"",
        "CREATE INDEX \"invoice_number\"",
    ));

    let changes = generate(&app).unwrap();

    assert_eq!(
        changes,
        [
            "created modules/billing/migrations/0002_invoices.sql",
            "created modules/billing/migrations/0003_index_invoice_number_key.sql",
        ]
    );
    assert_eq!(
        read(&app, "0002_invoices.sql"),
        "-- Contract step: the unique index `invoice_number_key` serves every lookup the index \
         `invoice_number` does, so once it is built:\n\
         -- DROP INDEX CONCURRENTLY IF EXISTS \"invoice_number\";\n"
    );
    assert_eq!(
        read(&app, "0003_index_invoice_number_key.sql"),
        "CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS \"invoice_number_key\" ON \"invoice\" \
         (\"number\");\n"
    );
    assert_eq!(schema_findings(&app), Vec::<String>::new());
}

#[test]
fn slugged_coded_and_default_aggregates_get_the_indexes_their_behaviors_rely_on() {
    let app = billing_app("");
    app.write(
        "modules/billing/src/invoice/mod.rs",
        &INVOICE.replace(
            "behaviors(SoftDelete)",
            "behaviors(SoftDelete, Default, Slugged(number), UniqueCode)",
        ),
    );

    generate(&app).unwrap();

    let created = read(&app, "0002_invoices.sql");
    for index in [
        "CREATE UNIQUE INDEX \"invoice_is_default\" ON \"invoice\" (\"account\", \"is_default\") \
         WHERE \"is_default\";\n",
        "CREATE UNIQUE INDEX \"invoice_slug\" ON \"invoice\" (\"slug\");\n",
        "CREATE UNIQUE INDEX \"invoice_code\" ON \"invoice\" (\"code\");\n",
    ] {
        assert!(created.contains(index), "{created}");
    }
    assert_eq!(schema_findings(&app), Vec::<String>::new());
}

#[test]
fn a_column_migrated_with_another_type_or_nullability_is_left_to_the_developer() {
    let app = billing_app(
        &CREATED
            .replace("\"total\" bigint NOT NULL", "\"total\" integer NOT NULL")
            .replace("\"number\" text NOT NULL", "\"number\" text")
            .replace(
                "\"paid_at\" timestamptz",
                "\"paid_at\" timestamptz NOT NULL",
            ),
    );

    let refused = generate(&app);

    let Err(GenError::Usage(message)) = refused else {
        panic!("{refused:?}");
    };
    for note in [
        "`invoice.total` is declared as `bigint`, yet migrated as another type",
        "`invoice.number` is required, yet migrated nullable. Once every row has one:",
        "`invoice.paid_at` is optional, yet migrated NOT NULL",
        "ALTER TABLE \"invoice\" ALTER COLUMN \"paid_at\" DROP NOT NULL;",
    ] {
        assert!(message.contains(note), "{message}");
    }
}
