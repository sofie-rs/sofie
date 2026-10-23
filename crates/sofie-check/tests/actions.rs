use std::path::PathBuf;

use sofie_check::{Finding, Rule, Screens};
use sofie_model::testing::{
    TemporaryDirectory, app, area, field, function, module, route, type_name,
};
use sofie_model::{InputDeclaration, Method, Route};

fn posting(path: &str, handler: &str) -> Route {
    route(Method::Post, path, handler, 2)
}

fn pick_input() -> InputDeclaration {
    InputDeclaration {
        name: "Pick".to_owned(),
        path: vec!["actions".to_owned(), "Pick".to_owned()],
        from: None,
        fields: vec![
            field("order", "OrderId", 3),
            field("note", "Option<String>", 4),
        ],
        unconstrained: Vec::new(),
        file: PathBuf::from("modules/order/src/actions.rs"),
        line: 2,
    }
}

fn action_findings(screen: &str) -> Vec<Finding> {
    let directory = TemporaryDirectory::new("sofie-check-actions");
    let routes = vec![
        posting("/orders/pick", "actions::pick"),
        posting("/orders/{order}/drop", "actions::drop"),
    ];
    let mut module = module("order", directory.path("modules/order"));
    let mut web = area(&module, "web", routes);
    web.screens = vec![directory.write("modules/order/src/areas/web/screens/orders.html", screen)];
    module.areas.push(web);
    let mut pick = function("actions::pick", "modules/order/src/actions.rs", 5);
    pick.arguments = vec![type_name("Pick")];
    module.code.functions = vec![
        pick,
        function("actions::drop", "modules/order/src/actions.rs", 9),
    ];
    module.code.inputs = vec![pick_input()];
    let app = app(directory.root(), vec![module]);
    sofie_check::check(&app, &Screens::read(&app.modules))
        .into_iter()
        .filter(|finding| finding.rule == Rule::ActionInput)
        .collect()
}

#[test]
fn an_action_posts_the_fields_its_route_s_input_requires_and_no_others() {
    let screen = "<Action post=\"/orders/pick\" label=\"x\" with={{order: data.id}} />\n\
        <Action post=\"/orders/pick\" label=\"x\" with={{order: data.id, note: 2, ordre: 1}} />\n\
        <Action post=\"/orders/pick\" label=\"x\" with={{note: 2}} />\n\
        <Action post=\"/orders/pick\" label=\"x\" />\n\
        <Action post=\"/orders/{data.id}/drop\" label=\"x\" with={{anything: 1}} />\n\
        <Action post=\"/elsewhere\" label=\"x\" with={{anything: 1}} />\n";

    let findings = action_findings(screen);

    let lines: Vec<usize> = findings.iter().map(|finding| finding.line).collect();
    assert_eq!(lines, [2, 3, 4], "{findings:#?}");
    assert!(
        findings[0]
            .message
            .contains("sends `ordre`, but its input `Pick` has no field"),
        "{}",
        findings[0].message
    );
    assert!(
        findings[1]
            .message
            .contains("does not send `order`, which its input `Pick` requires"),
        "{}",
        findings[1].message
    );
}
