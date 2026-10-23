use std::fs;

use sofie_cli::{AreaAccess, GenError, NewCode, NewMenuEntry, NewRoute, new_code, upgrade};
use sofie_model::testing::{TemporaryDirectory, manifest};

fn billing_app() -> TemporaryDirectory {
    let directory = TemporaryDirectory::new("sofie-new-code");
    directory.write(
        "Cargo.toml",
        "[workspace]\nresolver = \"3\"\nmembers = [\"app\", \"modules/*\", \"modules/*/contract\"]\n\n\
         [workspace.dependencies]\nsofie = { path = \"sofie\" }\n",
    );
    directory.write(
        "sofie/Cargo.toml",
        &format!("{}\n[features]\nbilling = []\n", manifest("sofie", "")),
    );
    directory.write("sofie/src/lib.rs", "");
    directory.write(
        "app/Cargo.toml",
        &manifest("shop", "billing = { path = \"../modules/billing\" }\n"),
    );
    directory.write("app/src/main.rs", "fn main() {}\n");
    directory.write("modules/billing/Cargo.toml", &manifest("billing", ""));
    directory.write(
        "modules/billing/contract/Cargo.toml",
        &manifest("billing-contract", ""),
    );
    directory.write(
        "modules/billing/contract/src/lib.rs",
        "//! What other modules may use of module `billing`.\n",
    );
    directory.write(
        "modules/billing/src/lib.rs",
        "pub mod module;\n\npub mod areas {\n    pub mod web {\n        pub mod routes;\n    }\n}\n",
    );
    directory.write(
        "modules/billing/src/module.rs",
        "pub const MODULE: sofie::Module = sofie::Module {\n    name: \"billing\",\n    tier: sofie::Tier::Core,\n    context: None,\n    settings: &[],\n};\n",
    );
    directory.write(
        "modules/billing/src/areas/web/routes.rs",
        "use sofie::prelude::*;\n\npub fn routes(_routes: &mut Routes<Web>) {}\n",
    );
    directory.write("modules/billing/migrations/0001_start.sql", "");
    directory
}

fn read(directory: &TemporaryDirectory, relative: &str) -> String {
    fs::read_to_string(directory.path(relative)).unwrap()
}

fn generate(directory: &TemporaryDirectory, code: &NewCode) -> Vec<String> {
    let root = format!("{}/", directory.root().display());
    new_code(directory.root(), code)
        .unwrap()
        .iter()
        .map(|change| change.to_string().replacen(&root, "", 1))
        .collect()
}

fn usage(directory: &TemporaryDirectory, code: &NewCode) -> String {
    match new_code(directory.root(), code) {
        Err(GenError::Usage(message)) => message,
        other => panic!("{other:?}"),
    }
}

fn aggregate(name: &str) -> NewCode {
    NewCode::Aggregate {
        module: "billing".to_owned(),
        name: name.to_owned(),
    }
}

fn route(method: &str, path: &str, handler: &str, screen: Option<&str>) -> NewCode {
    NewCode::Route(NewRoute {
        module: "billing".to_owned(),
        area: "web".to_owned(),
        method: method.to_owned(),
        path: path.to_owned(),
        handler: handler.to_owned(),
        screen: screen.map(str::to_owned),
        fields: None,
        read_model: None,
        title: None,
        back: None,
    })
}

#[test]
fn a_migration_takes_the_next_number() {
    let app = billing_app();

    let changes = generate(
        &app,
        &NewCode::Migration {
            module: "billing".to_owned(),
            name: "add_notes".to_owned(),
        },
    );

    assert_eq!(
        changes,
        ["created modules/billing/migrations/0002_add_notes.sql"]
    );
    assert_eq!(
        read(&app, "modules/billing/migrations/0002_add_notes.sql"),
        ""
    );
    let refused = usage(
        &app,
        &NewCode::Migration {
            module: "billing".to_owned(),
            name: "Add notes".to_owned(),
        },
    );
    assert!(
        refused.starts_with("`Add notes` is not a name"),
        "{refused}"
    );
}

#[test]
fn an_aggregate_gets_its_folder_declaration_and_table() {
    let app = billing_app();

    let changes = generate(&app, &aggregate("Invoice"));

    assert_eq!(
        changes,
        [
            "created modules/billing/src/invoice/mod.rs",
            "changed modules/billing/src/lib.rs",
            "created modules/billing/migrations/0002_create_invoice.sql",
        ]
    );
    assert_eq!(
        read(&app, "modules/billing/src/invoice/mod.rs"),
        "use sofie::prelude::*;\n\n#[derive(Aggregate)]\npub struct Invoice {\n    meta: Meta,\n}\n"
    );
    assert_eq!(
        read(&app, "modules/billing/src/lib.rs"),
        "mod invoice;\npub mod module;\npub use invoice::Invoice;\n\npub mod areas {\n    pub mod web {\n        \
         pub mod routes;\n    }\n}\n"
    );
    assert_eq!(
        read(&app, "modules/billing/migrations/0002_create_invoice.sql"),
        "CREATE TABLE \"invoice\" (\n    id uuid PRIMARY KEY,\n    version bigint NOT NULL,\n    \
         created_at timestamptz NOT NULL,\n    updated_at timestamptz NOT NULL\n);\n"
    );
    let refused = usage(&app, &aggregate("Invoice"));
    assert!(refused.contains("has `Invoice`"), "{refused}");
    let refused = usage(&app, &aggregate("invoice"));
    assert!(
        refused.starts_with("`invoice` is not a type name"),
        "{refused}"
    );
}

#[test]
fn search_adds_the_behavior_its_column_and_an_index_built_concurrently() {
    let app = billing_app();
    app.write(
        "modules/billing/Cargo.toml",
        &manifest("billing", "sofie.workspace = true\n"),
    );
    generate(&app, &aggregate("Invoice"));
    let search = NewCode::Search {
        module: "billing".to_owned(),
        aggregate: "Invoice".to_owned(),
    };

    let changes = generate(&app, &search);

    assert_eq!(
        changes,
        [
            "changed modules/billing/src/invoice/mod.rs",
            "created modules/billing/migrations/0003_invoice_search_text.sql",
            "created modules/billing/migrations/0004_invoice_search_index.sql",
            "changed modules/billing/Cargo.toml",
        ]
    );
    assert_eq!(
        read(&app, "modules/billing/src/invoice/mod.rs"),
        "use sofie::prelude::*;\n\n#[derive(Aggregate)]\n#[aggregate(behaviors(Searched))]\npub struct Invoice {\n    meta: Meta,\n}\n"
    );
    assert!(
        read(
            &app,
            "modules/billing/migrations/0004_invoice_search_index.sql"
        )
        .starts_with(
            "CREATE INDEX CONCURRENTLY IF NOT EXISTS invoice_search ON \"invoice\" USING gin"
        )
    );
    assert!(
        read(&app, "modules/billing/Cargo.toml")
            .contains("sofie = { workspace = true, features = [\"search\"] }"),
        "{}",
        read(&app, "modules/billing/Cargo.toml")
    );
    let again = usage(&app, &search);
    assert!(again.contains("is searched already"), "{again}");
}

#[test]
fn a_module_with_sqlite_twins_gets_a_twin_of_each_new_migration() {
    let app = billing_app();
    app.write(
        "modules/billing/Cargo.toml",
        &manifest("billing", "sofie.workspace = true\n"),
    );
    app.write("modules/billing/migrations/sqlite/0001_start.sql", "");
    let migration = NewCode::Migration {
        module: "billing".to_owned(),
        name: "add_notes".to_owned(),
    };
    let search = NewCode::Search {
        module: "billing".to_owned(),
        aggregate: "Invoice".to_owned(),
    };

    let created: Vec<String> = [aggregate("Invoice"), search, migration]
        .iter()
        .flat_map(|code| generate(&app, code))
        .filter(|change| change.contains("/migrations/"))
        .collect();

    assert_eq!(
        created,
        [
            "created modules/billing/migrations/0002_create_invoice.sql",
            "created modules/billing/migrations/sqlite/0002_create_invoice.sql",
            "created modules/billing/migrations/0003_invoice_search_text.sql",
            "created modules/billing/migrations/sqlite/0003_invoice_search_text.sql",
            "created modules/billing/migrations/0004_invoice_search_index.sql",
            "created modules/billing/migrations/sqlite/0004_invoice_search_index.sql",
            "created modules/billing/migrations/0005_add_notes.sql",
            "created modules/billing/migrations/sqlite/0005_add_notes.sql",
        ]
    );
    let twin = |name: &str| read(&app, &format!("modules/billing/migrations/sqlite/{name}"));
    assert_eq!(
        twin("0002_create_invoice.sql"),
        "CREATE TABLE \"billing_invoice\" (\n    id uuid PRIMARY KEY NOT NULL,\n    \
         version bigint NOT NULL,\n    created_at timestamptz NOT NULL,\n    \
         updated_at timestamptz NOT NULL\n);\n"
    );
    assert!(twin("0003_invoice_search_text.sql").ends_with(
        "ALTER TABLE \"billing_invoice\" ADD COLUMN search_text text NOT NULL DEFAULT '';\n"
    ));
    assert_eq!(
        twin("0004_invoice_search_index.sql"),
        "-- SQLite needs no such step.\n"
    );
    assert_eq!(twin("0005_add_notes.sql"), "");
}

#[test]
fn an_event_is_declared_in_the_contract_and_a_listener_imports_it() {
    let app = billing_app();

    let event = generate(
        &app,
        &NewCode::Event {
            module: "billing".to_owned(),
            name: "InvoicePaid".to_owned(),
        },
    );
    let allowed = read(&app, "dead-code-allow.toml");
    let listener = generate(
        &app,
        &NewCode::Listener {
            module: "billing".to_owned(),
            event: "InvoicePaid".to_owned(),
            handler: "record_payment".to_owned(),
        },
    );

    assert_eq!(
        event,
        [
            "created modules/billing/contract/src/events.rs",
            "changed modules/billing/contract/src/lib.rs",
            "changed modules/billing/contract/Cargo.toml",
            "created dead-code-allow.toml",
        ]
    );
    assert!(allowed.contains(
        "[[item]]\npackage = \"billing-contract\"\nname = \"InvoicePaid\"\n\
             reason = \"scaffolded by `sofie new event`;"
    ));
    assert_eq!(
        read(&app, "modules/billing/contract/src/events.rs"),
        "use sofie::prelude::*;\n\n#[derive(Event)]\npub struct InvoicePaid {}\n"
    );
    assert_eq!(
        read(&app, "modules/billing/contract/src/lib.rs"),
        "//! What other modules may use of module `billing`.\npub mod events;\npub use events::InvoicePaid;\n"
    );
    assert!(
        read(&app, "modules/billing/contract/Cargo.toml")
            .ends_with("[dependencies]\nsofie.workspace = true\n")
    );
    assert_eq!(
        listener,
        [
            "created modules/billing/src/listeners.rs",
            "changed modules/billing/src/lib.rs",
            "changed modules/billing/Cargo.toml",
            "changed dead-code-allow.toml",
        ]
    );
    assert!(
        !read(&app, "dead-code-allow.toml").contains("InvoicePaid"),
        "the listener uses the event, so its allowance would be stale and fail the build"
    );
    assert_eq!(
        read(&app, "modules/billing/src/listeners.rs"),
        "use billing_contract::InvoicePaid;\nuse sofie::prelude::*;\n\n#[on]\npub async fn record_payment(_event: InvoicePaid) -> Result<()> {\n    Ok(())\n}\n"
    );
    assert!(read(&app, "modules/billing/src/lib.rs").contains("pub mod listeners;\n"));
    assert!(
        read(&app, "modules/billing/Cargo.toml")
            .contains("[dependencies]\nbilling-contract = { path = \"contract\" }\n")
    );
}

/// The facade's package is `sofie-framework`, its crate `sofie`.
#[test]
fn an_event_leaves_a_contract_already_on_the_framework_as_it_is() {
    let app = billing_app();
    app.write(
        "Cargo.toml",
        "[workspace]\nresolver = \"3\"\nmembers = [\"app\", \"modules/*\", \"modules/*/contract\"]\n\n\
         [workspace.dependencies]\nsofie = { package = \"sofie-framework\", path = \"sofie\" }\n",
    );
    app.write("sofie/Cargo.toml", &manifest("sofie-framework", ""));
    let manifest = manifest("billing-contract", "sofie.workspace = true\n");
    app.write("modules/billing/contract/Cargo.toml", &manifest);

    let changes = generate(
        &app,
        &NewCode::Event {
            module: "billing".to_owned(),
            name: "InvoicePaid".to_owned(),
        },
    );

    assert!(!changes.contains(&"changed modules/billing/contract/Cargo.toml".to_owned()));
    assert_eq!(read(&app, "modules/billing/contract/Cargo.toml"), manifest);
}

#[test]
fn a_route_adds_its_handler_line_and_screen() {
    let app = billing_app();
    generate(&app, &aggregate("Invoice"));

    let write = generate(
        &app,
        &route("post", "/invoices/{invoice}/pay", "pay_invoice", None),
    );
    let page = generate(
        &app,
        &route("get", "/invoices", "list_invoices", Some("invoices")),
    );

    assert_eq!(
        write,
        [
            "created modules/billing/src/actions.rs",
            "changed modules/billing/src/lib.rs",
            "changed modules/billing/src/areas/web/routes.rs",
            "changed modules/billing/contract/src/lib.rs",
        ]
    );
    assert_eq!(
        read(&app, "modules/billing/contract/src/lib.rs"),
        "//! What other modules may use of module `billing`.\nmod r#gen;\npub use r#gen::pruned;\npub use r#gen::routes;\n",
        "the contract exports the pruned events and route builders `sofie gen` writes, so `sofie check` passes"
    );
    assert_eq!(
        read(&app, "modules/billing/src/actions.rs"),
        "use crate::invoice::Invoice;\nuse sofie::prelude::*;\n\npub async fn pay_invoice(invoice: Invoice) -> Result<Invoice> {\n    Ok(invoice)\n}\n"
    );
    assert_eq!(
        page,
        [
            "created modules/billing/src/queries.rs",
            "changed modules/billing/src/lib.rs",
            "changed modules/billing/src/areas/web/routes.rs",
            "created modules/billing/src/areas/web/screens/invoices.html",
        ]
    );
    assert_eq!(
        read(&app, "modules/billing/src/areas/web/routes.rs"),
        "use crate::{actions, queries};\nuse sofie::prelude::*;\n\npub fn routes(routes: &mut Routes<Web>) {\n    \
         routes.post(\"/invoices/{invoice}/pay\", actions::pay_invoice);\n    \
         routes.screen(\"/invoices\", \"invoices\", queries::list_invoices);\n}\n"
    );
    assert_eq!(
        read(&app, "modules/billing/src/areas/web/screens/invoices.html"),
        "<Section title=\"Invoices\">\n</Section>\n"
    );
    assert_eq!(
        read(&app, "modules/billing/src/queries.rs"),
        "use sofie::prelude::*;\n\n#[derive(ReadModel)]\npub struct ListInvoices {}\n\n\
         pub async fn list_invoices(_read: Read) -> Result<ListInvoices> {\n    \
         Ok(ListInvoices {})\n}\n"
    );
}

#[test]
fn a_get_naming_an_aggregate_loads_a_read_model_of_it_from_the_path() {
    let app = billing_app();
    generate(&app, &aggregate("Invoice"));

    generate(
        &app,
        &route(
            "get",
            "/invoices/{invoice}",
            "show_invoice",
            Some("invoice"),
        ),
    );
    generate(
        &app,
        &route("get", "/invoices/{invoice}/raw", "raw_invoice", None),
    );

    assert_eq!(
        read(&app, "modules/billing/src/queries.rs"),
        "use crate::invoice::Invoice;\nuse sofie::prelude::*;\n\n\
         #[derive(ReadModel)]\n#[read(from = Invoice)]\npub struct ShowInvoice {\n    id: Id<Invoice>,\n}\n\n\
         pub async fn show_invoice(invoice: ShowInvoice) -> Result<ShowInvoice> {\n    Ok(invoice)\n}\n\n\
         #[derive(ReadModel)]\n#[read(from = Invoice)]\npub struct RawInvoice {\n    id: Id<Invoice>,\n}\n\n\
         pub async fn raw_invoice(invoice: RawInvoice) -> Result<RawInvoice> {\n    Ok(invoice)\n}\n"
    );
}

#[test]
fn a_read_model_named_like_the_aggregate_it_reads_is_refused() {
    let app = billing_app();
    generate(&app, &aggregate("Invoice"));

    let refused = usage(&app, &route("get", "/invoices/{invoice}", "invoice", None));

    assert!(
        refused.ends_with(
            "the read model would be named `Invoice`, like the aggregate it reads; choose another \
             handler name or a `read_model`"
        ),
        "{refused}"
    );
    assert!(!app.path("modules/billing/src/queries.rs").exists());
}

#[test]
fn a_read_model_named_like_a_prelude_type_is_refused() {
    let app = billing_app();

    let refused = usage(&app, &route("get", "/session", "session", Some("session")));

    assert!(
        refused.starts_with("`Session` is a name a prelude brings into every file"),
        "{refused}"
    );
}

const INVOICE_WITH_FIELDS: &str = "use sofie::prelude::*;\nuse sofie::values::Secret;\n\
     use sofie_identity::AccountId as Payer;\n\n\
     #[derive(Aggregate)]\npub struct Invoice {\n    meta: Meta,\n    payer: Payer,\n    \
     status: InvoiceStatus,\n    total: Money,\n    #[sensitive]\n    access_code: String,\n    \
     recovery: Option<Secret<String>>,\n    lines: Children<Line>,\n}\n\n#[derive(Clone, Copy, Debug, PartialEq, Eq, Enum)]\n\
     pub enum InvoiceStatus {\n    Open,\n    Paid,\n}\n";

fn route_reading(path: &str, handler: &str, fields: &[&str]) -> NewCode {
    NewCode::Route(NewRoute {
        module: "billing".to_owned(),
        area: "web".to_owned(),
        method: "get".to_owned(),
        path: path.to_owned(),
        handler: handler.to_owned(),
        screen: Some(handler.to_owned()),
        fields: Some(fields.iter().map(|field| (*field).to_owned()).collect()),
        read_model: None,
        title: None,
        back: None,
    })
}

#[test]
fn a_get_reads_the_aggregate_fields_and_its_screen_shows_them() {
    let app = billing_app();
    generate(&app, &aggregate("Invoice"));
    app.write("modules/billing/src/invoice/mod.rs", INVOICE_WITH_FIELDS);

    let changes = generate(
        &app,
        &route(
            "get",
            "/invoices/{invoice}",
            "show_invoice",
            Some("invoice"),
        ),
    );

    assert_eq!(
        read(&app, "modules/billing/src/queries.rs"),
        "use crate::invoice::{Invoice, InvoiceStatus};\nuse sofie::prelude::*;\n\n\
         #[derive(ReadModel)]\n#[read(from = Invoice)]\npub struct ShowInvoice {\n    id: Id<Invoice>,\n    \
         status: InvoiceStatus,\n    total: Money,\n}\n\n\
         pub async fn show_invoice(invoice: ShowInvoice) -> Result<ShowInvoice> {\n    Ok(invoice)\n}\n"
    );
    let screen = read(&app, "modules/billing/src/areas/web/screens/invoice.html");
    assert_eq!(
        screen,
        "<Section title=\"Invoice\">\n  <dl>\n    \
         <dt>Status</dt>\n    <dd>{data.status}</dd>\n    <dt>Total</dt>\n    \
         <dd><Money value={data.total} /></dd>\n  </dl>\n</Section>\n"
    );
    sofie_engine::Template::parse("invoice", &screen).unwrap();
    let notes: Vec<&String> = changes
        .iter()
        .filter(|change| change.starts_with("note: "))
        .collect();
    assert_eq!(
        notes,
        [
            "note: left out `payer`: it names a record of `Account`, which the viewer may not \
             reach; name it in `fields` to read it",
            "note: left out `access_code`: a secret, by its `#[sensitive]` mark or its type; name it in \
             `fields` to read it",
            "note: left out `recovery`: a secret, by its `#[sensitive]` mark or its type; name it in \
             `fields` to read it",
            "note: no back link: area `web` has no page listing `Invoice`; `back` names one",
        ]
    );
}

const PLACED_ORDER: &str = "use sofie::prelude::*;\nuse sofie_identity::AccountId;\n\n\
     #[derive(Aggregate)]\npub struct Invoice {\n    meta: Meta,\n    #[owner]\n    account: AccountId,\n    \
     customer: Id<Customer>,\n    total: Money,\n    placed_at: Instant,\n}\n";

fn show_placed_invoice(app: &TemporaryDirectory) -> (Vec<String>, String) {
    generate(app, &aggregate("Invoice"));
    app.write("modules/billing/src/invoice/mod.rs", PLACED_ORDER);
    let changes = generate(
        app,
        &route(
            "get",
            "/invoices/{invoice}",
            "show_invoice",
            Some("invoice"),
        ),
    );
    let screen = read(app, "modules/billing/src/areas/web/screens/invoice.html");
    (changes, screen)
}

#[test]
fn a_page_leaves_out_the_owner_and_other_records_and_says_so_unless_fields_name_them() {
    let app = billing_app();

    let (changes, screen) = show_placed_invoice(&app);
    generate(
        &app,
        &route_reading("/invoices/{invoice}/owner", "invoice_owner", &["account"]),
    );

    assert!(
        !screen.contains("account") && !screen.contains("customer"),
        "{screen}"
    );
    for note in [
        "note: left out `account`: the record's owner, which the viewer knows; name it in \
         `fields` to read it",
        "note: left out `customer`: it names a record of `Customer`, which the viewer may not \
         reach; name it in `fields` to read it",
    ] {
        assert!(changes.iter().any(|change| change == note), "{changes:#?}");
    }
    assert!(
        read(&app, "modules/billing/src/queries.rs").contains(
            "pub struct InvoiceOwner {\n    id: Id<Invoice>,\n    account: AccountId,\n}"
        )
    );
}

#[test]
fn an_instant_is_formatted_by_date_whatever_features_the_app_builds() {
    let app = billing_app();

    let (changes, screen) = show_placed_invoice(&app);

    assert!(
        screen.contains("<dd>{date(data.placed_at)}</dd>"),
        "{screen}"
    );
    assert!(
        changes.iter().all(|change| !change.contains("placed_at")),
        "{changes:#?}"
    );
}

#[test]
fn a_get_leaves_out_a_field_of_a_sensitive_value_the_module_imports() {
    let app = billing_app();
    generate(&app, &aggregate("Invoice"));
    app.write(
        "modules/billing/src/tax.rs",
        "use sofie::prelude::*;\n\n#[derive(Value)]\n#[sensitive]\npub struct TaxNumber(String);\n",
    );
    let library = read(&app, "modules/billing/src/lib.rs");
    app.write(
        "modules/billing/src/lib.rs",
        &format!("mod tax;\n{library}"),
    );
    app.write(
        "modules/billing/src/invoice/mod.rs",
        "use sofie::prelude::*;\n\nuse crate::tax::TaxNumber;\n\n#[derive(Aggregate)]\npub struct Invoice {\n    \
         meta: Meta,\n    total: Money,\n    tax_number: TaxNumber,\n}\n",
    );

    generate(
        &app,
        &route("get", "/invoices/{invoice}", "show_invoice", None),
    );

    let queries = read(&app, "modules/billing/src/queries.rs");
    assert!(
        queries.contains("pub struct ShowInvoice {\n    id: Id<Invoice>,\n    total: Money,\n}"),
        "{queries}"
    );
}

#[test]
fn a_get_keeps_personal_data_such_as_an_email_and_leaves_out_a_secret() {
    let app = billing_app();
    generate(&app, &aggregate("Invoice"));
    app.write(
        "modules/billing/src/invoice/mod.rs",
        "use sofie::prelude::*;\n\n#[derive(Aggregate)]\npub struct Invoice {\n    meta: Meta,\n    \
         email: Email,\n    token: Secret<String>,\n}\n",
    );

    let changes = generate(
        &app,
        &route("get", "/invoices/{invoice}", "show_invoice", None),
    );

    let queries = read(&app, "modules/billing/src/queries.rs");
    assert!(
        queries.contains("pub struct ShowInvoice {\n    id: Id<Invoice>,\n    email: Email,\n}"),
        "{queries}"
    );
    assert!(
        changes.iter().any(|change| change
            == "note: left out `token`: a secret, by its `#[sensitive]` mark or its type; name it \
                in `fields` to read it"),
        "{changes:#?}"
    );
}

#[test]
fn a_read_model_named_like_a_module_type_or_an_import_is_refused() {
    let app = billing_app();
    generate(&app, &aggregate("Invoice"));
    app.write("modules/billing/src/invoice/mod.rs", INVOICE_WITH_FIELDS);
    let named = |read_model: &str| {
        NewCode::Route(NewRoute {
            module: "billing".to_owned(),
            area: "web".to_owned(),
            method: "get".to_owned(),
            path: "/invoices/{invoice}".to_owned(),
            handler: "show_invoice".to_owned(),
            screen: None,
            fields: Some(vec!["payer".to_owned()]),
            read_model: Some(read_model.to_owned()),
            title: None,
            back: None,
        })
    };

    let declared = usage(&app, &named("InvoiceStatus"));
    let imported = usage(&app, &named("Payer"));

    assert!(
        declared.contains("`InvoiceStatus` is a type of module `billing`"),
        "{declared}"
    );
    assert!(
        imported.contains("`Payer` is a name the handler's file"),
        "{imported}"
    );
}

#[test]
fn a_get_leaves_out_a_field_of_a_sensitive_value_another_crate_declares() {
    let app = billing_app();
    generate(&app, &aggregate("Invoice"));
    app.write("modules/tax/Cargo.toml", &manifest("tax", ""));
    app.write(
        "modules/tax/src/lib.rs",
        "pub mod module;\n\nuse sofie::prelude::*;\n\n#[derive(Value)]\n#[sensitive]\npub struct TaxNumber(String);\n",
    );
    app.write(
        "modules/tax/src/module.rs",
        &read(&app, "modules/billing/src/module.rs").replace("billing", "tax"),
    );
    app.write(
        "modules/billing/src/invoice/mod.rs",
        "use sofie::prelude::*;\n\n#[derive(Aggregate)]\npub struct Invoice {\n    \
         meta: Meta,\n    total: Money,\n    tax_number: TaxNumber,\n}\n",
    );

    generate(
        &app,
        &route("get", "/invoices/{invoice}", "show_invoice", None),
    );

    let queries = read(&app, "modules/billing/src/queries.rs");
    assert!(
        queries.contains("pub struct ShowInvoice {\n    id: Id<Invoice>,\n    total: Money,\n}"),
        "{queries}"
    );
}

#[test]
fn a_get_reads_only_the_fields_named() {
    let app = billing_app();
    generate(&app, &aggregate("Invoice"));
    app.write("modules/billing/src/invoice/mod.rs", INVOICE_WITH_FIELDS);

    generate(
        &app,
        &route_reading(
            "/invoices/{invoice}",
            "invoice_code",
            &["id", "access_code", "total", "recovery"],
        ),
    );
    let unknown = usage(
        &app,
        &route_reading("/invoices/{invoice}/lines", "invoice_lines", &["lines"]),
    );
    let unloaded = usage(&app, &route_reading("/invoices", "invoices", &["total"]));

    assert!(
        read(&app, "modules/billing/src/queries.rs").contains(
            "pub struct InvoiceCode {\n    id: Id<Invoice>,\n    #[sensitive]\n    access_code: String,\n    \
             total: Money,\n    #[sensitive]\n    recovery: Option<Secret<String>>,\n}"
        )
    );
    assert_eq!(
        unknown,
        "`Invoice` has no field `lines` a read model can read; its fields: payer, status, total, \
         access_code, recovery"
    );
    assert!(unloaded.contains("this route loads none"), "{unloaded}");
}

#[test]
fn a_field_of_an_aggregate_without_fields_is_refused_saying_it_has_none() {
    let app = billing_app();
    generate(&app, &aggregate("Invoice"));

    let refused = usage(
        &app,
        &route_reading("/invoices/{invoice}", "show_invoice", &["number"]),
    );

    assert_eq!(
        refused,
        "`Invoice` has no field `number` a read model can read; it has no readable field besides `id`"
    );
}

fn invoice_page(fields: Option<&[&str]>, title: Option<&str>, back: Option<&str>) -> NewCode {
    NewCode::Route(NewRoute {
        module: "billing".to_owned(),
        area: "web".to_owned(),
        method: "get".to_owned(),
        path: "/invoices/{invoice}".to_owned(),
        handler: "show_invoice".to_owned(),
        screen: Some("invoice".to_owned()),
        fields: fields.map(|fields| fields.iter().map(|field| (*field).to_owned()).collect()),
        read_model: None,
        title: title.map(str::to_owned),
        back: back.map(str::to_owned),
    })
}

/// As the server shows the screen, whose own name would title the page.
fn rendered_page(screen: &str) -> String {
    let body = sofie_engine::Template::parse("invoice", screen)
        .unwrap()
        .render(&serde_json::json!({}))
        .unwrap();
    sofie_engine::Page {
        title: "Invoice",
        lang: "en",
        body: &body,
        notice: &[],
        nonce: "nonce",
        islands: None,
        script_notices: sofie_engine::ScriptNotices::plain(),
    }
    .render()
}

fn notes(changes: &[String]) -> Vec<&str> {
    changes
        .iter()
        .filter_map(|change| change.strip_prefix("note: "))
        .collect()
}

#[test]
fn a_record_page_links_back_to_the_area_page_whose_read_model_lists_the_aggregate() {
    let app = billing_app();
    generate(&app, &aggregate("Invoice"));
    app.write("modules/billing/src/invoice/mod.rs", INVOICE_WITH_FIELDS);
    app.write(
        "modules/billing/src/queries.rs",
        "use sofie::prelude::*;\n\nuse crate::invoice::Invoice;\n\n\
         #[derive(ReadModel)]\n#[read(from = Invoice)]\npub struct InvoiceRow {\n    id: Id<Invoice>,\n}\n\n\
         pub async fn list_bills(read: Read) -> Result<Vec<InvoiceRow>> {\n    \
         InvoiceRow::query().all(&read).await\n}\n",
    );
    let library = read(&app, "modules/billing/src/lib.rs");
    app.write(
        "modules/billing/src/lib.rs",
        &format!("pub mod queries;\n{library}"),
    );
    app.write(
        "modules/billing/src/areas/web/routes.rs",
        "use sofie::prelude::*;\n\nuse crate::queries;\n\n\
         pub fn routes(routes: &mut Routes<Web>) {\n    routes.screen(\"/bills\", \"bills\", queries::list_bills);\n}\n",
    );

    let changes = generate(
        &app,
        &invoice_page(Some(&["total"]), Some("Your invoice"), None),
    );

    let screen = read(&app, "modules/billing/src/areas/web/screens/invoice.html");
    assert_eq!(
        screen,
        "<title>Your invoice</title>\n<Section title=\"Your invoice\">\n  \
         <a href=\"/bills\">Back to bills</a>\n  <dl>\n    \
         <dt>Total</dt>\n    <dd><Money value={data.total} /></dd>\n  </dl>\n</Section>\n"
    );
    let page = rendered_page(&screen);
    let (head, body) = page.split_once("</head>").unwrap();
    assert!(head.contains("<title>Your invoice</title>"), "{head}");
    assert!(body.contains("<h2>Your invoice</h2>"), "{body}");
    assert_eq!(
        notes(&changes),
        [
            "back link to /bills (`list_bills`): it lists `Invoice` through `InvoiceRow`; `back` \
          names another page or `none`"
        ]
    );
}

#[test]
fn a_record_page_links_back_to_the_page_at_its_path_s_plural_unless_back_says_otherwise() {
    let app = billing_app();
    generate(&app, &aggregate("Invoice"));
    app.write("modules/billing/src/invoice/mod.rs", INVOICE_WITH_FIELDS);
    generate(
        &app,
        &route("get", "/invoices", "list_invoices", Some("invoices")),
    );
    generate(&app, &route("get", "/", "home", Some("home")));

    let found = generate(&app, &invoice_page(Some(&["total"]), None, None));
    let found_screen = read(&app, "modules/billing/src/areas/web/screens/invoice.html");
    fs::remove_file(app.path("modules/billing/src/areas/web/screens/invoice.html")).unwrap();
    let mut named = invoice_page(Some(&["total"]), None, Some("home"));
    if let NewCode::Route(route) = &mut named {
        route.path = "/invoice/{invoice}".to_owned();
        route.handler = "invoice_named".to_owned();
    }
    let named_changes = generate(&app, &named);
    let named_screen = read(&app, "modules/billing/src/areas/web/screens/invoice.html");

    assert!(
        found_screen.starts_with(
            "<Section title=\"Invoice\">\n  <a href=\"/invoices\">Back to invoices</a>\n"
        ),
        "{found_screen}"
    );
    assert_eq!(
        notes(&found),
        [
            "back link to /invoices (`list_invoices`): its path is the plural of the record's; \
          `back` names another page or `none`"
        ]
    );
    assert!(
        named_screen.contains("  <a href=\"/\">Back to home</a>\n"),
        "{named_screen}"
    );
    assert_eq!(
        notes(&named_changes),
        ["back link to / (`home`), as `back` names it"]
    );
}

#[test]
fn back_none_leaves_the_link_out_and_back_naming_no_page_is_refused() {
    let app = billing_app();
    generate(&app, &aggregate("Invoice"));
    app.write("modules/billing/src/invoice/mod.rs", INVOICE_WITH_FIELDS);
    generate(
        &app,
        &route("get", "/invoices", "list_invoices", Some("invoices")),
    );

    let unknown = usage(&app, &invoice_page(None, None, Some("missing")));
    let off = generate(&app, &invoice_page(Some(&["total"]), None, Some("none")));

    assert!(
        unknown.contains("no such page answers `missing`; its pages: list_invoices"),
        "{unknown}"
    );
    assert!(notes(&off).is_empty(), "{off:#?}");
    assert!(!read(&app, "modules/billing/src/areas/web/screens/invoice.html").contains("<a "),);
}

#[test]
fn a_record_page_shows_its_id_only_when_fields_name_it() {
    let app = billing_app();
    generate(&app, &aggregate("Invoice"));
    app.write("modules/billing/src/invoice/mod.rs", INVOICE_WITH_FIELDS);

    generate(
        &app,
        &invoice_page(Some(&["id", "total"]), None, Some("none")),
    );

    let screen = read(&app, "modules/billing/src/areas/web/screens/invoice.html");
    assert!(
        screen.contains("    <dt>Id</dt>\n    <dd>{data.id}</dd>\n    <dt>Total</dt>"),
        "{screen}"
    );
}

#[test]
fn a_title_or_back_without_a_screen_or_a_title_carrying_markup_is_refused() {
    let app = billing_app();
    generate(&app, &aggregate("Invoice"));
    app.write("modules/billing/src/invoice/mod.rs", INVOICE_WITH_FIELDS);
    let mut screenless = invoice_page(None, Some("Invoice"), None);
    if let NewCode::Route(route) = &mut screenless {
        route.screen = None;
    }

    let without_screen = usage(&app, &screenless);
    let markup = usage(&app, &invoice_page(None, Some("<b>\"x\"</b>"), None));

    assert!(
        without_screen.contains("route it with a `screen`"),
        "{without_screen}"
    );
    assert!(markup.contains("is not a page title"), "{markup}");
}

#[test]
fn a_get_answers_the_read_model_named() {
    let app = billing_app();
    generate(&app, &aggregate("Invoice"));
    let named = |read_model: &str, path: &str, handler: &str, screen: Option<&str>| {
        let NewCode::Route(mut new_route) = route("get", path, handler, screen) else {
            unreachable!()
        };
        new_route.read_model = Some(read_model.to_owned());
        NewCode::Route(new_route)
    };

    generate(
        &app,
        &named("InvoiceDetail", "/invoices/{invoice}", "show_invoice", None),
    );
    let taken = usage(
        &app,
        &named(
            "InvoiceDetail",
            "/invoices/{invoice}/raw",
            "raw_invoice",
            None,
        ),
    );
    let lowercase = usage(&app, &named("detail", "/b/{invoice}", "b", None));
    let unanswered = usage(&app, &named("Listing", "/invoices", "list_invoices", None));

    assert!(read(&app, "modules/billing/src/queries.rs").contains(
        "pub struct InvoiceDetail {\n    id: Id<Invoice>,\n}\n\n\
             pub async fn show_invoice(invoice: InvoiceDetail) -> Result<InvoiceDetail> {"
    ));
    assert!(
        taken.ends_with("has a read model `InvoiceDetail` already; choose another `read_model`"),
        "{taken}"
    );
    assert!(
        lowercase.starts_with("`detail` is not a type name"),
        "{lowercase}"
    );
    assert!(
        unanswered.contains("route it without `read_model`"),
        "{unanswered}"
    );
}

#[test]
fn a_route_path_repeating_the_area_prefix_is_refused_with_the_relative_path() {
    let app = billing_app();
    app.write("modules/billing/contract/src/lib.rs", "pub mod area;\n");
    app.write(
        "modules/billing/contract/src/area.rs",
        "pub struct Member;\n\nimpl Area for Member {\n    const NAME: &'static str = \"member\";\n    \
         const PREFIX: &'static str = \"/account\";\n    type Principal = sofie_identity::Account;\n}\n",
    );
    let in_member = |path: &str| {
        let NewCode::Route(mut new_route) = route("get", path, "show_order", None) else {
            unreachable!()
        };
        new_route.area = "member".to_owned();
        NewCode::Route(new_route)
    };

    let repeated = usage(&app, &in_member("/account/orders/{order}"));
    let prefix_alone = usage(&app, &in_member("/account"));
    generate(&app, &in_member("/accounts"));

    assert_eq!(
        repeated,
        "`/account/orders/{order}` repeats area `member`'s prefix `/account`: route paths are \
         relative to it, so write `/orders/{order}`"
    );
    assert!(prefix_alone.ends_with("so write `/`"), "{prefix_alone}");
}

#[test]
fn a_new_area_refuses_a_prefix_or_a_type_name_already_taken() {
    let app = billing_app();
    app.write("modules/billing/contract/src/lib.rs", "pub mod area;\n");
    app.write(
        "modules/billing/contract/src/area.rs",
        "pub struct Ledger;\n\npub struct Member;\n\nimpl Area for Member {\n    \
         const NAME: &'static str = \"member\";\n    const PREFIX: &'static str = \"/account\";\n    \
         type Principal = sofie_identity::Account;\n}\n",
    );
    let area = |name: &str| NewCode::Area {
        module: "billing".to_owned(),
        name: name.to_owned(),
        access: AreaAccess::Public,
    };

    let prefix = usage(&app, &area("account"));
    let type_name = usage(&app, &area("ledger"));
    let principal = usage(&app, &area("visitor"));

    assert_eq!(
        prefix,
        "area `member` serves under the prefix `/account` already, which area `account` would \
         take; choose another name"
    );
    assert_eq!(
        type_name,
        "`Ledger`, the type area `ledger` would define, is a name \
         modules/billing/contract/src/area.rs declares or imports already; choose another name"
    );
    assert!(
        principal.starts_with("`Visitor`, the type area `visitor` would define, is a name"),
        "{principal}"
    );
}

#[test]
fn a_route_refuses_what_it_cannot_write() {
    let app = billing_app();
    generate(&app, &route("get", "/invoices", "list_invoices", None));

    let cases = [
        (route("fetch", "/a", "a", None), "`fetch` is not a method"),
        (route("get", "a", "a", None), "`a` is not a route path"),
        (
            route("get", "/a/../admin", "a", None),
            "has the segment `..`",
        ),
        (route("get", "/a/{id", "a", None), "malformed parameter"),
        (
            route("post", "/a", "a", Some("a")),
            "a screen is shown by a `get` route",
        ),
        (
            route("get", "/invoices", "other", None),
            "GET /invoices is routed already",
        ),
        (
            route("get", "/b", "list_invoices", None),
            "has a function `list_invoices` already",
        ),
    ];

    for (code, expected) in cases {
        let message = usage(&app, &code);
        assert!(message.contains(expected), "{message}");
    }
}

#[test]
fn a_task_turns_tasks_on_once_and_refuses_a_main_it_cannot_change_or_a_taken_name() {
    let app = billing_app();
    let task = |name: &str| NewCode::Task {
        module: "billing".to_owned(),
        name: name.to_owned(),
    };
    let unchanged = usage(&app, &task("reindex"));
    app.write(
        "app/src/main.rs",
        "fn main() -> sofie::core::Result<()> {\n    sofie::run(shop::MODULES, shop::app, shop::worker)\n}\n",
    );
    app.write(
        "app/src/lib.rs",
        "mod wiring;\n\npub use wiring::{MODULES, worker};\n",
    );
    app.write(
        "modules/billing/Cargo.toml",
        &manifest("billing", "sofie.workspace = true\n"),
    );

    let first = generate(&app, &task("reindex"));
    let second = generate(&app, &task("recount"));

    assert!(
        unchanged.contains("does not call `sofie::run` with `shop::worker`"),
        "{unchanged}"
    );
    assert_eq!(
        first,
        [
            "changed app/src/main.rs",
            "changed app/src/lib.rs",
            "changed modules/billing/Cargo.toml",
            "created modules/billing/src/tasks.rs",
            "changed modules/billing/src/lib.rs",
        ]
    );
    assert_eq!(second, ["changed modules/billing/src/tasks.rs"]);
    assert_eq!(
        read(&app, "modules/billing/src/tasks.rs"),
        "use sofie::prelude::*;\n\n#[task]\npub async fn reindex() -> Result<()> {\n    Ok(())\n}\n\n\
         #[task]\npub async fn recount() -> Result<()> {\n    Ok(())\n}\n"
    );
    assert!(read(&app, "app/src/lib.rs").contains("pub use wiring::{MODULES, tasks, worker};"));
    assert!(
        read(&app, "modules/billing/Cargo.toml")
            .contains("sofie = { workspace = true, features = [\"tasks\"] }")
    );
    let taken = usage(&app, &task("reindex"));
    assert!(taken.contains("has a task `reindex` already"), "{taken}");
}

#[test]
fn a_plan_joins_the_modules_catalog_and_turns_billing_on() {
    let app = billing_app();
    app.write(
        "modules/billing/Cargo.toml",
        &manifest("billing", "sofie.workspace = true\n"),
    );
    let plan = |name: &str| NewCode::Plan {
        module: "billing".to_owned(),
        name: name.to_owned(),
    };

    let first = generate(&app, &plan("free"));
    let second = generate(&app, &plan("pro"));

    assert_eq!(
        first,
        [
            "changed modules/billing/Cargo.toml",
            "created modules/billing/src/plans.rs",
            "changed modules/billing/src/lib.rs",
        ]
    );
    assert_eq!(second, ["changed modules/billing/src/plans.rs"]);
    assert_eq!(
        read(&app, "modules/billing/src/plans.rs"),
        "use sofie::billing::{Plan, Plans};\n\npub const FREE: Plan = Plan::new(\"free\");\n\n\
         pub const PRO: Plan = Plan::new(\"pro\");\n\n\
         pub const PLANS: Plans = Plans::new(FREE, &[PRO]);\n"
    );
    assert!(read(&app, "modules/billing/src/lib.rs").contains("pub mod plans;"));
    assert!(
        read(&app, "modules/billing/Cargo.toml")
            .contains("sofie = { workspace = true, features = [\"billing\"] }")
    );
    let taken = usage(&app, &plan("pro"));
    assert!(taken.contains("declares plan `pro` already"), "{taken}");
}

#[test]
fn a_plan_may_not_take_the_catalogs_name() {
    let app = billing_app();
    let refused = usage(
        &app,
        &NewCode::Plan {
            module: "billing".to_owned(),
            name: "plans".to_owned(),
        },
    );

    assert!(refused.contains("`PLANS` names the catalog"), "{refused}");
    assert!(!app.path("modules/billing/src/plans.rs").exists());
}

#[cfg(feature = "slow-tests")]
mod shop {
    use std::env;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};

    use sofie_model::testing::TemporaryDirectory;

    fn copy_tree(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            if ["target", "node_modules"]
                .iter()
                .any(|skipped| entry.file_name() == *skipped)
            {
                continue;
            }
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    /// The copy names the framework's crates by absolute path, so Cargo reuses their builds in the shared target directory.
    fn shop_copy(directory: &TemporaryDirectory) -> PathBuf {
        let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let workspace = fs::read_to_string(repository.join("Cargo.toml")).unwrap();
        let members_start = workspace.find("members = [").unwrap();
        let members_end =
            workspace.find("exclude = [\"bench\"]\n").unwrap() + "exclude = [\"bench\"]\n".len();
        let workspace = format!(
            "{}members = [\"shop/app\", \"shop/modules/*\", \"shop/modules/*/contract\"]\n{}",
            &workspace[..members_start],
            &workspace[members_end..]
        )
        .replace(
            "path = \"crates/",
            &format!("path = \"{}/crates/", repository.display()),
        );
        directory.write("Cargo.toml", &workspace);
        fs::copy(repository.join("Cargo.lock"), directory.path("Cargo.lock")).unwrap();
        let shop = directory.path("shop");
        copy_tree(&repository.join("examples/shop"), &shop);
        shop
    }

    fn sofie(shop: &Path, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_sofie"))
            .args(arguments)
            .arg(shop)
            .output()
            .unwrap()
    }

    fn shared_target() -> PathBuf {
        let executable = env::current_exe().unwrap();
        executable.ancestors().nth(3).unwrap().to_path_buf()
    }

    #[test]
    fn generated_code_in_the_shop_passes_gen_check_and_cargo_check() {
        let directory = TemporaryDirectory::new("sofie-new-shop");
        let shop = shop_copy(&directory);

        let aggregate = sofie(&shop, &["new", "aggregate", "order", "Invoice"]);
        let write = sofie(
            &shop,
            &[
                "new",
                "route",
                "order",
                "customer",
                "post",
                "/invoices/{invoice}/pay",
                "pay_invoice",
            ],
        );
        let page = sofie(
            &shop,
            &[
                "new",
                "route",
                "order",
                "customer",
                "get",
                "/invoices",
                "list_invoices",
                "--screen",
                "invoices",
            ],
        );
        let detail = sofie(
            &shop,
            &[
                "new",
                "route",
                "order",
                "customer",
                "get",
                "/orders/{order}",
                "show_order",
                "--screen",
                "order",
            ],
        );
        let task = sofie(&shop, &["new", "task", "order", "recount"]);
        let menu = sofie(
            &shop,
            &["new", "menu", "order", "customer", "list_orders", "Orders"],
        );
        let check = sofie(&shop, &["check"]);
        let tasks = sofie(&shop, &["tasks"]);
        let cargo = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .args([
                "check",
                "--offline",
                "--quiet",
                "--package",
                "shop",
                "--target-dir",
            ])
            .arg(shared_target())
            .current_dir(directory.root())
            .output()
            .unwrap();

        for output in [
            &aggregate, &write, &page, &detail, &task, &menu, &check, &tasks,
        ] {
            assert_eq!(output.status.code(), Some(0), "{output:?}");
        }
        assert_eq!(String::from_utf8_lossy(&check.stdout), "ok\n");
        assert!(
            String::from_utf8_lossy(&tasks.stdout).contains("order.recount"),
            "{tasks:?}"
        );
        assert_eq!(
            fs::read_to_string(shop.join("app/src/main.rs")).unwrap(),
            "fn main() -> sofie::core::Result<()> {\n    \
             sofie::run_with_tasks(shop::MODULES, shop::app, shop::worker, shop::tasks)\n}\n"
        );
        assert!(
            cargo.status.success(),
            "{}",
            String::from_utf8_lossy(&cargo.stderr)
        );
    }
}

#[test]
fn a_route_method_is_taken_in_either_case() {
    let app = billing_app();

    generate(&app, &route("GET", "/invoices", "list_invoices", None));

    assert!(
        read(&app, "modules/billing/src/areas/web/routes.rs")
            .contains("routes.get(\"/invoices\", queries::list_invoices);")
    );
}

fn menu_entry(module: &str, area: &str, handler: &str, label: &str) -> NewCode {
    NewCode::MenuEntry(NewMenuEntry {
        module: module.to_owned(),
        area: area.to_owned(),
        handler: handler.to_owned(),
        label: label.to_owned(),
    })
}

#[test]
fn a_menu_entry_creates_the_menu_then_appends_to_it() {
    let app = billing_app();
    app.write(
        "modules/billing/Cargo.toml",
        &manifest("billing", "sofie.workspace = true\n"),
    );
    generate(&app, &route("get", "/invoices", "list_invoices", None));
    generate(&app, &route("get", "/archive", "list_archive", None));

    let first = generate(
        &app,
        &menu_entry("billing", "web", "list_invoices", "Invoices"),
    );
    let second = generate(
        &app,
        &menu_entry("billing", "web", "list_archive", "Due \"now\""),
    );

    assert_eq!(
        first,
        [
            "created modules/billing/src/areas/web/menu.rs",
            "changed modules/billing/src/lib.rs",
            "changed modules/billing/Cargo.toml",
        ]
    );
    assert_eq!(second, ["changed modules/billing/src/areas/web/menu.rs"]);
    assert_eq!(
        read(&app, "modules/billing/src/areas/web/menu.rs"),
        "use sofie::prelude::*;\n\npub fn menu(menu: &mut Menu<Web>) {\n    \
         menu.entry(\"Invoices\", billing_contract::routes::web::list_invoices());\n    \
         menu.entry(\"Due \\\"now\\\"\", billing_contract::routes::web::list_archive());\n}\n"
    );
    assert!(read(&app, "modules/billing/src/lib.rs").contains("pub mod menu;"));
    let manifest = read(&app, "modules/billing/Cargo.toml");
    assert!(
        manifest.contains("sofie = { workspace = true, features = [\"menu\"] }"),
        "{manifest}"
    );
    assert!(
        manifest.contains("billing-contract = { path = \"contract\" }"),
        "{manifest}"
    );
}

#[test]
fn a_menu_entry_refuses_what_no_menu_can_link_to() {
    let app = billing_app();
    app.write(
        "modules/billing/Cargo.toml",
        &manifest("billing", "sofie.workspace = true\n"),
    );
    generate(&app, &route("get", "/invoices", "list_invoices", None));
    generate(
        &app,
        &route("get", "/invoices/{invoice}", "show_invoice", None),
    );
    generate(&app, &route("post", "/invoices", "create_invoice", None));
    generate(
        &app,
        &menu_entry("billing", "web", "list_invoices", "Invoices"),
    );

    let cases = [
        (
            menu_entry("shipping", "web", "list_invoices", "Invoices"),
            "the app has no module `shipping`",
        ),
        (
            menu_entry("billing", "console", "list_invoices", "Invoices"),
            "module `billing` routes nothing in area `console`",
        ),
        (
            menu_entry("billing", "web", "list_refunds", "Refunds"),
            "area `web` routes no handler `list_refunds`; its pages: list_invoices",
        ),
        (
            menu_entry("billing", "web", "create_invoice", "New"),
            "`create_invoice` answers POST /invoices in area `web`, which is no page",
        ),
        (
            menu_entry("billing", "web", "show_invoice", "Invoice"),
            "a page of the area without parameters; GET /invoices/{invoice} takes `invoice`",
        ),
        (
            menu_entry("billing", "web", "list_invoices", "Again"),
            "has an entry for `routes::web::list_invoices()` already",
        ),
        (
            menu_entry("billing", "web", "list_invoices", " "),
            "a menu entry needs a label",
        ),
    ];

    for (code, expected) in cases {
        let message = usage(&app, &code);
        assert!(message.contains(expected), "{message}");
    }
}

#[test]
fn a_generator_failing_part_way_undoes_what_it_wrote() {
    let directory = billing_app();
    let library = read(&directory, "modules/billing/src/lib.rs");
    fs::create_dir_all(directory.path("modules/billing/migrations/0002_create_invoice.sql"))
        .unwrap();

    let refused = usage(&directory, &aggregate("Invoice"));

    assert!(refused.contains("exists already"), "{refused}");
    assert_eq!(read(&directory, "modules/billing/src/lib.rs"), library);
    assert!(!directory.path("modules/billing/src/invoice").exists());
}

#[test]
fn a_codemod_naming_a_framework_crate_adds_the_dependency_on_it() {
    let app = billing_app();
    app.write(
        "modules/billing/src/lib.rs",
        "pub mod module;\n\npub mod areas {\n    pub mod web {\n        pub mod routes;\n    }\n}\n\npub fn hash(password: &sofie::auth::Password) -> String {\n    \
         password.hash()\n}\n",
    );

    let shown = upgrade(app.root(), false).unwrap();
    let manifests: Vec<_> = shown
        .changes
        .iter()
        .filter(|change| change.file.ends_with("Cargo.toml"))
        .map(|change| (change.file.clone(), change.after.clone()))
        .collect();
    upgrade(app.root(), true).unwrap();

    assert!(!shown.sources_pending);

    assert_eq!(
        manifests,
        [
            (
                app.path("modules/billing/Cargo.toml"),
                "sofie-identity.workspace = true".to_owned()
            ),
            (
                app.path("Cargo.toml"),
                format!(
                    "sofie-identity = {{ path = \"{}\" }}",
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .parent()
                        .unwrap()
                        .join("sofie-identity")
                        .display()
                )
            ),
            (
                app.path("app/Cargo.toml"),
                "sofie-identity.workspace = true".to_owned()
            ),
        ]
    );
    let module = fs::read_to_string(app.path("modules/billing/Cargo.toml")).unwrap();
    assert!(
        module.contains("[dependencies]\nsofie-identity.workspace = true\n"),
        "{module}"
    );
    let workspace = fs::read_to_string(app.path("Cargo.toml")).unwrap();
    assert!(
        workspace.contains("\nsofie-identity = { path = "),
        "{workspace}"
    );
    assert_eq!(upgrade(app.root(), false).unwrap().changes, []);
}

#[test]
fn an_upgrade_drops_the_concurrent_indexes_feature_of_a_sofie_dependency() {
    let app = billing_app();
    app.write(
        "sofie/Cargo.toml",
        &format!("{}\n[features]\nsearch = []\n", manifest("sofie", "")),
    );
    app.write(
        "modules/billing/Cargo.toml",
        &manifest(
            "billing",
            "sofie = { path = \"../../sofie\", features = [\"search\", \"concurrent-indexes\"] }\n",
        ),
    );

    let shown = upgrade(app.root(), false).unwrap();
    upgrade(app.root(), true).unwrap();

    assert_eq!(shown.changes.len(), 1);
    assert!(shown.sources_pending);
    assert_eq!(
        shown.changes[0].after,
        "sofie = { path = \"../../sofie\", features = [\"search\"] }"
    );
    assert!(
        read(&app, "modules/billing/Cargo.toml")
            .ends_with("sofie = { path = \"../../sofie\", features = [\"search\"] }\n")
    );
    assert_eq!(upgrade(app.root(), false).unwrap().changes, []);
}

#[test]
fn an_upgrade_whose_gen_fails_undoes_its_rewrites() {
    let app = billing_app();
    let library = "pub mod module;\n\npub mod areas {\n    pub mod web {\n        pub mod routes;\n    }\n}\n\npub fn hash(password: &sofie::auth::Password) -> String {\n    \
                   password.hash()\n}\n";
    app.write("modules/billing/src/lib.rs", library);
    let manifest = read(&app, "modules/billing/Cargo.toml");
    fs::create_dir_all(app.path("app/src/wiring.rs")).unwrap();

    assert!(upgrade(app.root(), true).is_err());

    assert_eq!(read(&app, "modules/billing/src/lib.rs"), library);
    assert_eq!(read(&app, "modules/billing/Cargo.toml"), manifest);
}
