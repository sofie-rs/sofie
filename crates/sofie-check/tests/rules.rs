mod support;

use std::path::{Path, PathBuf};

use sofie_check::{AllowList, Finding, Screens, check_dead_code, check_structure};
use sofie_model::testing::{
    self, aggregate, area, define_area, dependency, field, krate, module, route, type_name,
};
use sofie_model::{
    Aggregate, App, Cadence, ContractMethod, ContractTrait, Dependency, EnumDeclaration,
    FieldDeclaration, Function, Gateway, GatewayAdapter, Identity, Job, Listener, Method, Module,
    ModuleConfig, RawSql, ReadModel, RemoteModule, Route, Schedule, ScreenOverride, ScreenRender,
    Source, Then, Tier, WallClock,
};
use support::{Fixture, manifest_dependencies};

const ORDERS_TABLE: &str = "<Section id=\"orders\" title=\"Orders\"></Section>\n";

fn check(app: &App) -> Vec<Finding> {
    sofie_check::check(app, &Screens::read(&app.modules))
}

fn module_with_routes(package: &str, area_name: &str, routes: Vec<Route>) -> Module {
    let mut module = module(package, format!("modules/{package}"));
    let contribution = area(&module, area_name, routes);
    module.areas.push(contribution);
    let mut name = package.to_owned();
    name[..1].make_ascii_uppercase();
    module.code.aggregates.push(aggregate(
        &name,
        Vec::new(),
        format!("modules/{package}/src/{package}.rs"),
    ));
    module
}

fn app_with(modules: Vec<Module>) -> App {
    testing::app(PathBuf::new(), modules)
}

fn screen(path: &str, name: &str, handler: &str, line: usize) -> Route {
    Route {
        screen: Some(name.to_owned()),
        ..route(Method::Get, path, handler, line)
    }
}

fn refreshing(path: &str, screen: &str, line: usize) -> Route {
    Route {
        then: Some(Then::Screen {
            screen: screen.to_owned(),
            fragment: "orders".to_owned(),
        }),
        ..route(Method::Post, path, &action_named_after(path), line)
    }
}

fn action_named_after(path: &str) -> String {
    format!("actions::{}", path.rsplit('/').next().unwrap_or_default())
}

fn rule_lines(findings: &[Finding]) -> Vec<(&'static str, usize)> {
    findings
        .iter()
        .map(|finding| (finding.rule.name(), finding.line))
        .collect()
}

#[track_caller]
fn assert_contains(text: &str, expected: &str) {
    assert!(
        text.contains(expected),
        "{text:?} does not contain {expected:?}"
    );
}

fn files(findings: &[Finding]) -> Vec<&Path> {
    findings
        .iter()
        .map(|finding| finding.file.as_path())
        .collect()
}

#[test]
fn a_clean_app_has_no_findings() {
    let fixture = Fixture::new();
    let mut order = fixture.module(
        "order",
        "[package]\nname = \"order\"\n\n[dependencies]\nsofie = { workspace = true }\n",
        "use std::net::Ipv4Addr;\npub const NAME: &str = env!(\"CARGO_PKG_NAME\");\n\
         pub fn local(curl: u8) -> u8 { let env = curl; env }\n",
    );
    let web = area(
        &order,
        "web",
        vec![
            route(Method::Get, "/orders", "queries::list_orders", 2),
            route(Method::Post, "/orders", "actions::place_order", 3),
        ],
    );
    order.areas.push(web);
    let mut app = fixture.app(vec![order]);
    let main = fixture.write(
        "app/src/main.rs",
        "fn main() { std::env::var(\"DATABASE_URL\"); }\n",
    );
    app.app.sources = vec![Source::read(&main).unwrap()];
    assert_eq!(check(&app), []);
}

#[test]
fn a_module_depending_on_another_module_is_found() {
    let fixture = Fixture::new();
    let order = fixture.module("order", "[dependencies]\nsofie = \"0\"\n", "");
    let payment = fixture.module(
        "payment",
        "[package]\n\n[dependencies]\nsofie = \"0\"\norder = { path = \"../order\" }\n",
        "",
    );
    let findings = check(&fixture.app(vec![order, payment]));
    assert_eq!(rule_lines(&findings), [("module-dependency", 5)]);
    assert_eq!(
        files(&findings),
        [fixture.root().join("modules/payment/Cargo.toml")]
    );
    assert_contains(&findings[0].message, "`payment` depends on module `order`");
}

#[test]
fn a_module_may_depend_on_contracts_and_a_contract_on_no_implementation() {
    let fixture = Fixture::new();
    let order = with_contract(
        &fixture,
        fixture.module("order", "[dependencies]\nsofie = \"0\"\n", ""),
        "[package]\n\n[dependencies]\nsofie = \"0\"\npayment = { path = \"../../payment\" }\norder = { path = \"..\" }\n",
    );
    let payment = fixture.module(
        "payment",
        "[package]\n\n[dependencies]\nsofie = \"0\"\norder-contract = { path = \"../order/contract\" }\n",
        "",
    );

    let findings = check(&fixture.app(vec![order, payment]));

    assert_eq!(
        rule_lines(&findings),
        [("module-dependency", 5), ("module-dependency", 6)]
    );
    let contract = fixture.root().join("modules/order/contract/Cargo.toml");
    assert_eq!(files(&findings), [&contract, &contract]);
    assert_contains(
        &findings[0].message,
        "contract `order-contract` depends on `payment` of module `payment`",
    );
}

#[test]
fn reading_the_environment_in_a_module_is_found() {
    let fixture = Fixture::new();
    let source = "use std::env;\n\
                  fn a() { let _ = env::var(\"KEY\"); }\n\
                  fn b() { let _ = option_env!(\"API_TOKEN\"); }\n\
                  fn c() { println!(\"{}\", env!(\"HOME\")); }\n\
                  const PACKAGE: &str = env!(\"CARGO_PKG_NAME\");\n\
                  fn d(env: u8) -> u8 { env }\n\
                  fn e() { let _ = (env::args(), std::env::current_dir(), std::env::consts::OS); }\n\
                  fn f() { let _ = std::env::var_os(\"KEY\"); }\n";
    let module = fixture.module("order", "[dependencies]\n", source);
    let findings = check(&fixture.app(vec![module]));
    let expected = [
        ("environment", 2),
        ("environment", 3),
        ("environment", 4),
        ("environment", 8),
    ];
    assert_eq!(rule_lines(&findings), expected);
}

#[test]
fn listening_and_unix_sockets_in_a_module_are_found() {
    let fixture = Fixture::new();
    let source = "fn a() { let _ = std::net::TcpListener::bind; }\n\
                  fn b() { let _ = std::os::unix::net::UnixStream::connect; }\n\
                  fn c() { let _ = tokio::net::UnixListener::bind; }\n\
                  fn d() { let _ = std::os::unix::net::UnixDatagram::unbound; }\n";
    let module = fixture.module("payment", "[dependencies]\n", source);

    let findings = check(&fixture.app(vec![module]));

    let expected = [
        ("outgoing", 1),
        ("outgoing", 2),
        ("outgoing", 3),
        ("outgoing", 4),
    ];
    assert_eq!(rule_lines(&findings), expected);
}

#[test]
fn http_clients_and_raw_sockets_in_a_module_are_found() {
    let fixture = Fixture::new();
    let source = "use reqwest::Client;\n\
                  use std::net::{Ipv4Addr, TcpStream};\n\
                  fn a() { let _ = hyper_util::client::legacy::Client::builder; }\n\
                  fn b() { println!(\"{:?}\", tokio::net::UdpSocket::bind); }\n\
                  fn c(curl: u8) -> u8 { curl }\n\
                  fn d() -> Ipv4Addr { Ipv4Addr::LOCALHOST }\n";
    let module = fixture.module(
        "payment",
        "[dependencies]\nsofie = \"0\"\nreqwest = \"0\"\n",
        source,
    );
    let findings = check(&fixture.app(vec![module]));
    let expected = [
        ("outgoing", 3),
        ("outgoing", 1),
        ("outgoing", 2),
        ("outgoing", 3),
        ("outgoing", 4),
    ];
    assert_eq!(rule_lines(&findings), expected);
    assert_eq!(
        findings[0].file,
        fixture.root().join("modules/payment/Cargo.toml")
    );
}

#[test]
fn a_vendor_sdk_pulling_in_an_http_client_and_sockets_in_the_app_are_found() {
    let fixture = Fixture::new();
    fixture.write(
        "Cargo.lock",
        "version = 4\n\n[[package]]\nname = \"stripe-sdk\"\nversion = \"1.0.0\"\n\
         dependencies = [\n \"http-layer\",\n]\n\n\
         [[package]]\nname = \"http-layer\"\nversion = \"0.2.0\"\n\
         dependencies = [\n \"reqwest 0.12.0\",\n]\n\n\
         [[package]]\nname = \"sofie-framework\"\nversion = \"0.0.0\"\n\
         dependencies = [\n \"hyper\",\n]\n",
    );
    let module = fixture.module(
        "payment",
        "[dependencies]\nsofie-framework = \"0\"\nserde = \"1\"\nstripe-sdk = \"1\"\n",
        "use std::net::*;\nuse std::net::{self, IpAddr};\n",
    );
    let mut app = fixture.app(vec![module]);
    let main = fixture.write(
        "app/src/main.rs",
        "use std::net::{self, IpAddr};\nfn main() { let _ = tokio::net::TcpStream::connect; }\n",
    );
    app.app.sources = vec![Source::read(&main).expect("the fixture source parses")];

    let findings = check(&app);

    let outgoing: Vec<(&Path, usize)> = findings
        .iter()
        .filter(|finding| finding.rule.name() == "outgoing")
        .map(|finding| (finding.file.as_path(), finding.line))
        .collect();
    assert_eq!(
        outgoing,
        [
            (main.as_path(), 2),
            (
                fixture.root().join("modules/payment/Cargo.toml").as_path(),
                4
            ),
            (
                fixture.root().join("modules/payment/src/lib.rs").as_path(),
                1
            ),
        ]
    );
    let pulled_in = findings
        .iter()
        .find(|finding| finding.rule.name() == "outgoing" && finding.line == 4)
        .map_or("", |finding| finding.message.as_str());
    assert_contains(pulled_in, "`stripe-sdk` pulls in `reqwest`");
}

#[test]
fn tls_crates_sofie_client_and_an_outgoing_outside_gateways_are_found() {
    let fixture = Fixture::new();
    let source = "use tokio_rustls::TlsConnector;\n\
                  use sofie::client::Outgoing;\n\
                  fn a() { let _ = tokio::net::TcpSocket::new_v4; }\n";
    let mut module = fixture.module(
        "payment",
        "[dependencies]\nsofie = \"0\"\nhyper-util = \"0\"\nsofie-client = \"0\"\n",
        source,
    );
    let adapter = fixture.write(
        "modules/payment/src/gateways/stripe/client.rs",
        "use sofie::client::Outgoing;\npub struct StripeClient { http: Outgoing }\n",
    );
    module
        .krate
        .sources
        .push(Source::read(&adapter).expect("the fixture source parses"));

    let findings = check(&fixture.app(vec![module]));

    let expected = [
        ("outgoing", 3),
        ("outgoing", 4),
        ("outgoing", 1),
        ("outgoing", 2),
        ("outgoing", 3),
    ];
    assert_eq!(rule_lines(&findings), expected);
    assert_contains(&findings[3].message, "outside `src/gateways/`");
}

#[test]
fn a_gateway_without_adapters_and_an_adapter_the_wiring_cannot_build_are_found() {
    let fixture = Fixture::new();
    let mut module = module("payment", fixture.root().join("modules/payment"));
    let file = fixture.root().join("modules/payment/src/gateways/mod.rs");
    let gateway = |name: &str, line, adapters| Gateway {
        name: name.to_owned(),
        path: vec!["gateways".to_owned(), name.to_owned()],
        adapters,
        file: file.clone(),
        line,
    };
    let adapter = |is_adapter, line| GatewayAdapter {
        ty: vec!["gateways".to_owned(), "Stripe".to_owned()],
        is_adapter,
        vendor: None,
        settings: Vec::new(),
        file: fixture
            .root()
            .join("modules/payment/src/gateways/stripe.rs"),
        line,
    };
    module.gateways = vec![
        gateway("Payments", 3, vec![adapter(true, 5), adapter(false, 9)]),
        gateway("Refunds", 7, Vec::new()),
    ];

    let findings = check(&fixture.app(vec![module]));

    assert_eq!(rule_lines(&findings), [("outgoing", 7), ("outgoing", 9)]);
    assert_contains(&findings[1].message, "not `sofie::client::Adapter`");
}

#[test]
fn database_drivers_and_pools_in_a_module_are_found() {
    let fixture = Fixture::new();
    let manifest = "[dependencies]\nsofie = \"0\"\ndb = { package = \"sqlx\", version = \"0\" }\n\
                    deadpool-postgres = \"0\"\n";
    let source = "use tokio_postgres::Client;\n\
                  extern crate rusqlite;\n\
                  fn a() { let _ = sqlx::query!(\"select 1\"); }\n\
                  fn b() { let _ = deadpool_postgres::Pool::builder; }\n\
                  fn c(postgres: u8) -> u8 { postgres }\n";
    let mut module = fixture.module("order", manifest, source);
    module.krate.dependencies = ["sofie", "sqlx", "deadpool-postgres"]
        .map(dependency)
        .to_vec();

    let findings = check(&fixture.app(vec![module]));

    let expected = [
        ("database-driver", 3),
        ("database-driver", 4),
        ("database-driver", 1),
        ("database-driver", 2),
        ("database-driver", 3),
        ("database-driver", 4),
    ];
    assert_eq!(rule_lines(&findings), expected);
    assert_eq!(
        findings[0].file,
        fixture.root().join("modules/order/Cargo.toml")
    );
    assert_contains(&findings[0].message, "`sqlx`");
    assert_contains(&findings[4].message, "`sqlx::query!`");
    assert_contains(&findings[4].message, "owner scoping");
}

#[test]
fn a_get_must_route_a_query_and_every_other_method_an_action() {
    let routes = vec![
        route(Method::Get, "/orders", "queries::list_orders", 1),
        route(Method::Get, "/orders/export", "actions::export_orders", 2),
        route(
            Method::Delete,
            "/orders/{order}",
            "actions::delete_order",
            3,
        ),
        route(Method::Post, "/orders/search", "queries::search_orders", 4),
    ];
    let findings = check(&app_with(vec![module_with_routes("order", "web", routes)]));
    assert_eq!(
        rule_lines(&findings),
        [("route-kind", 2), ("route-kind", 4)]
    );
}

#[test]
fn a_screen_route_needs_its_template_and_a_query_loader() {
    let routes = vec![
        screen("/orders", "orders", "queries::list_orders", 1),
        screen("/invoices", "invoices", "queries::list_invoices", 2),
        screen("/export", "orders", "actions::export_orders", 3),
    ];
    let fixture = Fixture::new();
    let module = fixture.module_with_screens(routes, &[("orders", ORDERS_TABLE)]);

    let findings = check(&fixture.app(vec![module]));

    assert_eq!(
        rule_lines(&findings),
        [("missing-screen", 2), ("route-kind", 3)]
    );
    assert_contains(
        &findings[0].message,
        "modules/order/src/areas/web/screens/invoices.html does not exist",
    );
}

#[test]
fn a_resource_s_routes_are_served_by_the_framework_and_need_no_handler_or_template() {
    let routes = vec![
        screen("/orders", "order_index", "sofie::resource::list_orders", 1),
        route(
            Method::Post,
            "/orders/new",
            "sofie::resource::create_order",
            2,
        ),
    ];
    let fixture = Fixture::new();
    let module = fixture.module_with_screens(routes, &[]);

    let findings = check(&fixture.app(vec![module]));

    assert_eq!(rule_lines(&findings), []);
}

#[test]
fn then_screen_needs_exactly_one_screen_route_in_its_area() {
    let routes = vec![
        screen("/orders", "orders", "queries::list_orders", 1),
        screen("/orders/all", "all", "queries::all_orders", 2),
        screen("/orders/old", "all", "queries::old_orders", 3),
        refreshing("/orders/{order}/cancel", "orders", 4),
        refreshing("/orders/{order}/pay", "invoices", 5),
        refreshing("/orders/{order}/ship", "all", 6),
    ];
    let fixture = Fixture::new();
    let module =
        fixture.module_with_screens(routes, &[("orders", ORDERS_TABLE), ("all", ORDERS_TABLE)]);

    let findings = check(&fixture.app(vec![module]));

    assert_eq!(
        rule_lines(&findings),
        [("then-screen", 5), ("then-screen", 6)]
    );
    assert_contains(
        &findings[0].message,
        "POST /orders/{order}/pay refreshes screen `invoices`, but no",
    );
    assert_contains(&findings[1].message, "2 `.screen(…)` routes");
}

fn refreshing_element(path: &str, screen: &str, fragment: &str, line: usize) -> Route {
    Route {
        then: Some(Then::Screen {
            screen: screen.to_owned(),
            fragment: fragment.to_owned(),
        }),
        ..route(Method::Post, path, &action_named_after(path), line)
    }
}

const ORDERS_SCREEN: &str = "<Section title=\"Orders\">\n\
    <!-- <Stack id=\"commented\"></Stack> -->\n\
    <Table id=\"orders\" of={data} columns=\"id\" as=\"order\">\n\
    <Action post=\"/orders/{order.id}/cancel\" label=\"Cancel\" />\n\
    </Table>\n\
    {#each data as order}<div id=\"row-{order.id}\">x</div>{/each}\n\
    <p title=\"a > b\">id=\"prose\"</p>\n\
    </Section>\n";

#[test]
fn then_screen_must_name_an_element_its_screen_renders() {
    let fixture = Fixture::new();
    let routes = vec![
        screen("/orders", "orders", "queries::list_orders", 1),
        screen("/broken", "broken", "queries::broken", 2),
        refreshing_element("/orders/a", "orders", "orders", 3),
        refreshing_element("/orders/b", "orders", "row-7", 4),
        refreshing_element("/orders/c", "orders", "commented", 5),
        refreshing_element("/orders/d", "orders", "prose", 6),
        refreshing_element("/orders/e", "broken", "orders", 7),
    ];
    let broken = "<Section title=\"A\">\n<Nope />\n</Section>\n";
    let module =
        fixture.module_with_screens(routes, &[("broken", broken), ("orders", ORDERS_SCREEN)]);

    let findings = check(&fixture.app(vec![module]));

    assert_eq!(
        rule_lines(&findings),
        [
            ("then-screen-fragment", 5),
            ("then-screen-fragment", 6),
            ("screen-template", 2)
        ]
    );
    assert!(
        findings[2].file.ends_with("screens/broken.html"),
        "{}",
        findings[2].file.display()
    );
    assert_contains(&findings[2].message, "Nope");
    assert_contains(
        &findings[0].message,
        "POST /orders/c refreshes element `commented` of screen `orders`",
    );
}

#[test]
fn the_same_method_and_path_in_one_area_conflicts_across_modules() {
    let order = module_with_routes(
        "order",
        "web",
        vec![
            route(Method::Get, "/orders", "queries::list_orders", 1),
            route(Method::Post, "/orders", "actions::place_order", 2),
        ],
    );
    let report = module_with_routes(
        "report",
        "web",
        vec![route(Method::Get, "/orders", "queries::orders", 7)],
    );
    let mut console = module_with_routes(
        "console",
        "console",
        vec![route(Method::Get, "/orders", "queries::orders", 9)],
    );
    define_area(&mut console, "console");
    console.contract_declares_gen = true;
    console.contract_exports = vec!["routes".to_owned(), "pruned".to_owned()];
    let findings = check(&app_with(vec![order, report, console]));
    assert_eq!(
        rule_lines(&findings),
        [("route-conflict", 1), ("route-conflict", 7)]
    );
    assert_contains(
        &findings[0].message,
        "module `report` at modules/report/src/areas/web/routes.rs:7",
    );
    assert_contains(
        &findings[1].message,
        "module `order` at modules/order/src/areas/web/routes.rs:1",
    );
}

#[test]
fn routes_conflict_on_their_full_path_across_areas_and_identity() {
    let order = module_with_routes(
        "order",
        "web",
        vec![route(Method::Get, "/account/orders", "queries::orders", 1)],
    );
    let mut console = module_with_routes(
        "console",
        "console",
        vec![
            route(Method::Get, "/orders", "queries::orders", 2),
            route(Method::Get, "/tokens", "queries::tokens", 3),
        ],
    );
    define_area(&mut console, "console");
    console.contract_declares_gen = true;
    console.contract_exports = vec!["routes".to_owned()];
    let mut app = app_with(vec![order, console]);
    app.identity = Some(Identity {
        routes: vec![sofie_model::ListedRoute {
            mount: "permitted_routes".to_owned(),
            method: "GET".to_owned(),
            path: "/tokens".to_owned(),
            handler: "list_tokens".to_owned(),
            permission: None,
        }],
        mounts: vec![sofie_model::RouteMount {
            mount: "permitted_routes".to_owned(),
            area_type: "Member".to_owned(),
        }],
        ..Identity::default()
    });

    let findings = check(&app);

    let conflicts: Vec<(usize, &str)> = findings
        .iter()
        .filter(|finding| finding.rule.name() == "route-conflict")
        .map(|finding| (finding.line, finding.message.as_str()))
        .collect();
    let lines: Vec<usize> = conflicts.iter().map(|(line, _)| *line).collect();
    assert_eq!(lines, [2, 3, 1]);
    assert_contains(
        conflicts[1].1,
        "served at /account/tokens, which is also routed by sofie_identity's `list_tokens`",
    );
}

#[test]
fn a_parameter_and_a_catch_all_at_the_same_place_conflict_as_in_the_router() {
    let order = module_with_routes(
        "order",
        "web",
        vec![
            route(Method::Get, "/files/{order}", "queries::file", 1),
            route(Method::Get, "/files/{*path}", "queries::files", 2),
            route(Method::Get, "/files/{order}/meta", "queries::meta", 3),
            route(Method::Get, "/files/new", "queries::new_file", 4),
        ],
    );

    let findings = check(&app_with(vec![order]));

    assert_eq!(
        rule_lines(&findings),
        [
            ("route-conflict", 1),
            ("route-conflict", 2),
            ("route-conflict", 3)
        ]
    );
}

#[test]
fn paths_sharing_part_of_a_non_ascii_character_do_not_conflict() {
    let order = module_with_routes(
        "order",
        "web",
        vec![
            route(Method::Get, "/caf\u{e9}", "queries::cafe", 1),
            route(Method::Get, "/caf\u{e8}", "queries::other_cafe", 2),
        ],
    );

    let findings = check(&app_with(vec![order]));

    assert_eq!(rule_lines(&findings), []);
}

#[test]
fn a_route_conflicts_with_a_route_a_framework_package_serves_in_its_area() {
    let order = module_with_routes(
        "order",
        "web",
        vec![route(Method::Get, "/sitemap.xml", "queries::sitemap", 4)],
    );
    let mut app = app_with(vec![order]);
    app.packages = vec![sofie_model::RoutedPackage {
        crate_name: "sofie_content".to_owned(),
        routes: vec![sofie_model::ListedRoute {
            mount: "routes".to_owned(),
            method: "GET".to_owned(),
            path: "/sitemap.xml".to_owned(),
            handler: "sitemap".to_owned(),
            permission: None,
        }],
        mounts: vec![sofie_model::RouteMount {
            mount: "routes".to_owned(),
            area_type: "Web".to_owned(),
        }],
    }];

    let findings = check(&app);

    assert_eq!(rule_lines(&findings), [("route-conflict", 4)]);
    assert_contains(&findings[0].message, "sofie_content's `sitemap`");
}

#[test]
fn identity_routes_it_cannot_read_are_reported() {
    let mut console = module_with_routes("console", "console", Vec::new());
    define_area(&mut console, "console");
    let mut app = app_with(vec![console]);
    app.identity = Some(Identity {
        routes: vec![sofie_model::ListedRoute {
            mount: "routes".to_owned(),
            method: "GET".to_owned(),
            path: "/profile".to_owned(),
            handler: "profile".to_owned(),
            permission: None,
        }],
        ..Identity::default()
    });

    let findings = check(&app);

    let unread = findings
        .iter()
        .find(|finding| {
            finding
                .message
                .contains("sofie-identity's routes could not be read")
        })
        .expect("the unread identity routes are reported");
    assert_eq!(
        unread.file,
        PathBuf::from("modules/console/contract/src/area.rs")
    );
}

#[test]
fn an_unresolved_handler_is_reported_as_unchecked() {
    let order = module_with_routes(
        "order",
        "web",
        vec![
            route(Method::Get, "/orders", "list_orders", 1),
            route(Method::Get, "/orders/new", "actions::new_order", 2),
        ],
    );

    let findings = check(&app_with(vec![order]));

    let kinds: Vec<&sofie_check::Finding> = findings
        .iter()
        .filter(|finding| finding.rule.name() == "route-kind")
        .collect();
    assert_eq!(kinds.len(), 2, "{kinds:?}");
    assert_eq!((kinds[0].line, kinds[1].line), (1, 2));
    assert!(
        kinds[0].message.contains("cannot be traced"),
        "{}",
        kinds[0].message
    );
}

#[test]
fn a_wired_item_its_wiring_cannot_reach_is_found_and_a_private_root_module_is_not() {
    let mut order = module_with_routes("order", "customer", Vec::new());
    order.listeners = vec![listener("OrderPlaced", 1)];
    let private = |path: &str, line| sofie_model::PrivateItem {
        path: path.split("::").map(str::to_owned).collect(),
        file: PathBuf::from("modules/order/src/lib.rs"),
        line,
        crate_visible: false,
    };
    order.private_items = vec![
        private("listeners", 5),
        private("areas", 7),
        private("areas::customer", 10),
        private("audit", 2),
    ];

    let findings = check(&app_with(vec![order]));

    let visibility: Vec<(usize, &str)> = findings
        .iter()
        .filter(|finding| finding.rule.name() == "visibility")
        .map(|finding| (finding.line, finding.message.as_str()))
        .collect();
    assert_eq!(visibility.len(), 1, "{visibility:?}");
    assert_eq!(visibility[0].0, 10);
    assert_contains(visibility[0].1, "`order::areas::customer::routes`");
    assert_contains(visibility[0].1, "src/wiring.rs");
}

/// The module's wiring is inside its crate, so `pub(crate)` reaches it; the app's wiring is not.
#[test]
fn a_crate_visible_item_hides_nothing_from_the_module_s_own_wiring() {
    let mut order = module_with_routes("order", "customer", Vec::new());
    order.tasks = vec![sofie_model::Task {
        file: PathBuf::from("modules/order/src/tasks.rs"),
        line: 3,
        function: vec!["tasks".to_owned(), "reindex".to_owned()],
    }];
    let crate_visible = |path: &str, line| sofie_model::PrivateItem {
        path: path.split("::").map(str::to_owned).collect(),
        file: PathBuf::from("modules/order/src/lib.rs"),
        line,
        crate_visible: true,
    };
    order.private_items = vec![
        crate_visible("areas::customer", 10),
        crate_visible("tasks", 12),
    ];

    let findings = check(&app_with(vec![order]));

    let visibility: Vec<usize> = findings
        .iter()
        .filter(|finding| finding.rule.name() == "visibility")
        .map(|finding| finding.line)
        .collect();
    assert_eq!(visibility, [12]);
}

#[test]
fn a_routes_file_outside_the_module_tree_is_found() {
    let mut module = module_with_routes("order", "web", Vec::new());
    let mut console = area(&module, "console", Vec::new());
    console.declared = false;
    module.areas.push(console);
    let findings = check(&app_with(vec![module]));
    assert_eq!(rule_lines(&findings), [("undeclared-routes", 1)]);
    assert_eq!(
        files(&findings),
        [Path::new("modules/order/src/areas/console/routes.rs")]
    );
}

fn listener_named(function: &str, event: &str, line: usize) -> Listener {
    Listener {
        function: vec!["listeners".to_owned(), function.to_owned()],
        ..listener(event, line)
    }
}

fn listener(event: &str, line: usize) -> Listener {
    Listener {
        file: PathBuf::from("modules/order/src/listeners.rs"),
        line,
        function: vec!["listeners".to_owned(), "audit".to_owned()],
        event: vec!["crate".to_owned(), "events".to_owned(), event.to_owned()],
    }
}

fn schedule(interval: &str, line: usize) -> Schedule {
    Schedule {
        file: PathBuf::from("modules/order/src/jobs.rs"),
        line,
        function: vec!["jobs".to_owned(), format!("expire_{line}")],
        cadence: Cadence::Interval(interval.to_owned()),
        each: None,
    }
}

#[test]
fn a_listener_hears_an_event_some_module_derives() {
    let mut order = module("order", "modules/order");
    order.events = vec!["OrderCancelled".to_owned()];
    order.listeners = vec![
        listener("OrderCancelled", 1),
        listener_named("record", "OrderPlaced", 2),
    ];
    let mut audit = module("audit", "modules/audit");
    audit.listeners = vec![listener("OrderCancelled", 3)];

    let findings = check(&app_with(vec![order, audit]));

    assert_eq!(rule_lines(&findings), [("listener-event", 2)]);
}

#[test]
fn a_listener_hears_an_event_a_framework_package_raises() {
    let mut order = module("order", "modules/order");
    order.listeners = vec![
        Listener {
            event: vec!["sofie_identity".to_owned(), "SignInLocked".to_owned()],
            ..listener_named("warn", "SignInLocked", 1)
        },
        listener_named("greet", "MemberJoined", 2),
    ];
    let mut app = app_with(vec![order]);
    app.package_events = vec![sofie_model::EventDeclaration {
        name: "SignInLocked".to_owned(),
        file: PathBuf::from("sofie-identity/src/lock.rs"),
        line: 11,
    }];

    let findings = check(&app);

    assert_eq!(rule_lines(&findings), [("listener-event", 2)]);
}

#[test]
fn a_listener_hears_the_pruned_event_of_an_aggregate() {
    let mut order = module("order", "modules/order");
    order.code.aggregates.push(testing::aggregate(
        "Order",
        Vec::new(),
        "modules/order/src/order.rs",
    ));
    order.listeners = vec![
        listener_named("forget", "OrderPruned", 1),
        listener_named("free", "RoomPruned", 2),
    ];

    let findings = check(&app_with(vec![order]));

    assert_eq!(rule_lines(&findings), [("listener-event", 2)]);
}

#[test]
fn an_event_is_declared_in_the_contract_and_an_each_query_in_the_module() {
    let mut order = module("order", "modules/order");
    order.module_events = vec![sofie_model::EventDeclaration {
        name: "OrderPlaced".to_owned(),
        file: PathBuf::from("modules/order/src/events.rs"),
        line: 4,
    }];
    order.code.functions.push(testing::function(
        "jobs::stale",
        "modules/order/src/jobs.rs",
        9,
    ));
    let each = |query: &str, line| Schedule {
        each: Some(vec!["jobs".to_owned(), query.to_owned()]),
        ..schedule("1h", line)
    };
    order.schedules = vec![each("stale", 1), each("missing", 2)];

    let findings = check(&app_with(vec![order]));

    assert_eq!(
        rule_lines(&findings),
        [("event-in-contract", 4), ("schedule-each", 2)]
    );
}

#[test]
fn a_schedule_needs_a_readable_interval() {
    let mut order = module("order", "modules/order");
    order.schedules = vec![schedule("1h", 1), schedule("15", 2), schedule("1w", 3)];

    let findings = check(&app_with(vec![order]));

    assert_eq!(
        rule_lines(&findings),
        [("schedule-interval", 2), ("schedule-interval", 3)]
    );
}

#[test]
fn a_wall_clock_schedule_needs_a_zone_the_zone_database_knows() {
    let mut order = module("order", "modules/order");
    let daily = |zone: &str, line| Schedule {
        cadence: Cadence::WallClock(WallClock {
            daily_at: "03:00".to_owned(),
            weekly_on: None,
            zone: zone.to_owned(),
        }),
        ..schedule("1h", line)
    };
    order.schedules = vec![
        daily("Europe/Istanbul", 1),
        daily("Mars/Olympus", 2),
        daily("", 3),
        daily(" Europe/Istanbul", 4),
    ];

    let findings = check(&app_with(vec![order]));

    assert_eq!(
        rule_lines(&findings),
        [
            ("schedule-zone", 2),
            ("schedule-zone", 3),
            ("schedule-zone", 4)
        ]
    );
}

#[test]
fn jobs_are_named_once_and_schedules_run_within_range_as_at_boot() {
    let mut order = module("order", "modules/order");
    let each = |query: &str, line| Schedule {
        function: vec!["jobs".to_owned(), "expire".to_owned()],
        each: Some(vec!["jobs".to_owned(), query.to_owned()]),
        ..schedule("1h", line)
    };
    order.schedules = vec![each("stale", 1), each("late", 2), schedule("400000d", 3)];

    let findings: Vec<Finding> = check(&app_with(vec![order]))
        .into_iter()
        .filter(|finding| matches!(finding.rule.name(), "job-name" | "schedule-interval"))
        .collect();

    assert_eq!(
        rule_lines(&findings),
        [("job-name", 1), ("job-name", 2), ("schedule-interval", 3)]
    );
    assert_contains(
        &findings[0].message,
        "job `order.jobs.expire` shares its name with the job at modules/order/src/jobs.rs:2",
    );
    assert_contains(
        &findings[2].message,
        "runs every 34560000000s; a schedule runs at most once a second",
    );
}

#[test]
fn routes_differing_only_in_parameter_names_conflict() {
    let order = module_with_routes(
        "order",
        "web",
        vec![
            route(Method::Get, "/orders/{id}", "queries::show_order", 1),
            route(Method::Get, "/orders/{order}", "queries::order", 2),
            route(Method::Get, "/orders/{order}/lines", "queries::lines", 3),
        ],
    );

    let findings = check(&app_with(vec![order]));

    assert_eq!(
        rule_lines(&findings),
        [
            ("route-conflict", 1),
            ("route-name", 1),
            ("route-conflict", 2)
        ]
    );
}

#[test]
fn the_app_depends_on_every_module_and_routes_only_to_areas_that_exist() {
    let mut order = module("order", "shop/modules/order");
    let admin = area(&order, "admin", Vec::new());
    order.areas.push(admin);
    let mut app = testing::app("shop", vec![order]);
    app.app.dependencies.clear();

    let findings = check(&app);

    assert_eq!(
        rule_lines(&findings),
        [("app-dependency", 1), ("unknown-area", 1)]
    );
    assert_eq!(findings[0].file, Path::new("shop/app/Cargo.toml"));
    assert_contains(&findings[0].fix, "order = { path = \"../modules/order\" }");
    assert_contains(&findings[1].message, "no area is named `admin`");
}

#[test]
fn an_area_is_defined_once_by_a_contract_an_app_signing_members_in_depends_on() {
    let mut order = module_with_routes("order", "member", Vec::new());
    define_area(&mut order, "member");
    order.contract_declares_gen = true;
    order.contract_exports = vec!["pruned".to_owned()];
    let mut payment = module("payment", "modules/payment");
    define_area(&mut payment, "member");
    define_area(&mut payment, "web");
    payment.defined_areas[1].prefix = Some("/web".to_owned());
    let mut app = app_with(vec![order, payment]);
    app.identity = Some(sofie_model::Identity {
        directory: PathBuf::from("crates/sofie-identity"),
        screens: Vec::new(),
        routes: Vec::new(),
        mounts: Vec::new(),
        shipped: Vec::new(),
    });
    app.app
        .dependencies
        .retain(|dependency| dependency.package != "order-contract");

    let findings = check(&app);

    let found: Vec<(&str, &Path)> = findings
        .iter()
        .map(|finding| (finding.rule.name(), finding.file.as_path()))
        .collect();
    assert_eq!(
        found,
        [
            (
                "app-dependency",
                Path::new("modules/order/contract/src/area.rs")
            ),
            (
                "area-definition",
                Path::new("modules/payment/contract/src/area.rs")
            ),
            (
                "area-definition",
                Path::new("modules/payment/contract/src/area.rs")
            ),
        ]
    );
    assert_contains(&findings[0].fix, "add `order-contract`");
}

#[test]
fn an_app_signing_members_in_depends_on_a_member_area_no_module_routes_in() {
    let mut order = module("order", "modules/order");
    define_area(&mut order, "member");
    order.contract_declares_gen = true;
    order.contract_exports = vec!["pruned".to_owned()];
    let mut app = app_with(vec![order]);
    app.identity = Some(Identity::default());
    app.app
        .dependencies
        .retain(|dependency| dependency.package != "order-contract");

    let findings = check(&app);

    assert_eq!(rule_lines(&findings), [("app-dependency", 7)]);
    assert_eq!(
        findings[0].file,
        Path::new("modules/order/contract/src/area.rs")
    );
    assert_contains(&findings[0].fix, "add `order-contract`");
}

#[test]
fn two_areas_serving_under_one_prefix_are_a_finding() {
    let mut order = module("order", "modules/order");
    define_area(&mut order, "member");
    order.defined_areas[0].prefix = Some("/docs".to_owned());
    order.contract_declares_gen = true;
    order.contract_exports = vec!["pruned".to_owned()];
    let mut payment = module("payment", "modules/payment");
    define_area(&mut payment, "payer");
    payment.defined_areas[0].prefix = Some("/docs".to_owned());
    payment.contract_declares_gen = true;
    payment.contract_exports = vec!["pruned".to_owned()];

    let findings = check(&app_with(vec![order, payment]));

    assert_eq!(rule_lines(&findings), [("area-prefix", 7)]);
    assert_eq!(
        findings[0].file,
        Path::new("modules/payment/contract/src/area.rs")
    );
    assert_contains(
        &findings[0].message,
        "area `payer` serves under `/docs`, as area `member` does",
    );
}

fn replacing(module: &mut Module, built_in: &str, name: &str) {
    let file = module
        .krate
        .path
        .join(format!("src/areas/web/screens/{built_in}/{name}.html"));
    module.areas[0].overrides.push(ScreenOverride {
        module: built_in.to_owned(),
        file,
    });
}

#[test]
fn a_screen_folder_replaces_only_a_built_in_screen_of_a_crate_the_app_uses() {
    let mut order = module_with_routes("order", "web", Vec::new());
    replacing(&mut order, "sofie_identity", "sign_in");
    replacing(&mut order, "sofie_identity", "profile");
    let without = app_with(vec![order]);
    let mut with_identity = without.clone();
    with_identity.identity = Some(Identity {
        screens: vec!["sofie_identity/web/sign_in".to_owned()],
        ..Identity::default()
    });

    let unknown = check(&with_identity);
    let unused = check(&without);

    assert_eq!(rule_lines(&unknown), [("screen-override", 1)]);
    assert_contains(&unknown[0].message, "`sofie_identity/web/profile`");
    assert_eq!(
        rule_lines(&unused),
        [("screen-override", 1), ("screen-override", 1)]
    );
}

#[test]
fn every_embedded_file_has_a_path_from_the_app() {
    let mut order = module("order", "/elsewhere/order");
    order.migrations = vec![PathBuf::from("/elsewhere/order/migrations/0001_a.sql")];
    let mut app = app_with(vec![order]);
    app.app.path = PathBuf::from("../app");

    let findings = check(&app);

    assert_eq!(rule_lines(&findings), [("embedded-path", 1)]);
}

fn crate_at(fixture: &Fixture, package: &str, source: &str, tests: &str) -> sofie_model::Crate {
    let mut krate = krate(package, fixture.root().join(package));
    let source = fixture.write(&format!("{package}/src/lib.rs"), source);
    let tests = fixture.write(&format!("{package}/tests/a.rs"), tests);
    krate.sources = vec![Source::read(&source).expect("the fixture source parses")];
    krate.tests_and_examples = vec![Source::read(&tests).expect("the fixture test parses")];
    krate
}

#[test]
fn dead_code_is_named_nowhere_but_its_definition_a_re_export_or_a_doc() {
    let fixture = Fixture::new();
    let mut library = crate_at(
        &fixture,
        "library",
        "pub struct Used;\npub struct Reexported;\n/// Not `Documented`.\npub struct Documented;\n\
         pub struct Tested;\npub struct Valued;\nimpl Valued { pub fn try_new() {} }\n\
         impl Clone for Used { fn clone(&self) -> Self { Self } }\nfn f() { renamed::go(); }\n",
        "fn t(_: library::Tested) {}\n",
    );
    fixture.write(
        "library/Cargo.toml",
        "[package]\n\n[dependencies]\nunused = \"1\"\nrenamed = { package = \"real\" }\n",
    );
    library.dependencies = vec![
        dependency("unused"),
        Dependency {
            package: "real".to_owned(),
            crate_name: "renamed".to_owned(),
        },
    ];
    let mut user = crate_at(
        &fixture,
        "user",
        "pub use library::Reexported;\nfn f(_: library::Used, _: library::Valued) {}\n",
        "",
    );
    user.dependencies = vec![dependency("library")];
    let derives = crate_at(
        &fixture,
        "derives",
        "fn g() { quote!(Self::try_new(inner)) }\n",
        "",
    );
    let allowed = AllowList::read(fixture.root()).unwrap();

    let findings = check_dead_code(&[&library, &user], &[&derives], &allowed, 0.5).unwrap();

    let found: Vec<(&str, usize)> = findings
        .iter()
        .map(|finding| (finding.rule.name(), finding.line))
        .collect();
    assert_eq!(
        found,
        [
            ("unused-item", 2),
            ("unused-item", 4),
            ("unused-dependency", 4),
        ]
    );
}

#[test]
fn the_unused_share_counts_from_the_size_where_the_ceiling_allows_one_item() {
    let fixture = Fixture::new();
    let items = |count: usize| -> String {
        (0..count)
            .map(|index| format!("pub struct Item{index};"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let uses = |count: usize| -> String {
        (1..count)
            .map(|index| format!("fn f{index}(_: Item{index}) {{}}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let small = crate_at(&fixture, "small", &items(4), &uses(4));
    let large = crate_at(
        &fixture,
        "large",
        &format!("{}\npub struct Spare;\n", items(50)),
        &uses(50),
    );
    fixture.write(
        "dead-code-allow.toml",
        &format!(
            "{}{}{}",
            AllowList::item_entry("small", "Item0", "scaffolded"),
            AllowList::item_entry("large", "Item0", "kept"),
            AllowList::item_entry("large", "Spare", "kept")
        ),
    );
    let allowed = AllowList::read(fixture.root()).unwrap();

    let findings = check_dead_code(&[&small, &large], &[], &allowed, 0.02).unwrap();

    let found: Vec<(&str, String)> = findings
        .iter()
        .map(|finding| (finding.rule.name(), finding.message.clone()))
        .collect();
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, "unused-ratio");
    assert!(found[0].1.contains("of `large`"), "{found:?}");
}

fn with_tier(mut module: Module, tier: Tier) -> Module {
    if let Some(manifest) = module.manifest.as_mut() {
        manifest.tier = tier;
    }
    module
}

fn with_contract(fixture: &Fixture, mut module: Module, manifest: &str) -> Module {
    let package = module.krate.package.clone();
    fixture.write(&format!("modules/{package}/contract/Cargo.toml"), manifest);
    let mut contract = krate(
        &format!("{package}-contract"),
        fixture.root().join(format!("modules/{package}/contract")),
    );
    contract.dependencies = manifest_dependencies(manifest);
    module.contract = Some(contract);
    module
}

#[test]
fn a_contract_depends_on_no_other_module_not_even_on_its_contract() {
    let fixture = Fixture::new();
    let payment = fixture.module("payment", "[dependencies]\nsofie = \"0\"\n", "");
    let payment = with_contract(&fixture, payment, "[dependencies]\nsofie = \"0\"\n");
    let order = fixture.module("order", "[dependencies]\nsofie = \"0\"\n", "");
    let order = with_contract(
        &fixture,
        order,
        "[dependencies]\nsofie = \"0\"\nsofie-identity = \"0\"\npayment-contract = { path = \"../../payment/contract\" }\n",
    );

    let mut app = fixture.app(vec![order, payment]);
    app.identity = Some(sofie_model::Identity::default());

    let findings = check(&app);

    assert_eq!(rule_lines(&findings), [("module-dependency", 4)]);
    assert_contains(
        &findings[0].message,
        "contract `order-contract` depends on `payment-contract` of module `payment`",
    );
}

#[test]
fn dependencies_flow_down_the_tiers_and_never_to_the_app() {
    let fixture = Fixture::new();
    let order = fixture.module(
        "order",
        "[dependencies]\ncheckout-contract = { path = \"../checkout/contract\" }\nshop = { path = \"../../app\" }\n",
        "",
    );
    let order = with_contract(&fixture, order, "[dependencies]\n");
    let checkout = fixture.module(
        "checkout",
        "[dependencies]\norder-contract = { path = \"../order/contract\" }\n",
        "",
    );
    let checkout = with_contract(
        &fixture,
        with_tier(checkout, Tier::Experience),
        "[dependencies]\n",
    );
    let files_module = fixture.module(
        "files",
        "[dependencies]\norder-contract = { path = \"../order/contract\" }\n",
        "",
    );

    let findings = check(&fixture.app(vec![
        order,
        checkout,
        with_tier(files_module, Tier::Platform),
    ]));

    assert_eq!(
        rule_lines(&findings),
        [
            ("tier-direction", 2),
            ("tier-direction", 2),
            ("tier-direction", 3)
        ]
    );
    let messages: Vec<&str> = findings
        .iter()
        .map(|finding| finding.message.as_str())
        .collect();
    assert!(
        messages[0].starts_with(
            "module `files` (Platform) depends on `order-contract` of module `order` (Core)"
        ),
        "{messages:?}"
    );
    assert!(
        messages[1].starts_with(
            "module `order` (Core) depends on `checkout-contract` of module `checkout` (Experience)"
        ),
        "{messages:?}"
    );
    assert!(
        messages[2].starts_with("`order` depends on the app `shop`"),
        "{messages:?}"
    );
}

#[test]
fn a_platform_module_listens_only_to_platform_events() {
    let mut order = module("order", "modules/order");
    order.events = vec!["OrderPlaced".to_owned(), "Sent".to_owned()];
    order.contract = Some(krate("order-contract", "modules/order/contract"));
    let mut mail = with_tier(module("mail", "modules/mail"), Tier::Platform);
    mail.events = vec!["MailBounced".to_owned(), "Sent".to_owned()];
    mail.listeners = vec![
        Listener {
            event: vec!["order_contract".to_owned(), "OrderPlaced".to_owned()],
            ..listener("OrderPlaced", 4)
        },
        listener_named("bounce", "MailBounced", 5),
        listener_named("sent", "Sent", 7),
    ];
    let mut audit = module("audit", "modules/audit");
    audit.listeners = vec![listener("OrderPlaced", 6)];

    let findings = check(&app_with(vec![order, mail, audit]));

    assert_eq!(rule_lines(&findings), [("platform-listener", 4)]);
}

#[test]
fn every_module_declares_a_manifest_under_its_own_name() {
    let mut missing = module("order", "modules/order");
    missing.manifest = None;
    let mut misnamed = module("stock-level", "modules/stock-level");
    misnamed.manifest.as_mut().unwrap().name = "stock-level".to_owned();
    let mut undeclared = module("payment", "modules/payment");
    undeclared.manifest.as_mut().unwrap().declared = false;

    let findings = check(&app_with(vec![missing, misnamed, undeclared]));

    assert_eq!(
        rule_lines(&findings),
        [
            ("module-manifest", 1),
            ("module-manifest", 3),
            ("module-manifest", 3)
        ]
    );
    assert_eq!(
        files(&findings),
        [
            Path::new("modules/order/Cargo.toml"),
            Path::new("modules/payment/src/module.rs"),
            Path::new("modules/stock-level/src/module.rs"),
        ]
    );
    assert_contains(&findings[2].fix, "name: \"stock_level\"");
}

#[test]
fn a_setting_declared_twice_and_an_unreachable_config_are_found() {
    let mut order = module("order", "modules/order");
    order.manifest.as_mut().unwrap().settings = ["free_shipping_above", "banner", "banner"]
        .map(str::to_owned)
        .to_vec();
    order.config = Some(ModuleConfig {
        type_name: "OrderConfig".to_owned(),
        file: PathBuf::from("modules/order/src/config.rs"),
        line: 4,
        declared: false,
        keys: Vec::new(),
    });

    let findings = check(&app_with(vec![order]));

    assert_eq!(
        rule_lines(&findings),
        [("module-manifest", 4), ("module-manifest", 3)]
    );
    assert_contains(&findings[1].message, "`banner`");
    assert_eq!(findings[0].fix, "add `pub mod config;` to src/lib.rs");
}

#[test]
fn reading_the_system_time_in_a_module_is_found() {
    let fixture = Fixture::new();
    let source = "use std::time::{Duration, Instant, SystemTime};\n\
                  fn a() { let _ = SystemTime::now(); }\n\
                  fn b() { let _ = std::time::Instant::now().elapsed(); }\n\
                  fn c() { let _ = jiff::Timestamp::now(); }\n\
                  fn d() { let _ = chrono::Utc::now(); }\n\
                  fn e(clock: sofie::core::Clock) { let _ = clock.now(); let _ = Duration::from_secs(1); }\n";
    let order = fixture.module("order", "[dependencies]\n", source);

    let findings = check(&fixture.app(vec![order]));

    assert_eq!(
        rule_lines(&findings),
        [
            ("system-time", 2),
            ("system-time", 3),
            ("system-time", 4),
            ("system-time", 5)
        ]
    );
    assert!(
        findings[0]
            .message
            .starts_with("`SystemTime::now` reads the system time")
    );
}

#[test]
fn a_member_area_left_to_sofie_gen_needs_a_home_and_a_gen_module() {
    let mut order = module("order", "modules/order");
    define_area(&mut order, "member");
    order.defined_areas[0].implements_member_area = false;
    let mut written = order.clone();
    written.defined_areas[0].implements_member_area = true;

    let findings = check(&app_with(vec![order]));

    assert_eq!(
        rule_lines(&findings),
        [("member-home", 7), ("undeclared-gen", 1)]
    );
    assert!(
        findings[0]
            .message
            .contains("routes no screen without path parameters"),
        "a literal `PREFIX` is declared, so only the screen is missing: {}",
        findings[0].message
    );
    assert_eq!(
        files(&findings),
        [
            Path::new("modules/order/contract/src/area.rs"),
            Path::new("modules/order/contract/src/lib.rs"),
        ]
    );
    assert_eq!(check(&app_with(vec![written])), []);
}

#[test]
fn structure_checks_comment_density_and_god_types_of_every_crate() {
    let fixture = Fixture::new();
    fixture.write("app/Cargo.toml", "[package]\nname = \"shop\"\n");
    fixture.write(
        "app/src/lib.rs",
        "// one\n// two\n// three\n// four\nfn main() {}\n",
    );
    let methods = (0..50)
        .map(|index| format!("    pub fn m{index}(&self) {{}}"))
        .collect::<Vec<_>>()
        .join("\n");
    fixture.write("modules/order/Cargo.toml", "[package]\nname = \"order\"\n");
    fixture.write(
        "modules/order/src/lib.rs",
        &format!("pub struct Order;\n\nimpl Order {{\n{methods}\n}}\n"),
    );

    let findings = check_structure(fixture.root()).unwrap();

    assert_eq!(
        rule_lines(&findings),
        [("comment-density", 1), ("god-type", 1)]
    );
    assert_eq!(
        files(&findings),
        [
            fixture.root().join("app/Cargo.toml"),
            fixture.root().join("modules/order/src/lib.rs"),
        ]
    );
    assert_contains(&findings[1].message, "order::Order score");
}

fn function(path: &str, output: &str, renders: &[(Option<&str>, usize)]) -> Function {
    Function {
        output: Some(type_name(&format!("Result<{output}>"))),
        renders: renders
            .iter()
            .map(|(screen, line)| ScreenRender {
                screen: screen.map(str::to_owned),
                line: *line,
            })
            .collect(),
        ..testing::function(path, "modules/order/src/handlers.rs", 1)
    }
}

fn order_row() -> ReadModel {
    ReadModel {
        name: "OrderRow".to_owned(),
        from: Some("Order".to_owned()),
        fields: vec![
            field("id", "Id", 1),
            field("status", "OrderStatus", 1),
            field("lines", "Vec<Line>", 1),
        ],
        file: PathBuf::from("modules/order/src/queries.rs"),
        line: 3,
    }
}

#[test]
fn screen_bindings_must_name_fields_of_the_loaders_read_model() {
    let fixture = Fixture::new();
    let routes = vec![
        screen("/orders", "orders", "queries::list_orders", 1),
        screen("/page", "page", "queries::page_orders", 2),
        screen("/totals", "totals", "queries::totals", 3),
    ];
    let orders = "<Table id=\"t\" of={data} columns=\"id\" as=\"order\">\n\
        <Action post=\"/o/{order.id}\" label=\"x\" when={order.status_to.PAID} />\n\
        </Table>\n{#each data as order}{order.lines}{order.totl}{/each}\n{data.id}\n";
    let page = "{data.has_next}{#each data.items as order}{order.status}{/each}\n{data.next}\n";
    let mut module = fixture.module_with_screens(
        routes,
        &[("orders", orders), ("page", page), ("totals", "{data.sum}")],
    );
    module.code.read_models.push(order_row());
    module.code.functions = vec![
        function("queries::list_orders", "Vec<OrderRow>", &[]),
        function("queries::page_orders", "Paginated<OrderRow>", &[]),
        function("queries::totals", "Totals", &[]),
    ];

    let findings = check(&fixture.app(vec![module]));

    assert_eq!(
        rule_lines(&findings),
        [
            ("screen-binding", 4),
            ("screen-binding", 5),
            ("screen-binding", 2),
            ("screen-binding", 1),
        ]
    );
    let messages: Vec<&str> = findings
        .iter()
        .map(|finding| finding.message.as_str())
        .collect();
    assert_contains(messages[0], "`data[…].totl`");
    assert_contains(messages[0], "its fields are id, status, lines");
    assert_contains(messages[1], "a list has no field `id`");
    assert_contains(messages[2], "a page has `items`");
    assert_contains(messages[3], "answers `Totals`, which is no read model");
}

#[test]
fn an_action_s_posted_fields_must_name_fields_of_the_row() {
    let fixture = Fixture::new();
    let routes = vec![screen("/orders", "orders", "queries::list_orders", 1)];
    let orders = "<Table id=\"t\" of={data} columns=\"id\" as=\"order\">\n\
        <Action post=\"/o/pick\" label=\"x\" with={{order: order.id}} />\n\
        <Action post=\"/o/pick\" label=\"x\" with={{order: order.nope}} />\n</Table>\n";
    let mut module = fixture.module_with_screens(routes, &[("orders", orders)]);
    module.code.read_models.push(order_row());
    module.code.functions = vec![function("queries::list_orders", "Vec<OrderRow>", &[])];

    let findings = check(&fixture.app(vec![module]));

    assert_eq!(
        rule_lines(&findings),
        [("screen-binding", 3)],
        "{findings:?}"
    );
    assert_contains(&findings[0].message, "`data[…].nope`");
}

/// A loader taking `Listing<T>` answers `Listed<T>`, which a `Table` pages, sorts and filters.
#[test]
fn a_listed_answer_binds_its_rows_and_page_fields() {
    let fixture = Fixture::new();
    let routes = vec![screen("/orders", "orders", "queries::list_orders", 1)];
    let orders = "<Table id=\"t\" of={data} columns=\"id status\" as=\"order\">\n\
        <Action post=\"/o/{order.id}\" label=\"x\" />\n</Table>\n\
        {data.sort}{data.has_next}{data.controls}{#each data.items as order}{order.status}{/each}\n{data.nope}\n";
    let mut module = fixture.module_with_screens(routes, &[("orders", orders)]);
    module.code.read_models.push(order_row());
    module.code.functions = vec![function("queries::list_orders", "Listed<OrderRow>", &[])];

    let findings = check(&fixture.app(vec![module]));

    assert_eq!(
        rule_lines(&findings),
        [("screen-binding", 5)],
        "{findings:?}"
    );
}

#[test]
fn bindings_reach_into_the_read_models_a_computed_read_model_holds() {
    let fixture = Fixture::new();
    let routes = vec![screen("/summary", "summary", "queries::summary", 1)];
    let summary = "{#each data.by_status as line}{line.status}{line.count}{/each}\n\
        {#each data.by_status as line}{line.cuont}{/each}\n{data.best.status}{data.best.total}\n";
    let mut module = fixture.module_with_screens(routes, &[("summary", summary)]);
    let model = |name: &str, fields: &[(&str, &str)]| ReadModel {
        name: name.to_owned(),
        from: None,
        fields: fields.iter().map(|(name, ty)| field(name, ty, 1)).collect(),
        file: PathBuf::from("modules/order/src/queries.rs"),
        line: 1,
    };
    module.code.read_models = vec![
        model(
            "Summary",
            &[
                ("by_status", "Vec<StatusCount>"),
                ("best", "Option<StatusCount>"),
            ],
        ),
        model(
            "StatusCount",
            &[("status", "OrderStatus"), ("count", "u64")],
        ),
    ];
    module.code.functions = vec![function("queries::summary", "Summary", &[])];

    let findings = check(&fixture.app(vec![module]));

    assert_eq!(
        rule_lines(&findings),
        [("screen-binding", 2), ("screen-binding", 3)]
    );
    assert_contains(
        &findings[0].message,
        "read model `StatusCount` has no field `cuont`",
    );
    assert_contains(&findings[1].message, "no field `total`");
}

#[test]
fn a_screen_without_bindings_still_needs_a_read_model() {
    let fixture = Fixture::new();
    let routes = vec![
        screen("/empty", "empty", "queries::empty", 1),
        screen("/rows", "rows", "queries::rows", 2),
        route(Method::Get, "/shown", "queries::shown", 3),
    ];
    let plain_section = "<Section title=\"Plain\">\n</Section>\n";
    let mut module =
        fixture.module_with_screens(routes, &[("empty", plain_section), ("rows", plain_section)]);
    module.code.read_models.push(order_row());
    module.code.functions = vec![
        function("queries::empty", "()", &[]),
        function("queries::rows", "OrderRow", &[]),
        function("queries::shown", "Screen<()>", &[(Some("empty"), 7)]),
    ];

    let findings = check(&fixture.app(vec![module]));

    assert_eq!(
        rule_lines(&findings),
        [("screen-binding", 1), ("screen-binding", 7)]
    );
    assert_contains(
        &findings[0].message,
        "loader `queries::empty` answers `()`, which is no read model",
    );
    assert_contains(&findings[1].message, "handler `queries::shown`");
}

#[test]
fn an_embedded_island_reads_the_props_its_screen_gives_it() {
    let fixture = Fixture::new();
    let routes = vec![screen("/orders/{order}", "order", "queries::show_order", 1)];
    let mut module = fixture.module_with_screens(
        routes,
        &[(
            "order",
            "<Island name=\"order-lines\" props={data.lines} />\n\
             <Island name=\"order-chart\" props={data} />",
        )],
    );
    module.code.read_models.push(order_row());
    module.code.functions = vec![function("queries::show_order", "OrderRow", &[])];
    let island = |name: &str, props: &[&str]| sofie_model::Island {
        name: name.to_owned(),
        file: PathBuf::from(format!("modules/order/src/areas/web/islands/{name}.svelte")),
        area: Some("web".to_owned()),
        props: Some(sofie_model::PropsDeclaration {
            names: props.iter().map(|&prop| prop.to_owned()).collect(),
            type_name: Some("OrderRow".to_owned()),
            line: 3,
        }),
    };
    module.islands = vec![
        island("order-lines", &["id"]),
        island("order-chart", &["status", "total"]),
    ];

    let findings = check(&fixture.app(vec![module]));
    let props: Vec<&Finding> = findings
        .iter()
        .filter(|finding| finding.rule == sofie_check::Rule::IslandProps)
        .collect();

    assert_eq!(props.len(), 2, "{findings:?}");
    assert_contains(&props[0].message, "`data.lines` is no read model");
    assert_contains(
        &props[1].message,
        "reads prop `total`, which read model `OrderRow` does not have",
    );
}

#[test]
fn a_rendered_screen_must_exist_and_bind_what_the_screen_data_holds() {
    let fixture = Fixture::new();
    let routes = vec![route(
        Method::Get,
        "/orders/{order}",
        "queries::show_order",
        1,
    )];
    let mut module = fixture.module_with_screens(routes, &[("order", "{data.id}{data.code}")]);
    module.code.read_models.push(order_row());
    module.code.functions = vec![function(
        "queries::show_order",
        "Screen<OrderRow>",
        &[(Some("order"), 4), (Some("missing"), 5), (None, 6)],
    )];

    let findings = check(&fixture.app(vec![module]));

    assert_eq!(
        rule_lines(&findings),
        [
            ("screen-binding", 1),
            ("missing-screen", 5),
            ("missing-screen", 6)
        ]
    );
    assert_contains(&findings[0].message, "`data.code`");
    assert_contains(&findings[2].message, "not a string literal");
}

#[test]
fn a_prefix_without_its_leading_slash_or_ending_in_one_is_a_finding() {
    for prefix in ["docs", "/docs/", "/"] {
        let mut order = module("order", "modules/order");
        define_area(&mut order, "member");
        order.defined_areas[0].prefix = Some(prefix.to_owned());
        order.contract_declares_gen = true;
        order.contract_exports = vec!["pruned".to_owned()];

        let findings = check(&app_with(vec![order]));

        assert_eq!(rule_lines(&findings), [("area-prefix", 7)], "{prefix}");
        assert_contains(&findings[0].message, &format!("`{prefix}`"));
    }
}

#[test]
fn an_area_at_the_root_collides_with_the_web_area() {
    let mut order = module("order", "modules/order");
    define_area(&mut order, "member");
    order.defined_areas[0].prefix = Some(String::new());
    order.contract_declares_gen = true;
    order.contract_exports = vec!["pruned".to_owned()];

    let findings = check(&app_with(vec![order]));

    assert_eq!(rule_lines(&findings), [("area-prefix", 7)]);
    assert_contains(
        &findings[0].message,
        "area `member` serves under the root, as area `web` does",
    );
}

#[test]
fn routes_need_one_name_each_parameters_a_builder_can_take_and_an_exported_contract() {
    let mut order = module_with_routes(
        "order",
        "member",
        vec![
            route(Method::Get, "/orders", "queries::orders", 1),
            route(Method::Get, "/orders/all", "queries::orders", 2),
            route(Method::Post, "/orders/{Order-Id}", "actions::cancel", 3),
        ],
    );
    define_area(&mut order, "member");
    order.defined_areas[0].prefix = None;

    let findings = check(&app_with(vec![order]));

    assert_eq!(
        rule_lines(&findings),
        [
            ("area-prefix", 7),
            ("undeclared-gen", 1),
            ("route-name", 1),
            ("route-name", 2),
            ("route-name", 3),
        ]
    );
    assert_contains(&findings[2].message, "named `order.member.orders`");
    assert_contains(&findings[4].message, "`{Order-Id}`");
}

#[test]
fn a_route_parameter_names_an_aggregate_or_a_field_of_one() {
    let mut order = module_with_routes(
        "order",
        "member",
        vec![
            route(Method::Get, "/orders/{order}", "queries::order", 1),
            route(Method::Get, "/codes/{code}", "queries::by_code", 2),
            route(Method::Get, "/files/{*path}", "queries::file", 3),
            route(Method::Get, "/pages/{page}", "queries::page", 4),
        ],
    );
    order.code.aggregates[0]
        .fields
        .push(field("code", "String", 2));
    define_area(&mut order, "member");

    let findings = check(&app_with(vec![order]));
    let unread: Vec<&Finding> = findings
        .iter()
        .filter(|finding| finding.message.contains("no handler can read"))
        .collect();

    assert_eq!(unread.len(), 1, "{findings:?}");
    assert_eq!(unread[0].line, 4);
    assert_contains(&unread[0].message, "`{page}`");
}

#[test]
fn a_contract_exports_its_permissions_once_an_area_admits_by_permission() {
    let mut order = module_with_routes(
        "order",
        "member",
        vec![route(
            Method::Post,
            "/orders/{order}",
            "actions::cancel_order",
            3,
        )],
    );
    define_area(&mut order, "member");
    order.defined_areas[0].principal = vec!["Permitted".to_owned()];
    order.contract_declares_gen = true;
    order.contract_exports = vec!["routes".to_owned(), "pruned".to_owned()];

    let findings = check(&app_with(vec![order.clone()]));
    order.contract_exports.push("permissions".to_owned());

    assert_eq!(rule_lines(&findings), [("undeclared-gen", 1)]);
    assert_contains(&findings[0].fix, "pub use r#gen::permissions;");
    assert_eq!(rule_lines(&check(&app_with(vec![order]))), []);
}

#[test]
fn a_contract_exports_the_change_event_of_an_exposed_aggregate() {
    let mut order = module("order", "modules/order");
    let mut aggregate = testing::aggregate("Order", Vec::new(), "modules/order/src/order.rs");
    aggregate.exposed = vec!["total".to_owned()];
    order.code.aggregates.push(aggregate);
    define_area(&mut order, "member");
    order.contract_declares_gen = true;
    order.contract_exports = vec!["pruned".to_owned(), "Order".to_owned()];

    let findings = check(&app_with(vec![order.clone()]));
    order.contract_exports.push("OrderChanged".to_owned());

    assert_eq!(rule_lines(&findings), [("undeclared-gen", 1)]);
    assert_contains(&findings[0].fix, "OrderChanged");
    assert_eq!(rule_lines(&check(&app_with(vec![order]))), []);
}

#[test]
fn a_contract_exports_the_pruned_events_of_its_module_s_aggregates() {
    let mut order = module("order", "modules/order");
    order.code.aggregates.push(testing::aggregate(
        "Order",
        Vec::new(),
        "modules/order/src/order.rs",
    ));
    define_area(&mut order, "member");
    order.contract_declares_gen = true;

    let findings = check(&app_with(vec![order.clone()]));
    order.contract_exports.push("pruned".to_owned());

    assert_eq!(rule_lines(&findings), [("undeclared-gen", 1)]);
    assert_contains(&findings[0].fix, "pub use r#gen::pruned;");
    assert_eq!(rule_lines(&check(&app_with(vec![order]))), []);
}

#[test]
fn a_queued_route_queues_a_job_from_an_area_owning_nothing() {
    let queued = |path: &str, handler: &str, line| {
        let mut route = route(Method::Post, path, handler, line);
        route.modifiers.push(sofie_model::Modifier::Queued);
        route
    };
    let mut order = module_with_routes(
        "order",
        "web",
        vec![
            queued("/exports", "actions::export", 1),
            queued("/others", "actions::other", 2),
        ],
    );
    let member = area(
        &order,
        "member",
        vec![queued("/mine", "actions::export", 3)],
    );
    order.areas.push(member);
    define_area(&mut order, "member");
    order.jobs = vec![sofie_model::Job {
        file: PathBuf::from("modules/order/src/actions.rs"),
        line: 9,
        function: vec!["actions".to_owned(), "export".to_owned()],
    }];

    let findings = check(&app_with(vec![order]));
    let queued: Vec<(usize, &str)> = findings
        .iter()
        .filter(|finding| finding.rule == sofie_check::Rule::QueuedRoute)
        .map(|finding| (finding.line, finding.message.as_str()))
        .collect();

    assert_eq!(queued.len(), 2, "{queued:?}");
    assert_eq!(queued[0].0, 3);
    assert_contains(queued[0].1, "job reaches every record");
    assert_eq!(queued[1].0, 2);
    assert_contains(queued[1].1, "which is no `#[job]`");
}

#[test]
fn raw_sql_is_for_queries_only() {
    let raw = |module_path: &str, line| RawSql {
        module_path: module_path.split("::").map(str::to_owned).collect(),
        file: PathBuf::from("modules/order/src/lib.rs"),
        line,
        text: None,
    };
    let mut order = module("order", "modules/order");
    order.code.raw_sql = vec![
        raw("queries", 2),
        raw("queries::report", 3),
        raw("actions", 4),
        raw("jobs", 5),
    ];

    let findings = check(&app_with(vec![order]));

    assert_eq!(
        rule_lines(&findings),
        [
            ("raw-sql-in-queries-only", 4),
            ("raw-sql-in-queries-only", 5)
        ]
    );
    assert!(
        findings[0]
            .message
            .starts_with("raw SQL is for queries only; actions write through aggregates")
    );
}

#[test]
fn raw_sql_in_code_reaches_only_its_own_module() {
    let mut order = module("order", "modules/order");
    order.code.raw_sql = vec![RawSql {
        module_path: vec!["queries".to_owned()],
        file: PathBuf::from("modules/order/src/queries.rs"),
        line: 7,
        text: Some((
            "SELECT o.id, p.name FROM \"order\" o\n\
             JOIN catalog.product p ON p.id = o.product\n\
             WHERE o.id IN (SELECT id FROM sofie.outbox)"
                .to_owned(),
            8,
        )),
    }];

    let findings = check(&app_with(vec![order]));

    assert_eq!(rule_lines(&findings), [("cross-module-sql", 9)]);
    assert_eq!(findings[0].file, Path::new("modules/order/src/queries.rs"));
    assert_contains(&findings[0].message, "`catalog.product`");
}

fn constrained(name: &str, ty: &str, constraints: &[&str], line: usize) -> FieldDeclaration {
    FieldDeclaration {
        constraints: constraints.iter().map(|name| (*name).to_owned()).collect(),
        ..field(name, ty, line)
    }
}

#[test]
fn input_derived_from_an_aggregate_keeps_what_the_database_holds() {
    let fixture = Fixture::new();
    let mut catalog = module("catalog", fixture.root().join("modules/catalog"));
    catalog.code.aggregates = vec![aggregate(
        "Product",
        vec![
            constrained("name", "String", &["required", "max"], 2),
            constrained("stock", "i64", &["range"], 3),
        ],
        "modules/catalog/src/product.rs",
    )];
    catalog.code.inputs = vec![sofie_model::InputDeclaration {
        name: "Restock".to_owned(),
        path: vec!["actions".to_owned(), "Restock".to_owned()],
        from: Some("Product".to_owned()),
        fields: vec![
            constrained("name", "String", &["max"], 8),
            constrained("stock", "i64", &["range"], 9),
        ],
        unconstrained: Vec::new(),
        file: PathBuf::from("modules/catalog/src/actions.rs"),
        line: 7,
    }];

    let findings: Vec<Finding> = check(&fixture.app(vec![catalog]))
        .into_iter()
        .filter(|finding| finding.file.ends_with("actions.rs"))
        .collect();

    assert_eq!(
        rule_lines(&findings),
        [("schema-constraint", 8)],
        "{findings:#?}"
    );
    assert!(
        findings[0].message.starts_with(
            "`Restock.name` replaces the constraints it inherits from `Product` without `#[required]`"
        ),
        "{}",
        findings[0].message
    );
}

#[test]
fn constraints_and_enums_of_an_aggregate_need_their_database_rules() {
    let fixture = Fixture::new();
    let mut catalog = module("catalog", fixture.root().join("modules/catalog"));
    catalog.migrations = vec![fixture.write(
        "modules/catalog/migrations/0001_products.sql",
        "CREATE TABLE product_line (id uuid PRIMARY KEY, version bigint, created_at timestamptz, \
         updated_at timestamptz, name text CHECK (name <> ''), sku text, price bigint, status text, kind text CHECK (kind IN ('PLAIN')));\n\
         CREATE UNIQUE INDEX product_line_sku ON product_line (sku);",
    )];
    catalog.code.enums = vec![EnumDeclaration {
        name: "LineStatus".to_owned(),
        path: vec!["LineStatus".to_owned()],
        stored_names: vec!["OPEN".to_owned(), "CLOSED".to_owned()],
        variants: Vec::new(),
    }];
    catalog.code.enums.push(EnumDeclaration {
        name: "LineKind".to_owned(),
        path: vec!["LineKind".to_owned()],
        stored_names: vec!["PLAIN".to_owned(), "GIFT".to_owned()],
        variants: Vec::new(),
    });
    catalog.code.aggregates = vec![aggregate(
        "ProductLine",
        vec![
            constrained("name", "String", &["required", "max"], 2),
            constrained("sku", "Sku", &["unique", "length"], 3),
            constrained("price", "Money", &["min"], 4),
            constrained("status", "LineStatus", &[], 5),
            constrained("kind", "LineKind", &[], 6),
            constrained("code", "String", &["unique"], 7),
        ],
        "modules/catalog/src/product.rs",
    )];

    let findings = check(&fixture.app(vec![catalog]));

    assert_eq!(
        rule_lines(&findings),
        [
            ("schema-constraint", 3),
            ("schema-constraint", 4),
            ("schema-constraint", 5),
            ("schema-constraint", 6),
            ("schema-constraint", 7),
        ],
        "{findings:#?}"
    );
    assert_contains(&findings[3].message, "'GIFT'");
}

#[test]
fn an_unreadable_migration_is_reported_once() {
    let fixture = Fixture::new();
    let mut catalog = module("catalog", fixture.root().join("modules/catalog"));
    let migration = fixture.write("modules/catalog/migrations/0001_products.sql", "");
    std::fs::write(&migration, [0xff, 0xfe]).expect("the fixture migration is written");
    catalog.migrations = vec![migration.clone()];
    catalog.code.aggregates = vec![aggregate(
        "Product",
        Vec::new(),
        "modules/catalog/src/product.rs",
    )];

    let findings = check(&fixture.app(vec![catalog]));

    let unreadable: Vec<&Finding> = findings
        .iter()
        .filter(|finding| finding.file == migration)
        .collect();
    assert_eq!(unreadable.len(), 1, "{findings:#?}");
    assert_eq!(unreadable[0].rule.name(), "migration-lock");
}

#[test]
fn a_behavior_s_columns_must_be_in_the_migrations() {
    let fixture = Fixture::new();
    let mut catalog = module("catalog", fixture.root().join("modules/catalog"));
    catalog.migrations = vec![
        fixture.write(
            "modules/catalog/migrations/0001_products.sql",
            "CREATE TABLE product (id uuid PRIMARY KEY, version bigint NOT NULL);",
        ),
        fixture.write(
            "modules/catalog/migrations/0002_timestamps.sql",
            "ALTER TABLE product ADD COLUMN created_at timestamptz;",
        ),
    ];
    catalog.code.aggregates = vec![Aggregate {
        behaviors: vec!["SoftDelete".to_owned()],
        ..aggregate("Product", Vec::new(), "modules/catalog/src/product.rs")
    }];

    let findings = check(&fixture.app(vec![catalog]));

    let messages: Vec<&str> = findings
        .iter()
        .filter(|finding| finding.rule.name() == "behavior-column")
        .map(|finding| finding.message.as_str())
        .collect();
    assert_eq!(messages.len(), 2, "{findings:#?}");
    assert!(messages[0].contains("`Timestamps`") && messages[0].contains("`updated_at`"));
    assert!(messages[1].contains("`SoftDelete`") && messages[1].contains("`deleted_at`"));
}

#[test]
fn every_framework_behavior_s_columns_are_checked_with_their_type() {
    let fixture = Fixture::new();
    let mut catalog = module("catalog", fixture.root().join("modules/catalog"));
    catalog.migrations = vec![fixture.write(
        "modules/catalog/migrations/0001_products.sql",
        "CREATE TABLE product (id uuid PRIMARY KEY, version bigint NOT NULL, \
         created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL);",
    )];
    catalog.code.aggregates = vec![Aggregate {
        behaviors: vec!["Audited".to_owned(), "Slugged".to_owned()],
        ..aggregate("Product", Vec::new(), "modules/catalog/src/product.rs")
    }];

    let findings = check(&fixture.app(vec![catalog]));

    let fixes: Vec<&str> = findings
        .iter()
        .filter(|finding| finding.rule.name() == "behavior-column")
        .map(|finding| finding.fix.as_str())
        .collect();
    assert_eq!(fixes.len(), 3, "{findings:#?}");
    assert!(fixes[0].contains("created_by uuid"), "{}", fixes[0]);
    assert!(fixes[1].contains("updated_by uuid"), "{}", fixes[1]);
    assert!(fixes[2].contains("slug text"), "{}", fixes[2]);
}

#[test]
fn exposed_fields_need_a_contract_that_names_and_exports_them() {
    let product = || Aggregate {
        exposed: vec!["name".to_owned(), "sku".to_owned()],
        ..aggregate(
            "Product",
            vec![
                constrained("name", "String", &[], 2),
                constrained("sku", "Sku", &[], 3),
            ],
            "modules/catalog/src/product.rs",
        )
    };
    let mut without_contract = module("catalog", "modules/catalog");
    without_contract.code.aggregates = vec![product()];
    let mut unexported = module("stock", "modules/stock");
    define_area(&mut unexported, "member");
    unexported.defined_areas.clear();
    unexported.code.aggregates = vec![Aggregate {
        exposed: vec!["name".to_owned()],
        ..product()
    }];

    let findings = check(&app_with(vec![without_contract, unexported]));

    assert_eq!(
        rule_lines(&findings),
        [
            ("exposed-field", 1),
            ("exposed-field", 3),
            ("undeclared-gen", 1)
        ],
        "{findings:?}"
    );
    assert_contains(
        &findings[2].message,
        "Product and ProductChanged of module `stock`",
    );
}

#[test]
fn a_mirror_copies_only_fields_its_source_exposes() {
    let entity =
        |name: &str, fields: Vec<FieldDeclaration>, mirror: Option<&str>, line| Aggregate {
            mirror: mirror.map(|mirror| mirror.split("::").map(str::to_owned).collect()),
            exposed: vec!["name".to_owned()],
            line,
            ..aggregate(name, fields, "modules/order/src/copies.rs")
        };
    let mut catalog = module("catalog", "modules/catalog");
    define_area(&mut catalog, "member");
    catalog.code.aggregates = vec![entity("Product", Vec::new(), None, 1)];
    catalog.code.aggregates[0].behaviors = vec!["SoftDelete".to_owned()];
    let mut order = module("order", "modules/order");
    let fields = vec![
        constrained("name", "String", &[], 3),
        constrained("price", "Money", &[], 4),
    ];
    order.code.aggregates = vec![
        entity("ProductCopy", fields, Some("catalog_contract::Product"), 2),
        entity("Lost", Vec::new(), Some("catalog_contract::Missing"), 7),
    ];

    let findings = check(&app_with(vec![catalog, order]));

    let mirrors: Vec<(&str, usize)> = rule_lines(&findings)
        .into_iter()
        .filter(|(rule, _)| *rule == "mirror-field")
        .collect();
    assert_eq!(
        mirrors,
        [
            ("mirror-field", 2),
            ("mirror-field", 4),
            ("mirror-field", 7)
        ]
    );
}

#[test]
fn a_get_that_writes_is_reported_at_its_handler() {
    let routes = vec![
        route(Method::Get, "/orders", "queries::list_orders", 1),
        route(Method::Get, "/queued", "queries::queue_visit", 2),
        route(Method::Get, "/latest", "queries::latest_order", 3),
        route(Method::Get, "/seen", "queries::mark_seen", 4),
        route(Method::Post, "/orders", "actions::place_order", 5),
    ];
    let mut order = module_with_routes("order", "web", routes);
    let handler = |path: &str, arguments: &[&str], output: &str, line: usize| Function {
        arguments: arguments
            .iter()
            .map(|argument| type_name(argument))
            .collect(),
        line,
        ..function(path, output, &[])
    };
    let mut mark_seen = handler("queries::mark_seen", &[], "()", 40);
    mark_seen.effects.queued.push(sofie_model::Queued {
        job: None,
        line: 42,
    });
    order
        .code
        .aggregates
        .push(aggregate("Order", Vec::new(), "modules/order/src/order.rs"));
    order.code.functions = vec![
        handler("queries::list_orders", &["Read"], "Vec<OrderRow>", 10),
        handler("queries::queue_visit", &["Jobs"], "()", 20),
        handler("queries::latest_order", &["Read"], "Order", 30),
        mark_seen,
        handler("actions::place_order", &["Jobs"], "Order", 50),
    ];

    let findings: Vec<Finding> = check(&app_with(vec![order]))
        .into_iter()
        .filter(|finding| finding.rule.name() == "writing-get")
        .collect();

    assert_eq!(
        rule_lines(&findings),
        [
            ("writing-get", 20),
            ("writing-get", 30),
            ("writing-get", 42)
        ]
    );
    assert_contains(&findings[0].message, "GET /queued");
    assert_contains(&findings[0].message, "takes `Jobs`");
    assert_contains(&findings[1].message, "returns `Order`");
    assert_contains(&findings[2].message, "queues a job");
    assert_eq!(
        files(&findings)[0],
        Path::new("modules/order/src/handlers.rs")
    );
}

#[test]
fn the_build_and_the_engine_accept_the_same_island_names() {
    for name in [
        "order-chart",
        "chart-2",
        "a-",
        "-chart",
        "chart",
        "Order-chart",
        "order_chart",
        "font-face",
        "missing-glyph",
        "",
    ] {
        assert_eq!(
            sofie_model::is_island_name(name),
            sofie_engine::islands::is_island_name(name),
            "{name:?}"
        );
    }
}

#[test]
fn a_remote_module_is_reached_only_through_contracts_that_cross_apps() {
    let contract_trait = |name: &str, stays_local: Option<&'static str>| ContractTrait {
        name: name.to_owned(),
        path: vec![name.to_owned()],
        methods: vec![ContractMethod {
            name: "total".to_owned(),
            stays_local,
        }],
        file: PathBuf::from("split/order/modules/order/contract/src/lib.rs"),
        line: 1,
    };
    let mut cart = module("cart", "modules/cart");
    let mut checkout = testing::function("queries::checkout", "modules/cart/src/queries.rs", 7);
    checkout.arguments = vec![type_name("Dep<dyn Ledger>")];
    let mut list = testing::function("queries::list", "modules/cart/src/queries.rs", 12);
    list.arguments = vec![type_name("Dep<dyn order_contract::Orders>")];
    cart.code.functions = vec![checkout, list];
    let mut app = app_with(vec![cart]);
    app.app.dependencies.push(dependency("order"));
    app.split.remote = vec![RemoteModule {
        name: "order".to_owned(),
        contract: krate("order-contract", "split/order/modules/order/contract"),
        defined_areas: Vec::new(),
        exports: Vec::new(),
        events: Vec::new(),
        traits: vec![
            contract_trait("Orders", None),
            contract_trait("Ledger", Some("it is not `async`")),
        ],
    }];

    let findings = check(&app);

    assert_eq!(
        rule_lines(&findings),
        [("remote-dependency", 1), ("remote-call", 7)]
    );
    assert_contains(&findings[1].message, "`total` it is not `async`");
}

#[test]
fn a_migration_names_no_other_module_s_tables() {
    let fixture = Fixture::new();
    let mut catalog = module("catalog", fixture.root().join("modules/catalog"));
    catalog.migrations = vec![fixture.write(
        "modules/catalog/migrations/0001_products.sql",
        "SELECT sofie.module_role('catalog');\n\
         CREATE TABLE brand (id uuid PRIMARY KEY);\n\
         CREATE TABLE catalog.product (id uuid PRIMARY KEY, brand uuid REFERENCES brand (id),\n\
         owner uuid REFERENCES sofie_identity.account (id));\n\
         CREATE VIEW ordered AS SELECT p.id FROM product p\n\
         JOIN \"order\".\"order\" o ON o.product = p.id;",
    )];
    let order = module("order", fixture.root().join("modules/order"));

    let findings = check(&fixture.app(vec![catalog, order]));

    assert_eq!(
        rule_lines(&findings),
        [("cross-module-sql", 4), ("cross-module-sql", 6)],
        "{findings:#?}"
    );
    assert_contains(&findings[1].message, "module `order`");
}

#[test]
fn a_client_area_needs_sofie_s_clients_feature() {
    let fixture = Fixture::new();
    fixture.write(
        "app/Cargo.toml",
        "[package]\nname = \"shop\"\n\n[dependencies]\nsofie = { workspace = true }\n",
    );
    let mut notes = module("notes", fixture.root().join("modules/notes"));
    define_area(&mut notes, "board");
    notes.defined_areas[0].client = sofie_model::AreaClient::Svelte;
    let mut app = fixture.app(vec![notes]);
    app.app.dependencies.push(Dependency {
        package: "sofie-framework".to_owned(),
        crate_name: "sofie".to_owned(),
    });
    let features = |names: &[&str]| {
        names
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>()
    };

    let findings = sofie_check::check_client_feature(&app, &features(&["engine"]));

    assert_eq!(rule_lines(&findings), [("client-feature", 5)]);
    assert_contains(&findings[0].message, "area `board`");
    assert_contains(&findings[0].fix, "add \"clients\"");
    let enabled = sofie_check::check_client_feature(&app, &features(&["engine", "clients"]));
    assert_eq!(rule_lines(&enabled), []);
    app.modules[0].defined_areas[0].client = sofie_model::AreaClient::Html;
    let html = sofie_check::check_client_feature(&app, &features(&["engine"]));
    assert_eq!(rule_lines(&html), []);
}

#[test]
fn tasks_need_an_app_that_runs_them() {
    let fixture = Fixture::new();
    let mut order = fixture.module("order", "[dependencies]\nsofie = \"0\"\n", "");
    order.tasks = vec![sofie_model::Task {
        file: fixture.root().join("modules/order/src/tasks.rs"),
        line: 3,
        function: vec!["tasks".to_owned(), "reindex".to_owned()],
    }];
    let mut app = fixture.app(vec![order]);
    let task_runner = |app: &App| -> Vec<Finding> {
        check(app)
            .into_iter()
            .filter(|finding| finding.rule.name() == "task-runner")
            .collect()
    };
    let plain = fixture.write(
        "app/src/main.rs",
        "fn main() { sofie::run(&[], app, worker).unwrap(); }\n",
    );
    app.app.sources = vec![Source::read(&plain).unwrap()];
    let unrun = task_runner(&app);
    let running = fixture.write(
        "app/src/main.rs",
        "fn main() { sofie::run_with_tasks(&[], app, worker, shop::tasks).unwrap(); }\n",
    );
    app.app.sources = vec![Source::read(&running).unwrap()];

    assert_eq!(rule_lines(&unrun), [("task-runner", 1)]);
    assert_contains(&unrun[0].message, "the `#[task]`s of order need");
    assert_eq!(task_runner(&app), []);
}

#[test]
fn a_module_depending_on_sofie_identity_needs_an_app_that_does() {
    let fixture = Fixture::new();
    let account = fixture.module(
        "account",
        "[dependencies]\nsofie = \"0\"\nsofie-identity = \"0\"\n",
        "",
    );

    let findings = check(&fixture.app(vec![account]));

    assert_eq!(rule_lines(&findings), [("app-dependency", 3)]);
    assert_contains(&findings[0].message, "wires no sign-in");
}

#[test]
fn a_mail_template_the_build_would_refuse_is_a_finding() {
    let fixture = Fixture::new();
    let mut order = module("order", fixture.root().join("modules/order"));
    order.mails = vec![
        sofie_model::MailTemplate {
            key: "order/receipt".to_owned(),
            file: fixture.write(
                "modules/order/src/mails/receipt.html",
                "<p>Booked: {service}</p>",
            ),
        },
        sofie_model::MailTemplate {
            key: "order/thanks".to_owned(),
            file: fixture.write(
                "modules/order/src/mails/thanks.html",
                "<p>Booked: {data.service}</p>",
            ),
        },
    ];

    let findings = check(&fixture.app(vec![order]));

    assert_eq!(
        rule_lines(&findings),
        [("mail-template", 1)],
        "{findings:#?}"
    );
    assert_eq!(
        findings[0].file,
        fixture.root().join("modules/order/src/mails/receipt.html")
    );
}

#[test]
fn a_contract_implementation_behind_a_private_module_is_found() {
    let mut order = module_with_routes("order", "customer", Vec::new());
    order.implementations = vec![sofie_model::Provision {
        contract: vec!["order_contract".to_owned(), "Orders".to_owned()],
        ty: vec!["contract".to_owned(), "OrderBook".to_owned()],
        unit: true,
        file: PathBuf::from("modules/order/src/contract.rs"),
        line: 3,
    }];
    order.private_items = vec![sofie_model::PrivateItem {
        path: vec!["contract".to_owned()],
        file: PathBuf::from("modules/order/src/lib.rs"),
        line: 2,
        crate_visible: false,
    }];

    let findings = check(&app_with(vec![order]));

    let visibility: Vec<&Finding> = findings
        .iter()
        .filter(|finding| finding.rule.name() == "visibility")
        .collect();
    assert_eq!(visibility.len(), 1, "{findings:#?}");
    assert_contains(&visibility[0].message, "`order::contract::OrderBook`");
    assert_contains(&visibility[0].message, "the app's wiring");
}

#[test]
fn a_handler_job_or_listener_reading_through_its_own_module_s_contract_is_reported() {
    let contract_trait = |name: &str| ContractTrait {
        name: name.to_owned(),
        path: vec![name.to_owned()],
        methods: Vec::new(),
        file: PathBuf::from("modules/scheduling/contract/src/lib.rs"),
        line: 1,
    };
    let routes = vec![
        route(Method::Post, "/bookings", "actions::book_slot", 1),
        route(Method::Post, "/quotes", "actions::quote", 2),
        route(Method::Post, "/plain", "actions::plain", 3),
    ];
    let mut scheduling = module_with_routes("scheduling", "customer", routes);
    scheduling.contract_traits = vec![contract_trait("Availability")];
    let handler = |path: &str, arguments: &[&str], line: usize| Function {
        arguments: arguments
            .iter()
            .map(|argument| type_name(argument))
            .collect(),
        line,
        ..function(path, "()", &[])
    };
    scheduling.code.functions = vec![
        handler("actions::book_slot", &["Read", "Dep<dyn Availability>"], 10),
        handler("actions::quote", &["Dep<dyn Pricing>"], 20),
        handler("actions::plain", &["Read"], 30),
        handler("jobs::remind", &["Dep<dyn Availability + Send>"], 40),
        handler("listeners::booked", &["Dep<dyn Availability>"], 50),
    ];
    scheduling.jobs.push(Job {
        file: PathBuf::from("modules/scheduling/src/jobs.rs"),
        line: 40,
        function: vec!["jobs".to_owned(), "remind".to_owned()],
    });
    scheduling.listeners.push(Listener {
        function: vec!["listeners".to_owned(), "booked".to_owned()],
        ..listener("BookingRequested", 50)
    });

    let findings: Vec<Finding> = check(&app_with(vec![scheduling]))
        .into_iter()
        .filter(|finding| finding.rule.name() == "own-contract")
        .collect();

    assert_eq!(
        rule_lines(&findings),
        [
            ("own-contract", 10),
            ("own-contract", 40),
            ("own-contract", 50)
        ]
    );
    assert_contains(
        &findings[0].message,
        "`actions::book_slot` takes `Dep<dyn Availability>`",
    );
    assert_contains(&findings[0].message, "skips the owner filter");
    assert_contains(&findings[0].fix, "`<Aggregate>::across(&read)`");
    assert_contains(&findings[0].fix, "`.guard(...)`");
}

#[test]
fn a_module_building_a_cross_owner_check_by_hand_is_reported() {
    let mut scheduling = module_with_routes("scheduling", "customer", Vec::new());
    let calling = |path: &str, call: &[&str], line: usize| Function {
        effects: sofie_model::Effects {
            calls: vec![sofie_model::Call {
                path: call.iter().map(|&part| part.to_owned()).collect(),
                line,
            }],
            ..sofie_model::Effects::default()
        },
        ..function(path, "()", &[])
    };
    scheduling.code.functions = vec![
        calling(
            "actions::peek",
            &["sofie", "db", "Across", "for_derived_across_only"],
            7,
        ),
        calling("actions::book", &["Booking", "across"], 9),
    ];

    let findings: Vec<Finding> = check(&app_with(vec![scheduling]))
        .into_iter()
        .filter(|finding| finding.rule.name() == "own-contract")
        .collect();

    assert_eq!(
        rule_lines(&findings),
        [("own-contract", 7)],
        "{findings:#?}"
    );
    assert_contains(&findings[0].message, "`actions::peek`");
    assert_contains(&findings[0].fix, "`<Aggregate>::across(&read)`");
}
