use serde_json::json;
use sofie_engine::{Components, Template, compile_template_with, load_template};

const ORDER_CARD: &str = "<p class=\"card\">{data.label}: {data.order.total}\
    {#each data.order.lines as line}<i>{line}</i>{/each}</p>";

fn components() -> Components {
    let mut components = Components::default();
    components.add("OrderCard", ORDER_CARD.to_owned()).unwrap();
    components
        .add(
            "Shelf",
            "<OrderCard order={data.first} label=\"First\" />".to_owned(),
        )
        .unwrap();
    components
}

#[test]
fn a_component_renders_with_its_props_inside_the_loops_around_it() {
    let source = "{#each data.orders as order, index}{index}\
        <OrderCard order={order} label=\"Total\" />{/each}";
    let compiled = compile_template_with("shop/web/orders", source, &components()).unwrap();
    let template = load_template("shop/web/orders", &compiled).unwrap();
    let data = json!({"orders": [
        {"total": 5, "lines": ["a", "<b>"]},
        {"total": 7, "lines": []},
    ]});

    assert_eq!(
        template.render(&data).unwrap(),
        "0<p class=\"card\">Total: 5<i>a</i><i>&lt;b&gt;</i></p>\
         1<p class=\"card\">Total: 7</p>"
    );
}

#[test]
fn a_component_may_write_another() {
    let template = Template::parse_with(
        "shop/web/shelf",
        "<Shelf first={data.top} />",
        &components(),
    )
    .unwrap();

    assert_eq!(
        template
            .render(&json!({"top": {"total": 3, "lines": []}}))
            .unwrap(),
        "<p class=\"card\">First: 3</p>"
    );
}

#[test]
fn a_component_is_given_every_prop_it_reads_and_no_children() {
    let refused = |source: &str| {
        Template::parse_with("shop/web/x", source, &components())
            .unwrap_err()
            .message
    };

    assert!(
        refused("<OrderCard order={data.o} />")
            .contains("reads `data.label`, but `<OrderCard>` is not given `label`")
    );
    assert!(refused("<OrderCard order=\"x\" label=\"y\" />").contains("its field `total`"));
    assert!(refused("<OrderCard order={data.o} label=\"y\"></OrderCard>").contains("no children"));
}

#[test]
fn a_component_name_is_new_pascal_case_and_given_once() {
    let mut components = components();
    let refused = |components: &mut Components, name: &str| {
        components
            .add(name, "<p></p>".to_owned())
            .unwrap_err()
            .message
    };

    assert!(refused(&mut components, "Table").contains("the catalog ships"));
    assert!(refused(&mut components, "order_card").contains("PascalCase"));
    assert!(refused(&mut components, "OrderCard").contains("added twice"));
}

#[test]
fn a_component_writing_itself_is_refused() {
    let mut components = Components::default();
    components.add("Loop", "<Loop />".to_owned()).unwrap();

    let error = Template::parse_with("shop/web/x", "<Loop />", &components).unwrap_err();

    assert!(error.message.contains("writes itself"), "{}", error.message);
}
