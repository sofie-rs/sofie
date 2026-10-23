use sofie_model::permissions::{module_permissions, refuse_colliding_constants, route_permission};
use sofie_model::testing::{TemporaryDirectory, manifest};
use sofie_model::{Modifier, Module, load};

const LIB: &str = "mod areas;\nmod module;\nmod listeners;\n\n\
    #[derive(Aggregate)]\npub struct Order { meta: Meta, total: Money, note: String }\n\n\
    #[derive(Aggregate)]\npub struct OrderLine { meta: Meta, total: Money }\n\n\
    #[derive(Aggregate)]\npub struct Refund { meta: Meta, total: Money }\n\n\
    #[derive(Aggregate)]\npub struct Payment { meta: Meta, total: Money }\n\n\
    #[derive(Aggregate)]\npub struct Status { meta: Meta, label: String }\n\n\
    pub async fn cancel_order(order: Order) -> Result<Order> { Ok(order) }\n\
    pub async fn list_orders(read: Read) -> Result<Vec<Order>> { Ok(Vec::new()) }\n\
    pub async fn order_invoice(order: Order) -> Result<()> { Ok(()) }\n\
    pub async fn cancel_order_line(line: OrderLine) -> Result<OrderLine> { Ok(line) }\n\
    pub async fn refund_payment(payment: Payment) -> Result<Payment> { Ok(payment) }\n\
    pub async fn update_status(status: Status) -> Result<Status> { Ok(status) }\n\
    pub async fn list_order_lines(read: Read) -> Result<Vec<OrderLine>> { Ok(Vec::new()) }\n\
    pub async fn read_statuses(status: Status) -> Result<Status> { Ok(status) }\n";

const ROUTES: &str = "use sofie::prelude::*;\n\npub fn routes(routes: &mut Routes<Console>) {\n    \
    routes.get(\"/orders\", crate::list_orders);\n    \
    routes.post(\"/orders/{order}/cancel\", crate::cancel_order).feature(crate::module::SPLIT);\n    \
    routes.get(\"/orders/{order}/invoice\", crate::order_invoice);\n    \
    routes.post(\"/lines/{line}/cancel\", crate::cancel_order_line);\n    \
    routes.post(\"/payments/{payment}/refund\", crate::refund_payment);\n    \
    routes.post(\"/statuses/{status}\", crate::update_status);\n    \
    routes.get(\"/lines\", crate::list_order_lines);\n    \
    routes.post(\"/statuses/{status}/read\", crate::read_statuses);\n}\n";

const MANIFEST: &str = "use sofie::flags::Feature;\n\n\
    pub const MODULE: sofie::Module = sofie::Module { name: \"order\", tier: sofie::Tier::Core, context: None, settings: &[] };\n\
    pub const SPLIT: Feature = Feature::new(\"order.split\").retire_after_days(14);\n";

const LISTENERS: &str = "use crate::module::SPLIT;\n\n\
    #[on]\n#[sofie::feature(SPLIT)]\npub async fn audit(event: OrderPlaced) -> Result<()> { Ok(()) }\n";

fn order_module() -> (TemporaryDirectory, Module) {
    module_with(LIB)
}

fn module_with(lib: &str) -> (TemporaryDirectory, Module) {
    let directory = TemporaryDirectory::new("sofie-model-permissions");
    directory.write(
        "Cargo.toml",
        "[workspace]\nresolver = \"3\"\nmembers = [\"app\", \"modules/*\"]\n",
    );
    directory.write(
        "app/Cargo.toml",
        &manifest("shop", "order = { path = \"../modules/order\" }\n"),
    );
    directory.write("app/src/lib.rs", "");
    directory.write("modules/order/Cargo.toml", &manifest("order", ""));
    directory.write("modules/order/src/lib.rs", lib);
    directory.write("modules/order/src/module.rs", MANIFEST);
    directory.write("modules/order/src/listeners.rs", LISTENERS);
    directory.write("modules/order/src/areas/mod.rs", "mod permitted;\n");
    directory.write("modules/order/src/areas/console/mod.rs", "mod routes;\n");
    directory.write("modules/order/src/areas/console/routes.rs", ROUTES);
    let app = load(directory.root()).unwrap();
    let module = app.modules.into_iter().next().unwrap();
    (directory, module)
}

#[test]
fn each_route_needs_its_verb_on_the_record_its_name_mentions() {
    let (_directory, module) = order_module();
    let routes = &module.areas[0].routes;
    let needed: Vec<String> = routes
        .iter()
        .map(|route| route_permission(&module, route))
        .collect();
    assert_eq!(
        needed,
        [
            "order.order.*.read",
            "order.order.*.cancel",
            "order.order.*.read",
            "order.order_line.*.cancel",
            "order.payment.*.refund",
            "order.status.*.update",
            "order.order_line.*.read",
            "order.status.*.update",
        ]
    );
    assert_eq!(
        routes[1].modifiers,
        [Modifier::Feature("crate::module::SPLIT".to_owned())]
    );
}

#[test]
fn a_module_grants_its_records_fields_and_actions() {
    let (_directory, module) = order_module();
    let permissions = module_permissions(&module);
    for expected in [
        "order.order.*.cancel",
        "order.order.*.create",
        "order.order.*.delete",
        "order.order.*.read",
        "order.order.*.update",
        "order.order.note.read",
        "order.order.total.update",
    ] {
        assert!(
            permissions.contains(&expected.to_owned()),
            "{expected}: {permissions:?}"
        );
    }
}

#[test]
fn two_permissions_writing_one_constant_are_refused() {
    let lib = LIB.replace("note: String }", "note: String, line_total: Money }");
    let (_directory, module) = module_with(&lib);

    let error = refuse_colliding_constants(&module).unwrap_err();

    assert!(
        error.message.contains("`ORDER_LINE_TOTAL_READ`"),
        "{}",
        error.message
    );
    let (_directory, module) = order_module();
    assert!(refuse_colliding_constants(&module).is_ok());
}

#[test]
fn a_gated_listener_finds_the_flag_its_module_declares() {
    let (_directory, module) = order_module();
    let features = &module.code.features;
    assert_eq!(features.declarations.len(), 1);
    assert_eq!(features.declarations[0].retire_after_days, 14);
    let gate = &features.gates[0];
    assert_eq!(gate.function, ["listeners", "audit"]);
    let declaration = features.declaration_of(gate).unwrap();
    assert_eq!(declaration.name.as_deref(), Some("order.split"));
}

#[test]
fn a_route_hides_behind_its_feature_and_a_listener_behind_its_gate() {
    let (_directory, module) = order_module();
    let routes = &module.areas[0].routes;
    let flag = |declaration: Option<&sofie_model::FeatureDeclaration>| {
        declaration.and_then(|declaration| declaration.name.clone())
    };

    assert_eq!(
        flag(module.route_feature(&routes[1])).as_deref(),
        Some("order.split")
    );
    assert_eq!(flag(module.route_feature(&routes[0])), None);
    let listener = ["listeners".to_owned(), "audit".to_owned()];
    assert_eq!(
        flag(module.code.features.gate_on(&listener)).as_deref(),
        Some("order.split")
    );
}
