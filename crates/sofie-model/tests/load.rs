use std::path::{Path, PathBuf};

use sofie_model::testing::{TemporaryDirectory, manifest};
use sofie_model::{
    AreaClient, AreaDefinition, ConfigKey, KeyKind, Manifest, Method, ModelError, Modifier, Module,
    Route, Then, Tier, load,
};

struct Fixture {
    directory: TemporaryDirectory,
}

impl Fixture {
    fn new() -> Self {
        let fixture = Self {
            directory: TemporaryDirectory::new("sofie-model"),
        };
        fixture.write(
            "Cargo.toml",
            "[workspace]\nresolver = \"3\"\nmembers = [\"app\", \"modules/*\"]\n",
        );
        fixture
    }

    fn order(lib: &str) -> Self {
        Self::new().with_app(&["order"]).with_module("order", lib)
    }

    fn with_app(self, modules: &[&str]) -> Self {
        let dependencies: Vec<String> = modules
            .iter()
            .map(|module| format!("{module} = {{ path = \"../modules/{module}\" }}\n"))
            .collect();
        self.write("app/Cargo.toml", &manifest("shop", &dependencies.concat()));
        self.write("app/src/lib.rs", "");
        self
    }

    fn with_module(self, name: &str, lib: &str) -> Self {
        self.write(&format!("modules/{name}/Cargo.toml"), &manifest(name, ""));
        self.write(&format!("modules/{name}/src/lib.rs"), lib);
        self
    }

    fn write(&self, relative: &str, text: &str) {
        self.directory.write(relative, text);
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.directory.path(relative)
    }

    fn load(&self) -> Result<sofie_model::App, ModelError> {
        load(self.directory.root())
    }

    fn module(&self, name: &str) -> Module {
        let app = self.load().unwrap();
        app.modules
            .into_iter()
            .find(|module| module.krate.package == name)
            .unwrap()
    }

    fn source_files(&self, name: &str) -> Vec<PathBuf> {
        self.module(name)
            .krate
            .sources
            .into_iter()
            .map(|source| source.file)
            .collect()
    }
}

const DECLARED_AREAS: &str = "pub mod areas { pub mod web { pub mod routes; } }\n";

fn routes_file(body: &str) -> String {
    format!("use sofie::prelude::*;\n\npub fn routes(routes: &mut Routes<Web>) {{\n{body}}}\n")
}

fn route_error(body: &str) -> ModelError {
    let fixture = Fixture::order(DECLARED_AREAS);
    fixture.write("modules/order/src/areas/web/routes.rs", &routes_file(body));
    fixture.load().unwrap_err()
}

fn with_contract(fixture: &Fixture, area_rs: &str) {
    fixture.write(
        "Cargo.toml",
        "[workspace]\nresolver = \"3\"\nmembers = [\"app\", \"modules/*\", \"modules/*/contract\"]\n",
    );
    fixture.write(
        "modules/order/contract/Cargo.toml",
        &manifest("order-contract", ""),
    );
    fixture.write("modules/order/contract/src/lib.rs", "pub mod area;\n");
    fixture.write("modules/order/contract/src/area.rs", area_rs);
}

#[test]
fn reads_a_contract_crate_and_the_areas_it_defines() {
    let fixture = Fixture::order("");
    with_contract(
        &fixture,
        "pub struct Member;\n\nimpl Clone for Member {\n    fn clone(&self) -> Self { Self }\n}\n\n\
         impl Area for Member {\n    const NAME: &'static str = \"member\";\n    \
         const PREFIX: &'static str = \"/account\";\n    type Principal = sofie_identity::Account;\n}\n\n\
         impl sofie_identity::MemberArea for Member {}\n",
    );
    fixture.write(
        "modules/order/contract/src/lib.rs",
        "pub mod area;\nmod r#gen;\n",
    );

    let module = fixture.module("order");

    assert_eq!(module.contract.unwrap().package, "order-contract");
    assert!(module.contract_declares_gen);
    assert_eq!(
        module.defined_areas,
        [AreaDefinition {
            name: "member".to_owned(),
            type_path: vec![
                "order_contract".to_owned(),
                "area".to_owned(),
                "Member".to_owned()
            ],
            prefix: Some("/account".to_owned()),
            client: AreaClient::Html,
            principal: vec!["sofie_identity".to_owned(), "Account".to_owned()],
            implements_member_area: true,
            opens_sign_up: false,
            file: fixture.path("modules/order/contract/src/area.rs"),
            line: 7,
        }]
    );
    assert_eq!(fixture.module("order").krate.package, "order");
}

#[test]
fn an_area_definition_needs_a_literal_name_beside_its_type() {
    for area_rs in [
        "impl Area for Member {\n    const NAME: &'static str = NAME;\n}\n",
        "impl Area for crate::Member {\n    const NAME: &'static str = \"member\";\n}\n",
    ] {
        let fixture = Fixture::order("");
        with_contract(&fixture, area_rs);

        let error = fixture.load().unwrap_err();

        assert_eq!(error.line, 1, "{error}");
        assert!(error.file.ends_with("contract/src/area.rs"), "{error}");
    }
}

#[test]
fn reads_packages_dependencies_and_migrations() {
    let fixture = Fixture::new()
        .with_module("order", "")
        .with_module("stock-level", "");
    fixture.write(
        "app/Cargo.toml",
        &manifest(
            "shop",
            "order = { path = \"../modules/order\" }\n\
             stock = { path = \"../modules/stock-level\", package = \"stock-level\" }\n",
        ),
    );
    fixture.write("app/src/lib.rs", "");
    fixture.write("modules/order/migrations/0002_index.sql", "");
    fixture.write("modules/order/migrations/0001_create.sql", "");
    fixture.write("modules/order/migrations/notes.txt", "");

    let app = fixture.load().unwrap();

    assert_eq!(app.app.package, "shop");
    let dependencies: Vec<(&str, &str)> = app
        .app
        .dependencies
        .iter()
        .map(|dependency| (dependency.package.as_str(), dependency.crate_name.as_str()))
        .collect();
    assert_eq!(dependencies, [("order", "order"), ("stock-level", "stock")]);
    let [order, stock] = app.modules.as_slice() else {
        panic!("expected two modules: {:?}", app.modules);
    };
    assert_eq!(
        order.migrations,
        [
            fixture.path("modules/order/migrations/0001_create.sql"),
            fixture.path("modules/order/migrations/0002_index.sql"),
        ]
    );
    assert_eq!(stock.krate.crate_name, "stock_level");
    assert_eq!(stock.migrations, Vec::<PathBuf>::new());
}

#[test]
fn reads_tests_examples_and_benches_beside_the_sources() {
    let fixture = Fixture::order("");
    for file in [
        "tests/common/mod.rs",
        "tests/suite/part.rs",
        "examples/a.rs",
        "benches/c.rs",
    ] {
        fixture.write(&format!("modules/order/{file}"), "pub fn helper() {}\n");
    }
    fixture.write(
        "modules/order/tests/b.rs",
        "mod common;\npub fn helper() {}\n",
    );
    fixture.write("modules/order/tests/suite/main.rs", "mod part;\n");
    fixture.write("modules/order/tests/fixtures/broken.rs", "fn broken( {\n");
    fixture.write("modules/order/tests/notes.txt", "");

    let order = fixture.module("order");

    let files: Vec<PathBuf> = order
        .krate
        .tests_and_examples
        .iter()
        .map(|source| source.file.clone())
        .collect();
    let expected = [
        "tests/b.rs",
        "tests/common/mod.rs",
        "tests/suite/main.rs",
        "tests/suite/part.rs",
        "examples/a.rs",
        "benches/c.rs",
    ]
    .map(|file| fixture.path(&format!("modules/order/{file}")));
    assert_eq!(files, expected);
    assert_eq!(
        order.krate.tests_and_examples[0].mentions.count("helper"),
        1
    );
}

#[test]
fn follows_nested_and_inline_modules() {
    let fixture = Fixture::order("mod a;\nmod b {\n    mod c;\n}\n");
    fixture.write("modules/order/src/a.rs", "mod d;\n");
    fixture.write("modules/order/src/a/d.rs", "");
    fixture.write("modules/order/src/b/c/mod.rs", "mod e;\n");
    fixture.write("modules/order/src/b/c/e.rs", "");
    fixture.write("modules/order/src/unreachable.rs", "");

    let sources = fixture.source_files("order");

    let expected: Vec<PathBuf> = ["a/d.rs", "a.rs", "b/c/e.rs", "b/c/mod.rs", "lib.rs"]
        .iter()
        .map(|file| fixture.path(&format!("modules/order/src/{file}")))
        .collect();
    assert_eq!(sources, expected);
}

#[test]
fn follows_path_attributes() {
    let lib = "#[path = \"other/x.rs\"]\nmod x;\nmod i {\n    #[path = \"z.rs\"]\n    mod z;\n}\n";
    let fixture = Fixture::new().with_app(&[]).with_module("order", lib);
    fixture.write("modules/order/src/other/x.rs", "mod y;\n");
    fixture.write("modules/order/src/other/y.rs", "");
    fixture.write("modules/order/src/i/z.rs", "");

    let sources = fixture.source_files("order");

    let expected: Vec<PathBuf> = ["i/z.rs", "lib.rs", "other/x.rs", "other/y.rs"]
        .iter()
        .map(|file| fixture.path(&format!("modules/order/src/{file}")))
        .collect();
    assert_eq!(sources, expected);
}

#[test]
fn a_module_without_a_file_is_left_out() {
    let fixture = Fixture::new().with_app(&[]).with_module(
        "order",
        "mod generated;\n#[path = \"gone.rs\"]\nmod gone;\n",
    );

    let sources = fixture.source_files("order");

    assert_eq!(sources, [fixture.path("modules/order/src/lib.rs")]);
}

#[test]
fn reads_the_routes_of_a_declared_area() {
    let fixture = Fixture::order(DECLARED_AREAS);
    let body = "    routes.get(\"/orders\", queries::list_orders);\n    routes.post(\"/orders\", place_order);\n";
    fixture.write("modules/order/src/areas/web/routes.rs", &routes_file(body));

    let module = fixture.module("order");

    let [web] = module.areas.as_slice() else {
        panic!("expected one area: {:?}", module.areas);
    };
    assert_eq!(web.area, "web");
    assert!(web.declared);
    assert_eq!(
        web.routes,
        [
            Route {
                method: Method::Get,
                path: "/orders".to_owned(),
                handler: vec!["queries".to_owned(), "list_orders".to_owned()],
                screen: None,
                island: None,
                then: None,
                modifiers: Vec::new(),
                webhook: None,
                line: 4,
            },
            Route {
                method: Method::Post,
                path: "/orders".to_owned(),
                handler: vec!["place_order".to_owned()],
                screen: None,
                island: None,
                then: None,
                modifiers: Vec::new(),
                webhook: None,
                line: 5,
            },
        ]
    );
}

#[test]
fn an_undeclared_routes_file_is_still_read() {
    let fixture = Fixture::order("");
    let routes_path = "modules/order/src/areas/web/routes.rs";
    fixture.write(routes_path, &routes_file(""));

    let module = fixture.module("order");

    assert_eq!(module.areas[0].routes_file, fixture.path(routes_path));
    assert!(!module.areas[0].declared);
}

#[test]
fn a_non_literal_path_is_an_error_at_its_line() {
    let error = route_error("    let path = \"/orders\";\n");
    assert_eq!(error.line, 4);
    assert!(error.message.contains("may only hold"), "{error}");

    let error = route_error("\n    routes.get(PATH, list_orders);\n");
    assert!(
        error.file.ends_with(Path::new("areas/web/routes.rs")),
        "{error}"
    );
    assert_eq!(error.line, 5);
    assert!(error.message.contains("string literal"), "{error}");
}

#[test]
fn unknown_methods_and_modifiers_are_errors() {
    let error = route_error("    routes.head(\"/orders\", list_orders);\n");
    assert!(
        error.message.contains("unknown route method `head`"),
        "{error}"
    );

    let error = route_error("    routes.post(\"/orders\", place_order).cache(10);\n");
    assert!(error.message.contains("`.cache(…)`"), "{error}");
}

#[test]
fn reads_screen_routes_and_the_area_templates() {
    let fixture = Fixture::order(DECLARED_AREAS);
    let body = "    routes.screen(\"/orders\", \"orders\", queries::list_orders);\n";
    fixture.write("modules/order/src/areas/web/routes.rs", &routes_file(body));
    fixture.write("modules/order/src/areas/web/screens/orders.html", "");
    fixture.write("modules/order/src/areas/web/screens/notes.md", "");

    let module = fixture.module("order");

    let web = &module.areas[0];
    assert_eq!(
        web.routes,
        [Route {
            method: Method::Get,
            path: "/orders".to_owned(),
            handler: vec!["queries".to_owned(), "list_orders".to_owned()],
            screen: Some("orders".to_owned()),
            island: None,
            then: None,
            modifiers: Vec::new(),
            webhook: None,
            line: 4,
        }]
    );
    assert_eq!(
        web.screens,
        [fixture.path("modules/order/src/areas/web/screens/orders.html")]
    );
}

#[test]
fn a_screen_name_cannot_leave_the_screens_folder() {
    let error = route_error("    routes.screen(\"/orders\", \"../secret\", list_orders);\n");
    assert!(error.message.contains("screen name"), "{error}");

    let error = route_error("    routes.screen(\"/orders\", list_orders);\n");
    assert!(
        error
            .message
            .contains("`.screen(\"/path\", \"name\", loader)`"),
        "{error}"
    );
}

#[test]
fn reads_an_action_that_refreshes_a_screen() {
    let fixture = Fixture::order(DECLARED_AREAS);
    let body = "    routes\n        .post(\"/orders/{order}/cancel\", actions::cancel_order)\n        .then_screen(\"orders\", \"orders-table\");\n";
    fixture.write("modules/order/src/areas/web/routes.rs", &routes_file(body));

    let module = fixture.module("order");

    assert_eq!(
        module.areas[0].routes,
        [Route {
            method: Method::Post,
            path: "/orders/{order}/cancel".to_owned(),
            handler: vec!["actions".to_owned(), "cancel_order".to_owned()],
            screen: None,
            island: None,
            then: Some(Then::Screen {
                screen: "orders".to_owned(),
                fragment: "orders-table".to_owned(),
            }),
            modifiers: Vec::new(),
            webhook: None,
            line: 4,
        }]
    );
}

#[test]
fn reads_an_action_that_redirects_and_refuses_a_path_leaving_the_app() {
    let fixture = Fixture::order(DECLARED_AREAS);
    let body = "    routes.post(\"/sign-in\", actions::sign_in).then_redirect(\"/account\");\n";
    fixture.write("modules/order/src/areas/web/routes.rs", &routes_file(body));

    let module = fixture.module("order");

    assert_eq!(
        module.areas[0].routes[0].then,
        Some(Then::Redirect("/account".to_owned()))
    );
    for path in ["\"//evil.test\"", "\"account\"", "PATH", "\"/a\", \"/b\""] {
        let error = route_error(&format!(
            "    routes.post(\"/sign-in\", sign_in).then_redirect({path});\n"
        ));
        assert!(
            error.message.contains("write `.then_redirect("),
            "{path}: {error}"
        );
    }
}

#[test]
fn then_screen_needs_an_action_a_screen_name_and_an_element_id() {
    let error =
        route_error("    routes.get(\"/orders\", list).then_screen(\"orders\", \"orders\");\n");
    assert!(error.message.contains("follows an action"), "{error}");

    for arguments in [
        "\"orders\"",
        "\"../x\", \"orders\"",
        "\"orders\", \"#orders\"",
        "NAME, \"orders\"",
    ] {
        let error = route_error(&format!(
            "    routes.post(\"/orders\", place).then_screen({arguments});\n"
        ));
        assert!(
            error.message.contains("write `.then_screen("),
            "{arguments}: {error}"
        );
    }

    let error =
        route_error("    routes.post(\"/orders\", place).then_screen(\"a\", \"b\").throttle(1);\n");
    assert!(error.message.contains("`.then_screen(…)`"), "{error}");
}

#[test]
fn a_listener_s_event_is_named_through_its_imports() {
    let fixture = Fixture::order(
        "pub mod listeners;\npub mod events {\n    #[derive(Debug, Event)]\n    pub struct OrderCancelled {}\n}\n",
    );
    fixture.write(
        "modules/order/src/listeners.rs",
        "use crate::events::OrderCancelled;\nuse order_contract::OrderPlaced;\n\n\
         #[on]\npub async fn placed(event: OrderPlaced) {}\n\n\
         #[on]\npub async fn cancelled(event: OrderCancelled) {}\n",
    );

    let order = fixture.module("order");

    let events: Vec<Vec<String>> = order
        .listeners
        .iter()
        .map(|listener| listener.event.clone())
        .collect();
    assert_eq!(
        events,
        [
            vec!["order_contract".to_owned(), "OrderPlaced".to_owned()],
            vec![
                "crate".to_owned(),
                "events".to_owned(),
                "OrderCancelled".to_owned()
            ],
        ]
    );
}

/// The wiring sits in another module, so naming the event through the listener's private glob import would not compile.
#[test]
fn a_listener_s_event_brought_in_by_a_glob_is_named_where_the_glob_points() {
    let fixture = Fixture::order(
        "pub mod listeners;\npub mod events {\n    #[derive(Debug, Event)]\n    pub struct OrderCancelled {}\n}\n",
    );
    fixture.write(
        "modules/order/src/listeners.rs",
        "use sofie::prelude::*;\nuse order_contract::*;\n\n\
         #[on]\npub async fn placed(event: OrderPlaced) {}\n",
    );

    let order = fixture.module("order");

    assert_eq!(
        order.listeners[0].event,
        ["order_contract".to_owned(), "OrderPlaced".to_owned()]
    );
}

#[test]
fn a_listener_s_event_behind_two_globs_is_an_error_asking_for_an_import_by_name() {
    let fixture = Fixture::order(
        "pub mod listeners;\npub mod events {\n    #[derive(Debug, Event)]\n    pub struct OrderCancelled {}\n}\n",
    );
    fixture.write(
        "modules/order/src/listeners.rs",
        "use crate::events::*;\nuse order_contract::*;\n\n\
         #[on]\npub async fn placed(event: OrderPlaced) {}\n",
    );

    let error = fixture.load().unwrap_err();

    assert_eq!(error.line, 4, "{error}");
    assert!(
        error
            .message
            .contains("import it by name, as `use …::OrderPlaced;`"),
        "{error}"
    );
}

#[test]
fn a_schedule_s_query_is_named_through_its_file_s_imports() {
    let fixture = Fixture::order("pub mod jobs;\npub mod queries;\n");
    fixture.write(
        "modules/order/src/queries.rs",
        "pub async fn stale() {}\npub async fn late() {}\n",
    );
    fixture.write(
        "modules/order/src/jobs.rs",
        "use crate::queries::stale;\nuse crate::queries;\n\n\
         #[every(\"1h\", each = stale)]\npub async fn expire() {}\n\n\
         #[every(\"1h\", each = queries::late)]\npub async fn remind() {}\n",
    );

    let order = fixture.module("order");

    let each: Vec<Option<Vec<String>>> = order
        .schedules
        .iter()
        .map(|schedule| schedule.each.clone())
        .collect();
    let path = |segments: &[&str]| Some(segments.iter().map(|s| (*s).to_owned()).collect());
    assert_eq!(
        each,
        [path(&["queries", "stale"]), path(&["queries", "late"])]
    );
}

/// The app's wiring names an exposed aggregate from outside the crate, through the crate root.
#[test]
fn an_aggregate_re_exported_by_a_glob_is_named_from_the_crate_root() {
    let fixture = Fixture::order("mod order;\npub use order::*;\n");
    fixture.write(
        "modules/order/src/order.rs",
        "#[derive(Aggregate)]\npub struct Order {\n    meta: Meta,\n}\n",
    );

    let order = fixture.module("order");

    assert_eq!(order.code.aggregates[0].path, ["Order"]);
}

#[test]
fn reads_what_scaffolded_tests_check_of_records_and_status_enums() {
    let fixture = Fixture::order(
        "mod product;\nmod seller;\npub use product::{Product, ProductStatus as Status};\n",
    );
    fixture.write(
        "modules/order/src/product.rs",
        "#[derive(Enum)]\npub enum ProductStatus {\n    #[to(Listed, OnHold)]\n    Draft,\n    \
         OnHold,\n    Listed,\n}\n\
         #[derive(Aggregate)]\npub struct Product {\n    meta: Meta,\n    #[owner]\n    \
         seller: Id<Seller>,\n    #[required]\n    #[max(120)]\n    name: String,\n    \
         #[length(min = 2, max = 8)]\n    pub(crate) code: Option<String>,\n    \
         #[range(min = -5, max = 5)]\n    tilt: i64,\n}\n",
    );
    fixture.write(
        "modules/order/src/seller.rs",
        "#[derive(Aggregate)]\n#[owner]\npub struct Seller {\n    meta: Meta,\n}\n",
    );

    let order = fixture.module("order");

    let status = &order.code.enums[0];
    assert_eq!(status.path, ["Status"]);
    assert_eq!(status.variants[0].to, ["Listed", "OnHold"]);
    let product = &order.code.aggregates[0];
    let bounds: Vec<String> = product
        .fields
        .iter()
        .map(|field| format!("{}: {}", field.name, field.bounds.join(", ")))
        .collect();
    assert_eq!(
        bounds,
        [
            "seller: ",
            "name: required, max 120",
            "code: min 2, max 8",
            "tilt: min -5, max 5"
        ]
    );
    assert!(product.is_owned());
    assert!(order.code.aggregates[1].is_owned());
    assert!(order.code.aggregates[1].owner.is_none());
}

#[test]
fn reads_listeners_schedules_and_events_with_their_module_paths() {
    let fixture = Fixture::order(
        "pub mod jobs;\nmod events {\n    #[derive(Debug, Event)]\n    pub struct OrderCancelled {}\n}\n",
    );
    fixture.write(
        "modules/order/src/jobs.rs",
        "#[on]\npub async fn audit(event: crate::events::OrderCancelled) {}\n\n\
         #[sofie::every(\"15m\")]\npub async fn expire() {}\n\n#[every(15)]\npub async fn broken() {}\n\n\
         #[every(weekly_on = \"monday\", daily_at = \"03:00\", zone = \"Europe/Istanbul\",)]\n\
         pub async fn digest() {}\n",
    );

    let order = fixture.module("order");

    let jobs = fixture.path("modules/order/src/jobs.rs");
    assert_eq!(
        order.listeners,
        [sofie_model::Listener {
            file: jobs.clone(),
            line: 1,
            function: vec!["jobs".to_owned(), "audit".to_owned()],
            event: vec![
                "crate".to_owned(),
                "events".to_owned(),
                "OrderCancelled".to_owned()
            ],
        }]
    );
    let schedules: Vec<(Vec<String>, String, usize)> = order
        .schedules
        .iter()
        .map(|schedule| {
            (
                schedule.function.clone(),
                schedule.cadence.to_string(),
                schedule.line,
            )
        })
        .collect();
    let at = |function: &str, cadence: &str, line| {
        (
            vec!["jobs".to_owned(), function.to_owned()],
            cadence.to_owned(),
            line,
        )
    };
    assert_eq!(
        schedules,
        [
            at("expire", "every 15m", 4),
            at("broken", "every 15", 7),
            at("digest", "weekly on monday at 03:00 Europe/Istanbul", 10),
        ]
    );
    assert_eq!(order.events, ["OrderCancelled"]);
}

#[test]
fn reads_tasks_under_the_tasks_module_and_refuses_them_elsewhere() {
    let fixture = Fixture::order("pub mod tasks;\n");
    fixture.write(
        "modules/order/src/tasks/mod.rs",
        "pub mod totals;\n\n#[task]\npub async fn reindex() {}\n",
    );
    fixture.write(
        "modules/order/src/tasks/totals.rs",
        "#[sofie::task]\npub async fn backfill() {}\n",
    );

    let order = fixture.module("order");

    let mut tasks: Vec<(String, usize)> = order
        .tasks
        .iter()
        .map(|task| {
            (
                sofie_model::naming::task_name(&order, &task.function),
                task.line,
            )
        })
        .collect();
    tasks.sort();
    assert_eq!(
        tasks,
        [
            ("order.backfill".to_owned(), 1),
            ("order.reindex".to_owned(), 3)
        ]
    );

    for (lib, message) in [
        ("\n#[task]\npub async fn reindex() {}\n", "a task lives in"),
        (
            "pub mod tasks {\n    #[task]\n    pub async fn reindex() {}\n    pub mod again {\n        #[task]\n        pub async fn reindex() {}\n    }\n}\n",
            "another task of this module has this function's name",
        ),
    ] {
        let fixture = Fixture::order(lib);

        let error = fixture.load().unwrap_err();

        assert!(error.message.starts_with(message), "{}", error.message);
    }
}

#[test]
fn a_listener_attribute_with_arguments_or_without_an_event_is_an_error_at_its_line() {
    for (source, message) in [
        (
            "\n#[on(OrderCancelled)]\nasync fn audit(event: OrderCancelled) {}\n",
            "write a bare `#[on]`",
        ),
        (
            "\n#[on]\nasync fn audit() {}\n",
            "a listener takes the event it hears as its first argument",
        ),
    ] {
        let fixture = Fixture::order(source);

        let error = fixture.load().unwrap_err();

        assert_eq!(error.line, 2, "{source}");
        assert!(error.message.starts_with(message), "{}", error.message);
    }
}

#[test]
fn test_only_items_are_not_wired_and_other_cfg_is_an_error() {
    let fixture = Fixture::new().with_app(&["order"]).with_module(
        "order",
        "pub mod jobs;\n#[cfg(test)]\nmod tests;\n\
         #[cfg(test)]\nmod test_only {\n    #[on]\n    pub async fn audit(event: OrderPlaced) {}\n}\n\
         #[cfg(test)]\n#[every(\"1h\")]\npub async fn tidy() {}\n",
    );
    fixture.write(
        "modules/order/src/tests.rs",
        "#[job]\npub async fn export() {}\n",
    );
    fixture.write(
        "modules/order/src/jobs.rs",
        "#[job]\npub async fn export() {}\n",
    );

    let order = fixture.module("order");

    assert_eq!(order.listeners, []);
    assert_eq!(order.schedules, []);
    assert_eq!(order.jobs.len(), 1);

    for lib in [
        "\n#[cfg(feature = \"audit\")]\n#[on]\npub async fn audit(event: OrderPlaced) {}\n",
        "\n#[cfg(unix)]\npub mod listeners {\n    #[on]\n    pub async fn audit(event: OrderPlaced) {}\n}\n",
    ] {
        let fixture = Fixture::order(lib);

        let error = fixture.load().unwrap_err();

        assert!(
            error
                .message
                .contains("modules/order/src/lib.rs:2, but the wiring"),
            "{}",
            error.message
        );
    }
}

#[test]
fn keyword_modules_keep_r_hash_in_paths_but_not_in_names() {
    let fixture = Fixture::new().with_app(&["order"]).with_module(
        "order",
        "#[path = \"r_type/mod.rs\"]\npub mod r#type;\npub mod r#impl;\n",
    );
    fixture.write(
        "modules/order/src/r_type/mod.rs",
        "#[on]\npub async fn r#match(event: OrderPlaced) {}\n",
    );
    fixture.write(
        "modules/order/src/impl.rs",
        "#[job]\npub async fn r#move() {}\n",
    );

    let order = fixture.module("order");

    assert_eq!(order.listeners[0].function, ["r#type", "r#match"]);
    assert_eq!(order.jobs[0].function, ["r#impl", "r#move"]);
    assert_eq!(
        sofie_model::naming::job_name(&order, &order.listeners[0].function),
        "order.type.match"
    );
}

#[test]
fn an_area_folder_that_is_no_identifier_is_an_error() {
    let fixture = Fixture::order("");
    fixture.write(
        "modules/order/src/areas/back-office/routes.rs",
        &routes_file(""),
    );

    let error = fixture.load().unwrap_err();

    assert!(
        error
            .message
            .contains("the area folder `back-office` is not a Rust identifier"),
        "{}",
        error.message
    );
}

#[test]
fn an_area_module_mapped_to_another_folder_is_named_after_the_module() {
    let fixture = Fixture::new().with_app(&["order"]).with_module(
        "order",
        "pub mod areas {\n    #[path = \"back-office\"]\n    pub mod back_office {\n        pub mod routes;\n    }\n}\n",
    );
    fixture.write(
        "modules/order/src/areas/back-office/routes.rs",
        &routes_file(""),
    );
    fixture.write(
        "modules/order/src/areas/back-office/islands/Counter.svelte",
        "<p>count</p>\n",
    );

    let order = fixture.module("order");

    let [area] = order.areas.as_slice() else {
        panic!("expected one area: {:?}", order.areas);
    };
    assert_eq!(area.area, "back_office");
    assert!(area.declared);
    assert_eq!(order.islands[0].area.as_deref(), Some("back_office"));
}

#[test]
fn the_crate_name_is_the_lib_target_s() {
    let fixture = Fixture::order("");
    fixture.write(
        "modules/order/Cargo.toml",
        &format!("{}\n[lib]\nname = \"orders\"\n", manifest("order", "")),
    );

    let order = fixture.module("order");

    assert_eq!(order.krate.crate_name, "orders");
}

#[test]
fn a_dependency_is_named_by_its_lib_target() {
    let fixture = Fixture::order("");
    fixture.write(
        "modules/order/Cargo.toml",
        &format!("{}\n[lib]\nname = \"orders\"\n", manifest("order", "")),
    );

    let app = fixture.load().unwrap();

    let order = app
        .app
        .dependencies
        .iter()
        .find(|dependency| dependency.package == "order")
        .unwrap();
    assert_eq!(order.crate_name, "orders");
}

#[test]
fn records_private_modules_and_private_wired_functions() {
    let fixture = Fixture::new().with_app(&["order"]).with_module(
        "order",
        "pub(crate) mod listeners {\n    #[on]\n    pub async fn audit(event: OrderPlaced) {}\n}\n\
         pub mod jobs {\n    #[job]\n    async fn export() {}\n    pub fn helper() {}\n}\n",
    );

    let order = fixture.module("order");

    let private: Vec<(String, usize)> = order
        .private_items
        .iter()
        .map(|item| (item.path.join("::"), item.line))
        .collect();
    assert_eq!(
        private,
        [("listeners".to_owned(), 1), ("jobs::export".to_owned(), 7)]
    );
}

fn handlers_of(routes_rs: &str) -> Result<Vec<Vec<String>>, ModelError> {
    let fixture = Fixture::order(DECLARED_AREAS);
    fixture.write("modules/order/src/areas/web/routes.rs", routes_rs);
    let app = fixture.load()?;
    Ok(app.modules[0].areas[0]
        .routes
        .iter()
        .map(|route| route.handler.clone())
        .collect())
}

#[test]
fn handler_paths_are_read_from_the_crate_root() {
    let routes_rs = "use sofie::prelude::*;\nuse crate::actions::place_order;\n\
                     use crate::{queries as reads, actions::{self as writes}};\n\
                     use super::super::super::actions;\n\n\
                     pub fn routes(routes: &mut Routes<Web>) {\n\
                     routes.get(\"/a\", crate::queries::list_orders);\n\
                     routes.post(\"/b\", place_order);\n\
                     routes.get(\"/c\", reads::search);\n\
                     routes.post(\"/d\", super::actions::cancel);\n\
                     routes.get(\"/e\", self::local);\n\
                     routes.post(\"/f\", actions::ship);\n\
                     routes.post(\"/g\", writes::pay);\n\
                     routes.get(\"/h\", queries::unresolved);\n\
                     routes.get(\"/i\", local);\n}\n\n\
                     pub async fn local() {}\n";

    let handlers = handlers_of(routes_rs).unwrap();

    let expected: Vec<Vec<String>> = [
        "queries::list_orders",
        "actions::place_order",
        "queries::search",
        "areas::web::actions::cancel",
        "areas::web::routes::local",
        "actions::ship",
        "actions::pay",
        "queries::unresolved",
        "areas::web::routes::local",
    ]
    .iter()
    .map(|path| path.split("::").map(str::to_owned).collect())
    .collect();
    assert_eq!(handlers, expected);
}

#[test]
fn a_handler_is_resolved_through_the_crate_s_one_glob_import() {
    let routes_rs = "use sofie::prelude::*;\nuse crate::actions;\nuse crate::queries::*;\n\n\
                     pub fn routes(routes: &mut Routes<Web>) {\n\
                     routes.get(\"/orders\", list_orders);\n\
                     routes.post(\"/orders\", actions::place_order);\n}\n";

    let handlers = handlers_of(routes_rs).unwrap();

    assert_eq!(
        handlers,
        [["queries", "list_orders"], ["actions", "place_order"]]
    );
}

#[test]
fn a_handler_path_above_the_crate_root_is_an_error_at_its_line() {
    let routes_rs = "use sofie::prelude::*;\n\npub fn routes(routes: &mut Routes<Web>) {\n\
                     routes.post(\"/a\", super::super::super::super::actions::x);\n}\n";

    let error = handlers_of(routes_rs).unwrap_err();

    assert_eq!(error.line, 4);
    assert!(error.message.contains("above the crate root"), "{error}");
}

#[test]
fn reads_the_paths_and_macro_calls_each_source_names() {
    let fixture = Fixture::new().with_app(&["order"]).with_module(
        "order",
        "use std::{env, net::TcpStream};\n\nfn a() -> String {\n    env::var(\"KEY\").unwrap_or(env!(\"HOME\").to_owned())\n}\n",
    );

    let module = fixture.module("order");

    let [source] = module.krate.sources.as_slice() else {
        panic!("expected one source: {:?}", module.krate.sources);
    };
    let paths: Vec<(String, usize)> = source
        .paths
        .iter()
        .map(|path| (path.segments.join("::"), path.line))
        .collect();
    assert_eq!(
        paths,
        [
            ("std::env".to_owned(), 1),
            ("std::net::TcpStream".to_owned(), 1),
            ("env::var".to_owned(), 4),
        ]
    );
    let macros: Vec<(&str, Option<&str>, usize)> = source
        .macros
        .iter()
        .map(|call| (call.name(), call.first_string.as_deref(), call.line))
        .collect();
    assert_eq!(macros, [("env", Some("HOME"), 4)]);
}

#[test]
fn a_module_outside_the_app_workspace_is_an_error() {
    let fixture = Fixture::new().with_app(&[]).with_module("order", "");
    fixture.write(
        "Cargo.toml",
        "[workspace]\nresolver = \"3\"\nmembers = [\"app\"]\nexclude = [\"modules\"]\n",
    );

    let error = fixture.load().unwrap_err();

    assert!(error.file.ends_with("modules/order/Cargo.toml"), "{error}");
    assert!(
        error.message.contains("member of the app's workspace"),
        "{error}"
    );
}

#[test]
fn reads_the_screens_identity_ships_when_the_app_depends_on_it() {
    let fixture = Fixture::new().with_module("order", "");
    fixture.write(
        "Cargo.toml",
        "[workspace]\nresolver = \"3\"\nmembers = [\"app\", \"modules/*\", \"identity\"]\n",
    );
    fixture.write("identity/Cargo.toml", &manifest("sofie-identity", ""));
    fixture.write("identity/src/lib.rs", "");
    fixture.write("identity/src/screens/sign_up.html", "");
    fixture.write("identity/src/screens/sign_in.html", "");
    fixture.write(
        "app/Cargo.toml",
        &manifest("shop", "sofie-identity = { path = \"../identity\" }\n"),
    );
    fixture.write("app/src/lib.rs", "");

    let identity = fixture.load().unwrap().identity.unwrap();

    assert_eq!(
        identity.screens,
        ["sofie_identity/web/sign_in", "sofie_identity/web/sign_up"]
    );
    let without = Fixture::new()
        .with_app(&[])
        .with_module("order", "")
        .load()
        .unwrap();
    assert_eq!(without.identity, None);
}

#[test]
fn reads_the_identity_screens_from_outside_the_app_workspace() {
    let identity = TemporaryDirectory::new("sofie-model-identity");
    identity.write("Cargo.toml", &manifest("sofie-identity", ""));
    identity.write("src/lib.rs", "");
    identity.write("src/screens/sign_in.html", "");
    let fixture = Fixture::new().with_module("order", "");
    fixture.write(
        "app/Cargo.toml",
        &manifest(
            "shop",
            &format!("sofie-identity = {{ path = {:?} }}\n", identity.root()),
        ),
    );
    fixture.write("app/src/lib.rs", "");

    let screens = fixture.load().unwrap().identity.unwrap().screens;

    assert_eq!(screens, ["sofie_identity/web/sign_in"]);
}

#[test]
fn reads_a_module_manifest_and_whether_it_is_declared() {
    let manifest = "use sofie::{Module, Tier};\n\npub const MODULE: Module = Module {\n    \
                    name: \"order\",\n    tier: Tier::Core,\n    context: Some(\"commerce\"),\n    \
                    settings: &[Setting { name: \"free_shipping_above\", default: \
                    SettingValue::Number(5000) }],\n};\n";
    let fixture = Fixture::new()
        .with_app(&["order", "stock"])
        .with_module("order", "pub mod module;\n")
        .with_module("stock", "");
    fixture.write("modules/order/src/module.rs", manifest);
    fixture.write(
        "modules/stock/src/module.rs",
        &manifest
            .replace("\"order\"", "\"stock\"")
            .replace("Some(\"commerce\")", "None"),
    );

    assert_eq!(
        fixture.module("order").manifest,
        Some(Manifest {
            name: "order".to_owned(),
            tier: Tier::Core,
            context: Some("commerce".to_owned()),
            settings: vec!["free_shipping_above".to_owned()],
            file: fixture.path("modules/order/src/module.rs"),
            line: 3,
            declared: true,
        })
    );
    let stock = fixture.module("stock").manifest.unwrap();
    assert_eq!((stock.context, stock.declared), (None, false));
    assert_eq!(fixture.module("stock").krate.sources.len(), 1);
}

#[test]
fn a_config_struct_is_read_as_its_environment_keys() {
    let config = "use sofie::config::Database;\n\n#[derive(sofie::Config)]\npub struct OrderConfig {\n    \
                  /// Printed before every invoice number.\n    invoice_prefix: String,\n    \
                  window_hours: Option<u64>,\n    database: Database,\n    payment: Payment,\n}\n\n\
                  #[derive(sofie::Config)]\npub struct Payment {\n    api_key: Secret<String>,\n    \
                  sandbox: bool,\n}\n";
    let fixture = Fixture::order("pub mod config;\n");
    fixture.write("modules/order/src/config.rs", config);

    let config = fixture.module("order").config.unwrap();

    assert_eq!(
        (config.type_name.as_str(), config.line, config.declared),
        ("OrderConfig", 4, true)
    );
    let key = |name: &str, doc: &[&str], kind, required, fallback: Option<&str>| ConfigKey {
        name: name.to_owned(),
        doc: doc.iter().map(|line| (*line).to_owned()).collect(),
        kind,
        required,
        fallback: fallback.map(str::to_owned),
        secret: fallback.is_some(),
    };
    assert_eq!(
        config.keys,
        [
            key(
                "ORDER_INVOICE_PREFIX",
                &["Printed before every invoice number."],
                KeyKind::Text,
                true,
                None
            ),
            key("ORDER_WINDOW_HOURS", &[], KeyKind::Count, false, None),
            key(
                "ORDER_DATABASE_URL",
                &[],
                KeyKind::Text,
                false,
                Some("DATABASE_URL")
            ),
            ConfigKey {
                secret: true,
                ..key("ORDER_PAYMENT_API_KEY", &[], KeyKind::Text, true, None)
            },
            key("ORDER_PAYMENT_SANDBOX", &[], KeyKind::Boolean, true, None),
        ]
    );
}

#[test]
fn a_module_s_own_queue_struct_has_no_database_fallback() {
    let config = "#[derive(sofie::Config)]\npub struct OrderConfig {\n    queue: Queue,\n}\n\n\
                  #[derive(sofie::Config)]\npub struct Queue {\n    url: String,\n}\n";
    let fixture = Fixture::order("pub mod config;\n");
    fixture.write("modules/order/src/config.rs", config);

    let config = fixture.module("order").config.unwrap();

    assert_eq!(
        config.keys,
        [ConfigKey {
            name: "ORDER_QUEUE_URL".to_owned(),
            doc: Vec::new(),
            kind: KeyKind::Text,
            required: true,
            fallback: None,
            secret: false,
        }]
    );
}

#[test]
fn a_config_file_without_the_module_s_derived_struct_is_an_error() {
    for config in [
        "#[derive(sofie::Config)]\npub struct Settings { key: String }\n",
        "pub struct OrderConfig { key: String }\n",
    ] {
        let fixture = Fixture::order("pub mod config;\n");
        fixture.write("modules/order/src/config.rs", config);

        let error = fixture.load().unwrap_err();

        assert!(
            error
                .message
                .contains("`#[derive(sofie::Config)] pub struct OrderConfig"),
            "{}",
            error.message
        );
    }
}

#[test]
fn a_manifest_it_cannot_read_is_an_error_naming_the_shape() {
    let fixture = Fixture::order("pub mod module;\n");
    fixture.write(
        "modules/order/src/module.rs",
        "pub const MODULE: Module = Module { name: \"order\", tier: Tier::Middle, context: None, settings: &[] };\n",
    );

    let error = fixture.load().unwrap_err();

    assert_eq!(error.line, 1);
    assert!(
        error
            .message
            .starts_with("`tier` must be `Tier::Experience`"),
        "{}",
        error.message
    );
    assert!(
        error.message.contains("pub const MODULE: sofie::Module"),
        "{}",
        error.message
    );
}

#[test]
fn a_manifest_constant_is_not_a_public_item_and_build_rs_uses_the_crate() {
    let fixture = Fixture::order("pub mod module;\npub fn kept() {}\n");
    fixture.write(
        "modules/order/src/module.rs",
        "pub const MODULE: Module = Module { name: \"order\", tier: Tier::Core, context: None, settings: &[] };\n",
    );
    fixture.write("modules/order/build.rs", "fn main() { order::kept(); }\n");

    let krate = fixture.module("order").krate;

    let items: Vec<&str> = krate
        .sources
        .iter()
        .flat_map(|source| &source.public_items)
        .map(|item| item.name.as_str())
        .collect();
    assert_eq!(items, ["kept"]);
    assert_eq!(
        krate.tests_and_examples[0].file,
        fixture.path("modules/order/build.rs")
    );
}

#[test]
fn reads_route_modifiers_and_webhooks_as_written() {
    let fixture = Fixture::order(DECLARED_AREAS);
    let body = "    routes.get(\"/files/{file}\", queries::file).signed().throttle(Rate::per_minute(10));\n    \
        routes\n        .post(\"/orders\", actions::place_order)\n        .idempotent()\n        \
        .body_limit(1024 * 1024)\n        .then_redirect(\"/orders\");\n    \
        routes.webhook(\"/payments/stripe\", actions::paid).body_limit(4096).verify(StripeSignature);\n";
    fixture.write("modules/order/src/areas/web/routes.rs", &routes_file(body));

    let routes = fixture.module("order").areas[0].routes.clone();

    assert_eq!(
        routes[0].modifiers,
        [
            Modifier::Signed,
            Modifier::Throttle("Rate::per_minute(10)".to_owned())
        ]
    );
    assert_eq!(
        routes[1].modifiers,
        [
            Modifier::Idempotent,
            Modifier::BodyLimit("1024 * 1024".to_owned())
        ]
    );
    assert_eq!(routes[1].then, Some(Then::Redirect("/orders".to_owned())));
    assert_eq!(routes[2].method, Method::Post);
    assert_eq!(routes[2].webhook.as_deref(), Some("StripeSignature"));
    assert_eq!(
        routes[2].modifiers,
        [Modifier::BodyLimit("4096".to_owned())]
    );
}

#[test]
fn refuses_modifiers_a_route_of_that_kind_does_not_have() {
    for (body, expected) in [
        (
            "    routes.get(\"/a\", queries::a).idempotent();\n",
            "this route is a page",
        ),
        (
            "    routes.webhook(\"/a\", actions::a).throttle(x).verify(V);\n",
            "this route is a webhook",
        ),
        (
            "    routes.webhook(\"/a\", actions::a);\n",
            "served only once `.verify(Verifier)`",
        ),
        (
            "    routes.post(\"/a\", actions::a).then_redirect(\"/b\").signed();\n",
            "an action",
        ),
    ] {
        let error = route_error(body);
        assert!(error.message.contains(expected), "{body}: {error}");
    }
}

#[test]
fn children_of_an_aggregate_are_kept_apart_from_its_columns() {
    let lib = "#[derive(Aggregate)]\n\
        pub struct Order { meta: Meta, name: String, lines: Children<Line> }\n";
    let fixture = Fixture::order(lib);

    let code = fixture.module("order").code;

    let [aggregate] = code.aggregates.as_slice() else {
        panic!("one aggregate: {:?}", code.aggregates);
    };
    let children: Vec<String> = aggregate
        .children
        .iter()
        .map(|child| format!("{}: {}", child.name, child.ty))
        .collect();
    assert_eq!(children, ["lines: Children<Line>"]);
    assert_eq!(
        aggregate.columns(),
        ["name", "version", "created_at", "updated_at"]
    );
}

#[test]
fn reads_functions_read_models_aggregates_and_raw_sql() {
    let lib = "mod queries;\nmod actions;\n\n#[derive(Aggregate)]\n\
        #[aggregate(behaviors(SoftDelete, Slugged(from = name)))]\n\
        pub struct Order { meta: Meta, name: String, total: Option<Money> }\n";
    let fixture = Fixture::order(lib);
    fixture.write(
        "modules/order/src/queries.rs",
        "#[derive(ReadModel)]\n#[read(from = crate::Order)]\npub struct OrderRow { r#id: Id<Order>, total: Option<Money> }\n\n\
         pub async fn list(read: Read) -> Result<Vec<OrderRow>> {\n    read.sql::<OrderRow>(\"select 1\").all().await\n}\n\n\
         pub async fn show() -> Result<Screen<OrderRow>> {\n    Ok(Screen::render(\"order\", row()))\n}\n",
    );
    fixture.write(
        "modules/order/src/actions.rs",
        "pub fn touch(read: &Read) { let _ = read.sql::<OrderRow>(\"x\"); }\n\n\
         #[derive(Input)]\n#[input(from = Order)]\npub struct Rename { #[max(9)] name: String, #[unconstrained] total: Option<Money> }\n\n\
         pub async fn rename(order: Order, input: Rename) -> Result<Order> { todo!() }\n\n\
         fn late() -> FieldError { FieldError::new(\"total\", \"order.late\", \"is late\") }\n",
    );

    let code = fixture.module("order").code;

    let [aggregate] = code.aggregates.as_slice() else {
        panic!("one aggregate: {:?}", code.aggregates);
    };
    assert_eq!(aggregate.behaviors, ["SoftDelete", "Slugged"]);
    assert_eq!(
        aggregate.columns(),
        [
            "name",
            "total",
            "version",
            "created_at",
            "updated_at",
            "deleted_at",
            "slug"
        ]
    );
    let row = code.read_model("OrderRow").unwrap();
    assert_eq!(row.from.as_deref(), Some("Order"));
    let fields: Vec<String> = row
        .fields
        .iter()
        .map(|field| format!("{}: {}", field.name, field.ty))
        .collect();
    assert_eq!(fields, ["id: Id<Order>", "total: Option<Money>"]);
    let list = code
        .function(&["queries".to_owned(), "list".to_owned()])
        .unwrap();
    assert_eq!(
        list.output.as_ref().unwrap().to_string(),
        "Result<Vec<OrderRow>>"
    );
    let show = code
        .function(&["queries".to_owned(), "show".to_owned()])
        .unwrap();
    assert_eq!(show.renders[0].screen.as_deref(), Some("order"));
    let rename = code
        .function(&["actions".to_owned(), "rename".to_owned()])
        .unwrap();
    let input = code.input_of(rename).unwrap();
    assert_eq!(input.path, ["actions", "Rename"]);
    assert_eq!(input.from.as_deref(), Some("Order"));
    assert_eq!(input.unconstrained, ["total"]);
    assert_eq!(input.fields[0].constraints, ["max"]);
    let mut raw: Vec<String> = code
        .raw_sql
        .iter()
        .map(|raw| {
            let text = raw.text.as_ref().map_or("", |(text, _)| text.as_str());
            format!("{} {text}", raw.module_path.join("::"))
        })
        .collect();
    raw.sort();
    assert_eq!(raw, ["actions x", "queries select 1"]);
    let keys: Vec<(&str, usize)> = code
        .translation_keys
        .iter()
        .map(|literal| (literal.key.as_str(), literal.line))
        .collect();
    assert_eq!(keys, [("order.late", 9)]);
}

#[test]
fn reads_gateways_and_the_adapters_implementing_them() {
    let fixture = Fixture::new().with_app(&["payment"]).with_module(
        "payment",
        "pub mod gateways;\npub use gateways::stripe::Stripe;\n",
    );
    fixture.write(
        "modules/payment/src/gateways.rs",
        "pub mod stripe;\npub mod paypal;\n\n#[sofie::gateway]\npub trait Payments {}\n\n#[gateway]\ntrait Refunds {}\n",
    );
    fixture.write(
        "modules/payment/src/gateways/stripe.rs",
        "use sofie::client::Adapter;\nuse super::Payments;\n\npub struct Stripe;\n\n\
         #[derive(sofie::Config)]\npub struct StripeSettings {\n    /// The API's secret key.\n    secret_key: String,\n    account: Option<String>,\n}\n\n\
         impl Adapter for Stripe {\n    const VENDOR: &'static str = \"stripe\";\n    type Settings = StripeSettings;\n}\n\n\
         #[sofie::gateway]\nimpl Payments for Stripe {}\n",
    );
    fixture.write(
        "modules/payment/src/gateways/paypal.rs",
        "pub struct Paypal;\n\nimpl crate::gateways::Payments for Paypal {}\n",
    );

    let gateways = fixture.module("payment").gateways;

    let adapter = |ty: &[&str], is_adapter, file: &str, line| sofie_model::GatewayAdapter {
        ty: ty.iter().map(|segment| (*segment).to_owned()).collect(),
        is_adapter,
        vendor: None,
        settings: Vec::new(),
        file: fixture.path(file),
        line,
    };
    let key = |name: &str, doc: &[&str], required| sofie_model::ConfigKey {
        name: name.to_owned(),
        doc: doc.iter().map(|line| (*line).to_owned()).collect(),
        kind: sofie_model::KeyKind::Text,
        required,
        fallback: None,
        secret: false,
    };
    assert_eq!(gateways.len(), 2);
    assert_eq!(
        (gateways[0].name.as_str(), gateways[0].path.as_slice()),
        (
            "payments",
            &["gateways".to_owned(), "Payments".to_owned()][..]
        )
    );
    let mut adapters = gateways[0].adapters.clone();
    adapters.sort_by(|left, right| left.ty.cmp(&right.ty));
    assert_eq!(
        adapters,
        [
            sofie_model::GatewayAdapter {
                vendor: Some("stripe".to_owned()),
                settings: vec![
                    key(
                        "PAYMENT_STRIPE_SECRET_KEY",
                        &["The API's secret key."],
                        true
                    ),
                    key("PAYMENT_STRIPE_ACCOUNT", &[], false),
                ],
                ..adapter(
                    &["Stripe"],
                    true,
                    "modules/payment/src/gateways/stripe.rs",
                    18,
                )
            },
            adapter(
                &["gateways", "paypal", "Paypal"],
                false,
                "modules/payment/src/gateways/paypal.rs",
                3
            ),
        ]
    );
    assert_eq!(gateways[1].name, "refunds");
    assert_eq!(gateways[1].adapters, []);
}

#[test]
fn knows_what_the_contract_exports_of_its_generated_code() {
    let fixture = Fixture::order("");
    with_contract(&fixture, "");
    fixture.write(
        "modules/order/contract/src/lib.rs",
        "pub mod area;\nmod r#gen;\npub use r#gen::{permissions, routes};\n",
    );

    assert_eq!(
        fixture.module("order").contract_exports,
        ["permissions", "routes"]
    );
}

#[test]
fn knows_the_sensitive_values_its_contract_declares() {
    let fixture = Fixture::order("");
    with_contract(&fixture, "");
    fixture.write(
        "modules/order/contract/src/lib.rs",
        "pub mod area;\n\n#[derive(Value)]\n#[sensitive]\npub struct Iban(String);\n",
    );

    assert_eq!(
        fixture.module("order").code.telemetry.sensitive_values,
        ["Iban"]
    );
}

#[test]
fn reads_jobs_mirrors_events_and_what_a_module_provides_other_modules() {
    let fixture = Fixture::new()
        .with_app(&["order"])
        .with_module(
            "order",
            "pub mod actions;\npub mod contract;\nmod copies;\npub mod extensions {\n    pub mod mail;\n}\n\n\
             pub use copies::MemberCopy;\n\n#[derive(Event)]\npub struct Kept {}\n",
        )
        .with_module("mail", "");
    with_contract(&fixture, "");
    fixture.write(
        "modules/order/contract/src/lib.rs",
        "#[derive(sofie::Event)]\npub struct OrderPlaced {}\n",
    );
    fixture.write(
        "modules/order/Cargo.toml",
        &manifest(
            "order",
            "order-contract = { path = \"contract\" }\nmail = { path = \"../mail\" }\n",
        ),
    );
    fixture.write(
        "modules/order/src/actions.rs",
        "#[job]\npub async fn send_receipt() {}\n\n#[every(\"1h\", each = stale)]\npub async fn expire() {}\n\n\
         pub async fn stale() {}\n",
    );
    fixture.write(
        "modules/order/src/contract.rs",
        "use order_contract::Orders;\n\npub struct OrderContract(Db);\n\nimpl Orders for OrderContract {}\n\n\
         impl From<Db> for OrderContract {\n    fn from(db: Db) -> Self { Self(db) }\n}\n",
    );
    fixture.write(
        "modules/order/src/extensions/mail.rs",
        "pub struct Unsubscribe;\n\nimpl mail::Unsubscribing for Unsubscribe {}\n",
    );
    fixture.write(
        "modules/order/src/copies.rs",
        "#[derive(Aggregate)]\n#[mirror(member_contract::Member)]\npub struct MemberCopy {\n    meta: Meta,\n}\n",
    );

    let order = fixture.module("order");

    let provision =
        |contract: &[&str], ty: &[&str], unit, file: &str, line| sofie_model::Provision {
            contract: contract
                .iter()
                .map(|segment| (*segment).to_owned())
                .collect(),
            ty: ty.iter().map(|segment| (*segment).to_owned()).collect(),
            unit,
            file: fixture.path(file),
            line,
        };
    assert_eq!(
        order.implementations,
        [provision(
            &["order_contract", "Orders"],
            &["contract", "OrderContract"],
            false,
            "modules/order/src/contract.rs",
            5
        )]
    );
    assert_eq!(
        order.extensions,
        [provision(
            &["mail", "Unsubscribing"],
            &["extensions", "mail", "Unsubscribe"],
            true,
            "modules/order/src/extensions/mail.rs",
            3
        )]
    );
    assert_eq!(order.jobs[0].function, ["actions", "send_receipt"]);
    assert_eq!(
        order.schedules[0].each,
        Some(vec!["actions".to_owned(), "stale".to_owned()])
    );
    let copy = &order.code.aggregates[0];
    assert_eq!(copy.path, ["MemberCopy"]);
    assert_eq!(
        copy.mirror,
        Some(vec!["member_contract".to_owned(), "Member".to_owned()])
    );
    assert_eq!(order.events, ["OrderPlaced", "Kept"]);
    assert_eq!(order.module_events[0].name, "Kept");
}

#[test]
fn reads_what_functions_and_methods_do_and_where_states_move() {
    let fixture = Fixture::order("mod actions;\nmod order;\npub use order::Order;\n");
    fixture.write(
        "modules/order/src/order.rs",
        "/// A member's order.\n#[derive(Aggregate)]\npub struct Order {\n    meta: Meta,\n    \
         #[owner]\n    account: AccountId,\n    status: Status,\n}\n\n\
         #[derive(Enum)]\npub enum Status {\n    #[to(Paid, Cancelled)]\n    Pending,\n    Paid,\n    Cancelled,\n}\n\n\
         impl Order {\n    pub fn cancel(&mut self) -> Result<()> {\n        \
         self.meta.raise(OrderCancelled { order: self.id() })\n    }\n}\n\n\
         pub fn place() -> Order {\n    Self::helper()\n}\n",
    );
    fixture.write(
        "modules/order/src/actions.rs",
        "use crate::order::{self, Order};\n\n/// Cancels it.\n/// More.\n\
         pub async fn cancel(mut order: Order, jobs: Jobs, orders: Dep<dyn Orders>) -> Result<Order> {\n    \
         order.cancel()?;\n    order::place();\n    jobs.queue(send_receipt, &order)?;\n    \
         let other = order.clone();\n    other.cancel()\n}\n",
    );

    let order = fixture.module("order");

    let cancel = &order.code.functions[0];
    assert_eq!(cancel.doc.as_deref(), Some("Cancels it."));
    assert_eq!(cancel.arguments[2].to_string(), "Dep<dyn Orders>");
    let calls: Vec<String> = cancel
        .effects
        .calls
        .iter()
        .map(|call| call.path.join("::"))
        .collect();
    assert_eq!(calls, ["order::place"]);
    let methods: Vec<(Option<&str>, &str)> = cancel
        .effects
        .method_calls
        .iter()
        .map(|call| (call.receiver.as_deref(), call.method.as_str()))
        .collect();
    assert_eq!(
        methods,
        [
            (Some("Order"), "cancel"),
            (Some("Order"), "clone"),
            (None, "cancel")
        ]
    );
    assert_eq!(
        cancel.effects.queued[0].job,
        Some(vec!["actions".to_owned(), "send_receipt".to_owned()])
    );
    let method = &order.code.methods[0];
    assert_eq!(
        (method.owner.as_str(), method.name.as_str()),
        ("Order", "cancel")
    );
    assert_eq!(
        method.effects.raised[0].event.as_deref(),
        Some("OrderCancelled")
    );
    let aggregate = &order.code.aggregates[0];
    assert_eq!(aggregate.owner.as_deref(), Some("account"));
    assert_eq!(aggregate.doc.as_deref(), Some("A member's order."));
    let status = &order.code.enums[0];
    assert!(status.has_transitions());
    assert_eq!(status.variants[0].to, ["Paid", "Cancelled"]);
    assert_eq!(status.variants[1].to, Vec::<String>::new());
}

#[cfg(unix)]
#[test]
fn a_symlink_back_to_a_listed_folder_is_listed_once() {
    let fixture = Fixture::new();
    fixture.write("src/a.rs", "");
    std::os::unix::fs::symlink(fixture.path("src"), fixture.path("src/back")).unwrap();

    let files = sofie_model::rust_files(&fixture.path("src")).unwrap();

    assert_eq!(files, [fixture.path("src/a.rs")]);
}

#[test]
fn lists_each_check_across_owners_by_its_aggregate_and_caller() {
    let fixture = Fixture::order(
        "use sofie::prelude::*;\n\
         #[derive(Aggregate)]\n\
         pub struct Booking { meta: Meta, slot: i64 }\n\
         impl Booking {\n\
             pub async fn clash(&self, read: &Read) -> Result<bool> {\n\
                 Self::across(read).where_(Self::SLOT.eq(self.slot)).exists().await\n\
             }\n\
         }\n\
         pub async fn book(read: Read) -> Result<u64> {\n\
             Booking::across(&read).count().await\n\
         }\n\
         pub async fn list(read: Read) -> Result<u64> {\n\
             Booking::query().count(&read).await\n\
         }\n",
    );

    let checks: Vec<(String, String, usize)> = fixture
        .module("order")
        .code
        .cross_owner_checks()
        .into_iter()
        .map(|check| (check.aggregate, check.caller, check.line))
        .collect();

    assert_eq!(
        checks,
        [
            ("Booking".to_owned(), "book".to_owned(), 10),
            ("Booking".to_owned(), "Booking::clash".to_owned(), 6),
        ]
    );
}

#[test]
fn reads_keys_sql_and_checks_across_owners_inside_macro_bodies() {
    let fixture = Fixture::order(
        "#[derive(Aggregate)]\n\
         pub struct Booking { meta: Meta, slot: i64 }\n\
         pub async fn book(read: Read) -> Result<Vec<FieldError>> {\n\
             let (a, b) = tokio::try_join!(read.sql::<Row>(\"select 1\").all(), Booking::across(&read).count())?;\n\
             Ok(vec![FieldError::new(\"slot\", \"booking.taken\", \"is taken\")])\n\
         }\n",
    );

    let code = fixture.module("order").code;

    let raw: Vec<Option<&str>> = code
        .raw_sql
        .iter()
        .map(|raw| raw.text.as_ref().map(|(text, _)| text.as_str()))
        .collect();
    assert_eq!(raw, [Some("select 1")]);
    let keys: Vec<(&str, usize)> = code
        .translation_keys
        .iter()
        .map(|literal| (literal.key.as_str(), literal.line))
        .collect();
    assert_eq!(keys, [("booking.taken", 5)]);
    let checks: Vec<(String, usize)> = code
        .cross_owner_checks()
        .into_iter()
        .map(|check| (check.caller, check.line))
        .collect();
    assert_eq!(checks, [("book".to_owned(), 4)]);
}

#[test]
fn records_what_trait_impl_methods_do() {
    let fixture = Fixture::order(
        "#[derive(Aggregate)]\n\
         pub struct Booking { meta: Meta, slot: i64 }\n\
         pub struct Bookings;\n\
         impl BookingsContract for Bookings {\n\
             async fn taken(&self, read: &Read) -> Result<u64> {\n\
                 Booking::across(read).count().await\n\
             }\n\
         }\n",
    );

    let checks: Vec<(String, usize)> = fixture
        .module("order")
        .code
        .cross_owner_checks()
        .into_iter()
        .map(|check| (check.caller, check.line))
        .collect();

    assert_eq!(checks, [("Bookings::taken".to_owned(), 6)]);
}

#[test]
fn marks_the_methods_of_a_trait_impl_with_their_trait() {
    let fixture = Fixture::order(
        "pub struct Bookings;\n\
         impl Bookings { pub fn open(&self) {} }\n\
         impl sofie::BookingsContract for Bookings { fn taken(&self) {} }\n",
    );

    let methods: Vec<(String, Option<String>)> = fixture
        .module("order")
        .code
        .methods
        .into_iter()
        .map(|method| (method.name, method.implements))
        .collect();

    assert_eq!(
        methods,
        [
            ("open".to_owned(), None),
            ("taken".to_owned(), Some("BookingsContract".to_owned()))
        ]
    );
}

#[test]
fn structure_ceilings_are_opted_into_through_the_app_s_metadata() {
    let fixture = Fixture::order("");
    let opted = |value: &str| {
        fixture.write(
            "app/Cargo.toml",
            &format!(
                "{}\n[package.metadata.sofie]\nstructure-ceilings = {value}\n",
                manifest("shop", "order = { path = \"../modules/order\" }\n")
            ),
        );
        fixture.load()
    };

    assert!(!fixture.load().unwrap().structure_ceilings);
    assert!(opted("true").unwrap().structure_ceilings);
    assert!(!opted("false").unwrap().structure_ceilings);
    let error = opted("\"yes\"").unwrap_err();
    assert!(
        error
            .message
            .contains("`[package.metadata.sofie] structure-ceilings` is `true` or `false`"),
        "{error}"
    );
}
