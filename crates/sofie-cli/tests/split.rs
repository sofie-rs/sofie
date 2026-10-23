use std::fs;

use sofie_cli::{split_module, split_plan};
use sofie_model::testing::{TemporaryDirectory, manifest};

const MODULE: &str = "pub const MODULE: sofie::Module = sofie::Module {\n    name: \"NAME\",\n    \
                      tier: sofie::Tier::Core,\n    context: None,\n    settings: &[],\n    \
                      ..crate::wiring::GENERATED\n};\n";
const CONTRACT: &str = "mod r#gen;\n\npub use r#gen::routes;\n\n";
const LIBRARY: &str = "mod queries;\npub mod module;\nmod wiring;\nmod areas {\n    pub mod web {\n        \
                       pub mod routes;\n    }\n}\n";

fn shop() -> TemporaryDirectory {
    let directory = TemporaryDirectory::new("sofie-split");
    directory.write(
        "Cargo.toml",
        "[workspace]\nresolver = \"3\"\nmembers = [\"app\", \"modules/*\", \"modules/*/contract\"]\n\n\
         [workspace.dependencies]\nsofie = { path = \"sofie\" }\n",
    );
    directory.write(
        "app/Cargo.toml",
        &manifest(
            "shop",
            "sofie.workspace = true\norder = { path = \"../modules/order\" }\n\
             billing = { path = \"../modules/billing\" }\n",
        ),
    );
    directory.write(
        "sofie/Cargo.toml",
        &format!(
            "{}\n[features]\nremote-contracts = []\n",
            manifest("sofie", "")
        ),
    );
    directory.write("sofie/src/lib.rs", "");
    directory.write(
        "app/src/main.rs",
        "mod wiring;\n\nfn main() {\n    let _ = (wiring::MODULES, wiring::app, wiring::worker);\n}\n",
    );
    module(&directory, "billing", "", "/invoices");
    module(
        &directory,
        "order",
        "order-contract = { path = \"contract\" }\nbilling-contract = { path = \"../billing/contract\" }\n",
        "/orders",
    );
    directory.write(
        "modules/order/contract/src/lib.rs",
        &format!(
            "{CONTRACT}{}",
            "#[sofie::contract]\npub trait Orders: Send + Sync {\n    \
         async fn total(&self, order: sofie::db::Uuid) -> sofie::core::Result<i64>;\n}\n\n\
         #[sofie::contract]\npub trait Ledger: Send + Sync {\n    fn now(&self) -> i64;\n}\n\n\
         #[derive(sofie::Event)]\npub struct OrderPlaced {\n    pub order: sofie::db::Uuid,\n}\n"
        ),
    );
    directory
}

fn module(directory: &TemporaryDirectory, name: &str, dependencies: &str, path: &str) {
    directory.write(
        &format!("modules/{name}/Cargo.toml"),
        &manifest(name, dependencies),
    );
    directory.write(
        &format!("modules/{name}/contract/Cargo.toml"),
        &manifest(&format!("{name}-contract"), ""),
    );
    directory.write(&format!("modules/{name}/contract/src/lib.rs"), CONTRACT);
    directory.write(&format!("modules/{name}/src/lib.rs"), LIBRARY);
    directory.write(
        &format!("modules/{name}/src/module.rs"),
        &MODULE.replace("NAME", name),
    );
    directory.write(
        &format!("modules/{name}/src/queries.rs"),
        "pub fn list() {}\n\npub fn show() {}\n",
    );
    directory.write(
        &format!("modules/{name}/src/areas/web/routes.rs"),
        &format!(
            "use sofie::prelude::*;\n\npub fn routes(routes: &mut Routes<Web>) {{\n    \
             routes.get(\"{path}\", queries::list);\n    routes.get(\"{path}/{{*rest}}\", queries::show);\n}}\n"
        ),
    );
    directory.write(&format!("modules/{name}/migrations/0001_start.sql"), "");
}

fn read(directory: &TemporaryDirectory, relative: &str) -> String {
    fs::read_to_string(directory.path(relative)).unwrap()
}

#[test]
fn the_plan_names_what_moves_what_is_served_and_where_each_path_goes() {
    let shop = shop();

    let plan = split_plan(shop.root(), "order").unwrap().to_string();

    for expected in [
        "module `order` leaves `shop` for a new app `order-app`",
        "modules/order → split/order/modules/order",
        "contract order_contract::Orders",
        "event order.OrderPlaced",
        "Served back by shop\n  module billing",
        "method Ledger::now: it is not `async`",
        "/orders → order-app\n  anything else → shop",
        "shop: ORDER_REMOTE_BASE_URL",
        "order-app: BILLING_REMOTE_BASE_URL",
    ] {
        assert!(plan.contains(expected), "{expected}\n\n{plan}");
    }
    assert!(shop.path("modules/order").is_dir(), "a plan writes nothing");
}

#[test]
fn the_plan_names_the_app_s_tests_that_reach_into_the_moved_crate() {
    let shop = shop();
    shop.write(
        "app/tests/orders.rs",
        "use order::Order;\n\n#[test]\nfn placed() {\n    let _ = std::any::type_name::<Order>();\n}\n",
    );
    shop.write(
        "app/tests/support/mod.rs",
        "pub fn order() -> i64 {\n    1\n}\n",
    );

    let plan = split_plan(shop.root(), "order").unwrap();

    let orders = fs::canonicalize(shop.path("app/tests/orders.rs")).unwrap();
    assert_eq!(plan.orphaned_tests, [orders]);
    let shown = plan.to_string();
    assert!(
        shown.contains(
            "Tests naming `order`, which shop no longer depends on: move them to order-app \
             or reach the module through `order-contract`\n  test app/tests/orders.rs\n"
        ),
        "{shown}"
    );
}

#[test]
fn write_moves_the_module_and_wires_both_apps_to_each_other() {
    let shop = shop();

    let split = split_module(shop.root(), "order").unwrap();

    assert!(!shop.path("modules/order").exists());
    assert!(
        shop.path("split/order/modules/order/migrations/0001_start.sql")
            .is_file()
    );
    let moved = read(&shop, "split/order/modules/order/Cargo.toml");
    assert!(
        moved.contains("billing-contract = { path = \"../../../../modules/billing/contract\" }"),
        "{moved}"
    );
    assert!(
        moved.contains("order-contract = { path = \"contract\" }"),
        "{moved}"
    );
    let left = read(&shop, "app/Cargo.toml");
    assert!(!left.contains("order = {"), "{left}");
    assert!(
        left.contains("order-contract = { path = \"../split/order/modules/order/contract\" }"),
        "{left}"
    );
    assert!(
        left.contains("remote = [\"order\"]") && left.contains("serves = [\"billing\"]"),
        "{left}"
    );
    assert!(left.contains("features = [\"remote-contracts\"]"), "{left}");
    let workspace = read(&shop, "Cargo.toml");
    assert!(
        workspace.contains("\"split/order/app\", \"split/order/modules/*\""),
        "{workspace}"
    );
    assert_eq!(
        read(&shop, "proxy.toml")
            .lines()
            .skip(3)
            .collect::<Vec<_>>(),
        [
            "default = \"shop\"",
            "",
            "[prefixes]",
            "\"/orders\" = \"order-app\""
        ]
    );
    let served = read(&shop, "split/order/app/src/wiring.rs");
    assert!(
        served.contains(".contract::<dyn order_contract::Orders>()"),
        "{served}"
    );
    assert!(!served.contains("Ledger"), "{served}");
    assert!(
        served.contains(".publish(\"order.OrderPlaced\")"),
        "{served}"
    );
    assert!(
        served.contains("sofie::remote::connect(\"billing\")?;"),
        "{served}"
    );
    let left_wiring = read(&shop, "app/src/wiring.rs");
    assert!(
        left_wiring.contains("sofie::remote::connect(\"order\")?;"),
        "{left_wiring}"
    );
    assert!(
        left_wiring.contains(".pull(sofie::remote::feed(\"order\"))"),
        "{left_wiring}"
    );
    assert!(
        !left_wiring.contains("order::module::MODULE"),
        "{left_wiring}"
    );
    assert_ne!(split.generated, []);
}

#[test]
fn splitting_the_last_module_takes_its_members_out_and_earlier_splits_follow_its_contract() {
    let shop = shop();
    split_module(shop.root(), "order").unwrap();

    split_module(shop.root(), "billing").unwrap();

    let workspace = read(&shop, "Cargo.toml");
    assert!(
        !workspace.contains("\"modules/*\"") && !workspace.contains("\"modules/*/contract\""),
        "{workspace}"
    );
    assert!(
        workspace.contains("\"split/billing/modules/*\""),
        "{workspace}"
    );
    let earlier = read(&shop, "split/order/app/Cargo.toml");
    assert!(
        earlier
            .contains("billing-contract = { path = \"../../billing/modules/billing/contract\" }"),
        "{earlier}"
    );
}
