mod common;

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sofie_engine::{
    CLIENT_SCRIPT, ENGINE_SCRIPT, NOTICE_TARGET, Page, RenderError, STYLESHEET, Submitted,
    Template, TemplateError, field_errors, form_fields, notice,
};
use sofie_values::{
    Constraint, Control, Email, FormField, FormFields, Input, Money, Part, Present, Secret,
};

fn template(source: &str) -> Template {
    common::loaded("test", source)
}

fn render(source: &str, data: &Value) -> String {
    template(source)
        .render(data)
        .unwrap_or_else(|error| panic!("{error}"))
}

fn fragment(source: &str, data: &Value, id: &str) -> Result<String, RenderError> {
    template(source).render_fragment(data, id)
}

fn parse_error(source: &str) -> TemplateError {
    Template::parse("orders", source).expect_err("the template is invalid")
}

fn assert_parse_error(source: &str, line: usize, column: usize, fragment_of_message: &str) {
    let error = parse_error(source);
    assert_eq!(
        (error.line, error.column),
        (line, column),
        "position of {error}"
    );
    assert!(
        error.message.contains(fragment_of_message),
        "`{}` should mention `{fragment_of_message}`",
        error.message
    );
}

#[test]
fn text_elements_and_attributes() {
    let html = render(
        "<div class=\"box\" title={data.title}><img src=\"/a.png\" /><br>x</div>",
        &json!({"title": "T"}),
    );
    assert_eq!(
        html,
        "<div class=\"box\" title=\"T\"><img src=\"/a.png\"><br>x</div>"
    );
}

#[test]
fn self_closing_non_void_elements_get_a_close_tag() {
    assert_eq!(render("<span />", &json!({})), "<span></span>");
}

#[test]
fn interpolation_of_paths_and_literals() {
    let data = json!({"order": {"status": "paid", "count": 3, "ratio": 2.0, "flag": true}});
    let html = render(
        "{data.order.status} {data.order.count} {data.order.ratio} {data.order.flag} {\"lit\"} {7}",
        &data,
    );
    assert_eq!(html, "paid 3 2 true lit 7");
}

#[test]
fn missing_paths_and_structures_render_empty() {
    let data = json!({"order": {"lines": [1, 2]}, "name": "x"});
    let html = render(
        "[{data.nope}][{data.name.inner}][{data.order}][{data.order.lines}][{data.order.lines.first}]",
        &data,
    );
    assert_eq!(html, "[][][][][]");
}

#[test]
fn if_else_follows_truthiness() {
    let source = "{#if data.value}yes{:else}no{/if}";
    for (value, expected) in [
        (json!("a"), "yes"),
        (json!(""), "no"),
        (json!(0), "no"),
        (json!(2), "yes"),
        (json!([]), "no"),
        (json!([0]), "yes"),
        (json!({}), "no"),
        (json!(null), "no"),
        (json!(true), "yes"),
    ] {
        assert_eq!(
            render(source, &json!({"value": value})),
            expected,
            "{value}"
        );
    }
    assert_eq!(render(source, &json!({})), "no");
    assert_eq!(render("{#if data.x}only{/if}", &json!({})), "");
}

#[test]
fn each_binds_items_and_index() {
    let data = json!({"items": [{"name": "a"}, {"name": "b"}]});
    let html = render(
        "{#each data.items as item, position}<i>{position}:{item.name}</i>{/each}",
        &data,
    );
    assert_eq!(html, "<i>0:a</i><i>1:b</i>");
}

#[test]
fn nested_each_resolves_the_innermost_name() {
    let data = json!({"groups": [{"rows": [{"v": 1}, {"v": 2}], "v": 9}]});
    let html = render(
        "{#each data.groups as row}{#each row.rows as row}{row.v}{/each}{row.v}{/each}",
        &data,
    );
    assert_eq!(html, "129");
}

#[test]
fn each_over_a_non_array_renders_nothing() {
    let source = "{#each data.items as item}x{/each}";
    assert_eq!(render(source, &json!({"items": {"a": 1}})), "");
    assert_eq!(render(source, &json!({})), "");
}

#[test]
fn comments_are_dropped() {
    assert_eq!(
        render("<!-- screens/orders.html -->\n<p>a</p>", &json!({})),
        "\n<p>a</p>"
    );
}

#[test]
fn parse_errors_carry_their_position() {
    assert_parse_error("<div>\n  <p>", 2, 3, "`<p>` is never closed");
    assert_parse_error("<div>\n</span>", 2, 1, "does not close `<div>`");
    assert_parse_error("\n  <Carousel />", 2, 3, "unknown component `Carousel`");
    assert_parse_error(
        "<Carousel />",
        1,
        1,
        "the catalog has Section, Table, Money, Action, Form, Field, Submit",
    );
    assert_parse_error("<p>{data.a + 1}</p>", 1, 12, "expected `}`");
    assert_parse_error("{orders}", 1, 2, "unknown name `orders`");
    assert_parse_error("{data.total()}", 1, 12, "found `(`");
    assert_parse_error("{}", 1, 2, "expected an expression");
    assert_parse_error("a\n{#for x}{/for}", 2, 1, "unknown block `{#for}`");
    assert_parse_error("{#if data.a}x", 1, 1, "`{#if}` is never closed");
    assert_parse_error("{#if data.a}{/each}", 1, 13, "does not close `{#if}`");
    assert_parse_error("{:else}", 1, 1, "closes nothing");
    assert_parse_error("{#each data.a item}{/each}", 1, 15, "expected `as`");
    assert_parse_error("{#each data.a as data}{/each}", 1, 18, "root binding");
    assert_parse_error(
        "{#each data.a as item}{/each}{item}",
        1,
        31,
        "unknown name `item`",
    );
    assert_parse_error("<div class=\"a\" class=\"b\">", 1, 16, "given twice");
    assert_parse_error("<div class=a>", 1, 12, "expected `\"`, `'` or `{`");
    assert_parse_error("<div class=\"a>", 1, 12, "never closed");
    assert_parse_error("<div", 1, 1, "never ended");
    assert_parse_error("a < b", 1, 3, "expected a tag name");
    assert_parse_error("{\"open}", 1, 2, "unclosed string");
}

#[test]
fn parse_errors_display_template_line_and_column() {
    let error = parse_error("\n\n   </p>");
    assert_eq!(
        error.to_string(),
        "orders:3:4: `</p>` closes nothing that is open"
    );
}

#[test]
fn htmx_scripts_and_handlers_are_parse_errors() {
    assert_parse_error(
        "<button hx-post=\"/x\">",
        1,
        9,
        "use a component such as `Action`",
    );
    assert_parse_error("<div data-hx-get=\"/x\">", 1, 6, "htmx");
    assert_parse_error("<a onclick={data.x}>", 1, 4, "event handler");
    assert_parse_error("<a OnClick=\"x\">", 1, 4, "event handler");
    for tag in [
        "script", "style", "meta", "base", "link", "iframe", "object", "embed",
    ] {
        assert_parse_error(
            &format!("<p>\n  <{tag} id=\"x\"></{tag}></p>"),
            2,
            3,
            &format!("`<{tag}>` is not allowed in templates"),
        );
    }
}

#[test]
fn inline_styles_are_parse_errors_naming_the_template_and_line() {
    let error = parse_error("<p>\n<div style=\"color: red\"></div></p>");
    assert!(
        error
            .to_string()
            .starts_with("orders:2:6: `style` is not allowed"),
        "{error}"
    );
}

#[test]
fn the_first_unknown_prop_in_source_order_is_reported() {
    assert_parse_error(
        "<Money value={data.p} first=\"1\" second=\"2\" />",
        1,
        23,
        "no prop `first`",
    );
}

#[test]
fn component_prop_errors() {
    assert_parse_error("<Table of={data.rows} columns=\"a\" />", 1, 1, "`id` prop");
    assert_parse_error(
        "<Table id=\"t\" of={data.rows} columns={data.c} />",
        1,
        30,
        "quoted literal",
    );
    assert_parse_error(
        "<Table id=\"t\" of={data.rows} columns=\"a-b\" />",
        1,
        30,
        "not a field name",
    );
    assert_parse_error(
        "<Money value={data.p} size=\"2\" />",
        1,
        23,
        "no prop `size`",
    );
    assert_parse_error("<Money value={data.p}>x</Money>", 1, 1, "takes no children");
    assert_parse_error(
        "<Action post=\"/x\" label=\"Go\" target=\"a b\" />",
        1,
        30,
        "not an element id",
    );
    assert_parse_error("<Section>x</Section>", 1, 1, "`title` prop");
}

#[test]
fn interpolated_text_is_escaped() {
    let html = render(
        "<p>{data.text}</p>",
        &json!({"text": "<script>alert('x')</script> & \"q\""}),
    );
    assert_eq!(
        html,
        "<p>&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt; &amp; &quot;q&quot;</p>"
    );
}

#[test]
fn bound_attribute_values_are_escaped() {
    let html = render(
        "<div title={data.text}></div>",
        &json!({"text": "\" onmouseover=\"alert(1)"}),
    );
    assert_eq!(
        html,
        "<div title=\"&quot; onmouseover=&quot;alert(1)\"></div>"
    );
}

#[test]
fn authored_markup_is_kept_as_written_in_text_attributes_and_props() {
    let html = render(
        "<p class=\"a&b\" title=\"Fish &amp; chips {data.n}\">Fish &amp; chips</p>\
         <Section title=\"A &amp; B\"></Section>",
        &json!({"n": "<1>"}),
    );
    assert_eq!(
        html,
        "<p class=\"a&b\" title=\"Fish &amp; chips &lt;1&gt;\">Fish &amp; chips</p>\
         <section><h2>A &amp; B</h2></section>"
    );
}

#[test]
fn braced_literals_are_data_and_escaped() {
    assert_eq!(
        render("<p title={\"a\\\"b\"}>{\"<b>\"}</p>", &json!({})),
        "<p title=\"a&quot;b\">&lt;b&gt;</p>"
    );
}

#[test]
fn bound_urls_never_render_script_or_data_schemes() {
    let source = "<a href={data.url}>x</a><img src={data.url}>";
    for url in [
        "javascript:alert(1)",
        "  JAVASCRIPT:alert(1)",
        "java\nscript:alert(1)",
        "data:text/html;base64,PHNjcmlwdD4=",
    ] {
        assert_eq!(
            render(source, &json!({"url": url})),
            "<a href=\"#\">x</a><img src=\"#\">",
            "{url:?}"
        );
    }
    assert_eq!(
        render(source, &json!({"url": "/orders?a=1&b=2"})),
        "<a href=\"/orders?a=1&amp;b=2\">x</a><img src=\"/orders?a=1&amp;b=2\">"
    );
}

#[test]
fn authored_urls_are_guarded_and_kept_as_written() {
    assert_eq!(
        render(
            "<a href=\"javascript:alert(1)\">x</a><img src=\"/a?b=1&amp;c=2\">\
             <a href=\"/o?a=1&amp;b={data.b}\">y</a>",
            &json!({"b": "&"})
        ),
        "<a href=\"#\">x</a><img src=\"/a?b=1&amp;c=2\"><a href=\"/o?a=1&amp;b=%26\">y</a>"
    );
}

#[test]
fn numbers_and_booleans_fill_url_parts() {
    let data = json!({"n": -3, "flag": true, "ratio": 1.5});
    assert_eq!(
        render(
            "<a href=\"/x/{data.n}/{data.flag}?r={data.ratio}\">x</a>",
            &data
        ),
        "<a href=\"/x/-3/true?r=1.5\">x</a>"
    );
}

#[test]
fn action_urls_are_guarded() {
    let data = json!({"url": "javascript:alert(1)"});
    assert!(render("<Action post={data.url} label=\"x\" />", &data).contains("hx-post=\"#\""));
}

const LIST: &str = "<Section title=\"Orders\" id=\"orders\">\
{#each data.orders as order}<div id={order.dom_id}>{order.name}</div>{/each}\
{#if data.secret}<p id=\"secret\">hidden</p>{/if}\
</Section><p id=\"footer\">f</p>";

fn list_data() -> Value {
    json!({"orders": [{"dom_id": "o1", "name": "A"}, {"dom_id": "o2", "name": "B"}], "secret": false})
}

#[test]
fn fragment_at_top_level() {
    assert_eq!(
        fragment(LIST, &list_data(), "footer").expect("found"),
        "<p id=\"footer\">f</p>"
    );
    assert_eq!(
        fragment(LIST, &list_data(), "orders").expect("found"),
        render(LIST, &list_data()).replace("<p id=\"footer\">f</p>", "")
    );
}

#[test]
fn fragment_inside_each_uses_the_item_scope() {
    assert_eq!(
        fragment(LIST, &list_data(), "o2").expect("found"),
        "<div id=\"o2\">B</div>"
    );
}

fn not_found(id: &str) -> RenderError {
    RenderError::FragmentNotFound { id: id.to_owned() }
}

#[test]
fn fragment_not_found_is_its_own_error() {
    let error = fragment(LIST, &list_data(), "missing").expect_err("not found");
    assert_eq!(error, not_found("missing"));
    assert!(error.to_string().contains("`missing`"), "{error}");
    assert_eq!(fragment(LIST, &list_data(), ""), Err(not_found("")));
}

#[test]
fn fragment_in_a_false_branch_is_not_found() {
    assert_eq!(
        fragment(LIST, &list_data(), "secret"),
        Err(not_found("secret"))
    );
}

#[test]
fn a_defect_inside_the_fragment_is_a_template_error() {
    let source = "<div id=\"box\"><Form id=\"f\" post=\"/x\">\
                  <Field name=\"total\" currency={data.currency} /></Form></div>";
    let error = describe_one(source, MONEY)
        .unwrap()
        .render_fragment(&json!({"currency": "<b>"}), "box")
        .expect_err("defect");
    let RenderError::Template(error) = error else {
        panic!("expected a template error, got {error:?}");
    };
    assert_eq!((error.template.as_str(), error.line), ("test", 1));
}

#[test]
fn fragment_of_a_table() {
    let source = "<p>intro</p><Table id=\"orders\" of={data.rows} columns=\"name\" />";
    let html = fragment(source, &json!({"rows": [{"name": "a"}]}), "orders").expect("found");
    assert!(html.starts_with("<table id=\"orders\">"), "{html}");
    assert!(!html.contains("intro"));
}

#[test]
fn table_renders_header_rows_and_cell_shapes() {
    let data = json!({"orders": [
        {"account": "<Acme>", "status": "paid", "total": {"minor": 14990, "currency": "EUR"}, "created_at": 3, "flag": true},
        {"account": null, "status": {"x": 1}, "total": {"minor": 500, "currency": "JPY"}},
    ]});
    let html = render(
        "<Table id=\"orders\" of={data.orders} columns=\"account status total created_at flag\" />",
        &data,
    );
    assert_eq!(
        html,
        "<table id=\"orders\"><thead><tr>\
<th scope=\"col\">Account</th><th scope=\"col\">Status</th><th scope=\"col\">Total</th>\
<th scope=\"col\">Created at</th><th scope=\"col\">Flag</th></tr></thead><tbody>\
<tr><td>&lt;Acme&gt;</td><td>paid</td><td class=\"sofie-money\">149.90 EUR</td><td>3</td><td>true</td></tr>\
<tr><td></td><td></td><td class=\"sofie-money\">500 JPY</td><td></td><td></td></tr>\
</tbody></table>"
    );
}

#[test]
fn empty_table_renders_an_empty_row() {
    let html = render(
        "<Table id=\"t\" of={data.rows} columns=\"a b\" />",
        &json!({"rows": []}),
    );
    assert_eq!(
        html,
        "<table id=\"t\"><thead><tr><th scope=\"col\">A</th><th scope=\"col\">B</th></tr></thead>\
<tbody><tr class=\"sofie-empty\"><td colspan=\"2\">No entries</td></tr></tbody></table>"
    );
}

#[test]
fn money_component() {
    let source = "<Money value={data.price} />";
    assert_eq!(
        render(
            source,
            &json!({"price": {"minor": 1050, "currency": "BHD"}})
        ),
        "<span class=\"sofie-money\">1.050 BHD</span>"
    );
    assert_eq!(
        render(
            source,
            &json!({"price": {"minor": -199, "currency": "EUR"}})
        ),
        "<span class=\"sofie-money\">-1.99 EUR</span>"
    );
    assert_eq!(
        render(source, &json!({"price": 12.5})),
        "<span class=\"sofie-money\"></span>"
    );
}

#[test]
fn a_literal_post_a_browser_would_change_is_refused() {
    assert_parse_error(
        "<Action post=\"/a b\" label=\"x\" />",
        1,
        9,
        "percent-encode",
    );
    assert_parse_error(
        "<Form id=\"f\" post=\"/orders/{data.id}/ü\"></Form>",
        1,
        14,
        "percent-encode",
    );
    assert_parse_error(
        "<Action post=\"/a?b=1&amp;c=2\" label=\"x\" />",
        1,
        9,
        "'&'",
    );
}

#[test]
fn action_component() {
    assert_eq!(
        render(
            "<Action post=\"/orders/1/cancel\" label=\"Cancel\" target=\"orders\" />",
            &json!({})
        ),
        "<button type=\"button\" hx-post=\"/orders/1/cancel\" hx-target=\"#orders\" hx-sync=\"this:drop\" hx-swap=\"outerHTML\">Cancel</button>"
    );
    assert_eq!(
        render(
            "<Action id={data.id} post={data.url} label={data.label} />",
            &json!({"id": "b1", "url": "/x", "label": "<b>"})
        ),
        "<button type=\"button\" id=\"b1\" hx-post=\"/x\" hx-swap=\"outerHTML\">&lt;b&gt;</button>"
    );
}

#[test]
fn a_bound_target_that_is_no_element_id_targets_nothing() {
    let source = "<Action post=\"/x\" label=\"x\" target={data.t} />";
    for (target, written) in [("body, #x", ""), ("a&b", ""), ("row-7", "row-7")] {
        let html = render(source, &json!({ "t": target }));

        assert!(
            html.contains(&format!(" hx-target=\"#{written}\" ")),
            "{target}: {html}"
        );
    }
}

const ORDERS_WITH_ACTIONS: &str = "<Table id=\"orders\" of={data.orders} columns=\"status\" as=\"order\">\n\
  <Action post=\"/orders/{order.id}/cancel\" label=\"Cancel\" target=\"orders\" when={order.status_to.CANCELLED} />\n\
  <a href=\"/orders/{order.id}\">Open {order.status}</a>\n\
</Table>";

#[test]
fn table_actions_render_once_per_row_with_the_row_bound() {
    let data = json!({"orders": [
        {"id": "a1", "status": "PENDING", "status_to": {"PAID": true, "CANCELLED": true}},
        {"id": "b2", "status": "CANCELLED", "status_to": {}},
    ]});
    assert_eq!(
        render(ORDERS_WITH_ACTIONS, &data),
        "<table id=\"orders\"><thead><tr><th scope=\"col\">Status</th><th scope=\"col\"></th></tr></thead><tbody>\
<tr><td>PENDING</td><td class=\"sofie-actions\">\n\
<button type=\"button\" hx-post=\"/orders/a1/cancel\" hx-target=\"#orders\" hx-sync=\"this:drop\" hx-swap=\"outerHTML\">Cancel</button>\n\
<a href=\"/orders/a1\">Open PENDING</a>\n</td></tr>\
<tr><td>CANCELLED</td><td class=\"sofie-actions\">\n\n<a href=\"/orders/b2\">Open CANCELLED</a>\n</td></tr>\
</tbody></table>"
    );
}

#[test]
fn an_empty_table_with_actions_spans_the_actions_column() {
    let html = render(ORDERS_WITH_ACTIONS, &json!({"orders": []}));
    assert!(
        html.contains("<tr class=\"sofie-empty\"><td colspan=\"2\">No entries</td></tr>"),
        "{html}"
    );
}

#[test]
fn table_row_content_is_any_markup_with_the_row_bound() {
    let html = render(
        "<Table id=\"t\" of={data.rows} columns=\"a\" as=\"row\">\
         <b>{row.a}</b>{#if row.paid}paid{/if}<Money value={row.total} /></Table>",
        &json!({"rows": [{"a": "<x>", "paid": true, "total": {"minor": 5, "currency": "EUR"}}]}),
    );
    assert!(
        html.contains(
            "<td>&lt;x&gt;</td><td class=\"sofie-actions\"><b>&lt;x&gt;</b>paid\
             <span class=\"sofie-money\">0.05 EUR</span></td>"
        ),
        "{html}"
    );
}

#[test]
fn table_row_content_needs_a_valid_row_name() {
    assert_parse_error(
        "<Table id=\"t\" of={data.rows} columns=\"a\"><Action post=\"/x\" label=\"x\" /></Table>",
        1,
        1,
        "needs `as=",
    );
    assert_parse_error(
        "<Table id=\"t\" of={data.rows} columns=\"a\" as={data.x} />",
        1,
        42,
        "`as` needs a quoted name",
    );
    assert_parse_error(
        "<Table id=\"t\" of={data.rows} columns=\"a\" as=\"data\"></Table>",
        1,
        42,
        "`as` needs a quoted name",
    );
    assert_parse_error(
        "<Table id=\"t\" of={data.rows} columns=\"a\" as={data.x}></Table>",
        1,
        42,
        "`as` needs a quoted name",
    );
    assert_parse_error(
        "<Table id=\"t\" of={data.rows} columns=\"a\" as=\"row\" />{row.a}",
        1,
        54,
        "unknown name `row`",
    );
    assert_parse_error(
        "<Section title=\"x\" as=\"row\"></Section>",
        1,
        20,
        "no prop `as`",
    );
}

#[test]
fn action_renders_only_when_its_condition_holds() {
    let source = "<Action post=\"/x\" label=\"Go\" when={data.allowed} />";
    for (allowed, shown) in [
        (json!(true), true),
        (json!(false), false),
        (json!(null), false),
        (json!({}), false),
    ] {
        let html = render(source, &json!({"allowed": allowed}));
        assert_eq!(html.contains("<button"), shown, "{allowed}");
    }
    assert_eq!(render(source, &json!({})), "");
}

#[test]
fn section_component() {
    let html = render(
        "<Section title={data.title} id=\"s\"><p>Hi {data.name}</p></Section>",
        &json!({"title": "<T>", "name": "N"}),
    );
    assert_eq!(
        html,
        "<section id=\"s\"><h2>&lt;T&gt;</h2><p>Hi N</p></section>"
    );
}

#[test]
fn props_read_as_one_value_refuse_text_with_parts() {
    for (source, column) in [
        ("<Money value=\"{data.a}\" />", 8),
        ("<Action post=\"/x\" label=\"x\" when=\"{data.a}\" />", 29),
        ("<Table id=\"t\" of=\"{data.a}\" columns=\"a\" />", 15),
    ] {
        assert_parse_error(source, 1, column, "is one value");
    }
    for (field, input) in [
        ("options={data.o} value=\"{data.a}\"", SELECT),
        ("value=\"x{data.a}\"", CHECKBOX),
        ("value=\"{data.a}\"", MONEY),
        ("currency=\"{data.c}\"", MONEY),
    ] {
        let error = describe_one(
            &format!("<Form id=\"f\" post=\"/x\"><Field name=\"a\" {field} /></Form>"),
            input,
        )
        .expect_err("the field is refused");
        assert!(error.message.contains("is one value"), "{field}: {error}");
    }
    assert_eq!(
        render(
            "<Form id=\"f\" post=\"/x\"><Field name=\"a\" value=\"Hi {data.n}\" /></Form>",
            &json!({"n": "N"})
        )
        .matches("value=\"Hi N\"")
        .count(),
        1
    );
}

#[test]
fn props_that_read_data_refuse_literals() {
    for (source, column) in [
        ("<Money value=\"12\" />", 8),
        ("<Money value={\"12\"} />", 8),
        ("<Table id=\"t\" of=\"data.rows\" columns=\"a\" />", 15),
    ] {
        assert_parse_error(source, 1, column, "reads data");
    }
    let error = describe_one(
        "<Form id=\"f\" post=\"/x\"><Field name=\"a\" value=\"12\" /></Form>",
        MONEY,
    )
    .expect_err("the field is refused");
    assert!(error.message.contains("reads data"), "{error}");
}

#[test]
fn a_form_in_a_loop_or_table_row_is_refused() {
    assert_parse_error(
        "{#each data.rows as row}<Form id=\"f\" post=\"/x\"></Form>{/each}",
        1,
        25,
        "cannot be inside",
    );
    assert_parse_error(
        "<Table id=\"t\" of={data.rows} columns=\"a\" as=\"row\">\
<Form id=\"f\" post=\"/x\"></Form></Table>",
        1,
        51,
        "cannot be inside",
    );
}

#[test]
fn a_literal_id_is_refused_where_it_would_repeat() {
    assert_parse_error(
        "<p id=\"a\"></p>\n<Section title=\"S\" id=\"a\"></Section>",
        2,
        20,
        "given twice",
    );
    assert_parse_error(
        "{#each data.rows as row}<p id=\"row\"></p>{/each}",
        1,
        28,
        "would repeat",
    );
    assert_eq!(
        render(
            "{#if data.a}<p id=\"x\">a</p>{:else}<p id=\"x\">b</p>{/if}",
            &json!({"a": true})
        ),
        "<p id=\"x\">a</p>"
    );
}

#[test]
fn page_wraps_the_body_in_a_strict_document() {
    let html = Page {
        title: "Orders <&>",
        lang: "en",
        body: "<p>x</p>",
        notice: &[],
        nonce: "n0nce",
        islands: None,
        script_notices: sofie_engine::ScriptNotices::plain(),
    }
    .render();
    assert_eq!(
        html,
        format!(
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>Orders &lt;&amp;&gt;</title>\
<meta name=\"htmx-config\" content='{{\"allowEval\":false,\"allowScriptTags\":false,\"selfRequestsOnly\":true,\"includeIndicatorStyles\":false,\"historyCacheSize\":0,\
\"responseHandling\":[{{\"code\":\"204\",\"swap\":false}},{{\"code\":\"[234]..\",\"swap\":true}},{{\"code\":\"5..\",\"swap\":true,\"error\":true}}]}}'>\
<link rel=\"stylesheet\" href=\"{}\">\
<script src=\"{}\" nonce=\"n0nce\" defer></script>\
<script src=\"{}\" nonce=\"n0nce\" defer></script></head>\
<body><div id=\"sofie-notice\" role=\"status\" aria-live=\"polite\"></div><p>x</p></body></html>",
            STYLESHEET.path(),
            CLIENT_SCRIPT.path(),
            ENGINE_SCRIPT.path(),
        )
    );
    assert!(!html.contains("<style"));
    assert_eq!(NOTICE_TARGET, "#sofie-notice");
}

#[test]
fn a_template_s_title_names_the_page_in_its_head() {
    let body = render(
        "<title>Order {data.id}</title>\n<Section title=\"Your order\"></Section>",
        &json!({"id": "<7>"}),
    );
    let html = Page {
        title: "Order",
        lang: "en",
        body: &body,
        notice: &[],
        nonce: "n0nce",
        islands: None,
        script_notices: sofie_engine::ScriptNotices::plain(),
    }
    .render();

    let (head, page_body) = html.split_once("</head>").unwrap();
    assert!(
        head.contains("<title>Order &lt;7&gt;</title><meta name=\"htmx-config\""),
        "{head}"
    );
    assert_eq!(head.matches("<title>").count(), 1, "{head}");
    assert!(!page_body.contains("<title>"), "{page_body}");
    assert!(page_body.contains("<h2>Your order</h2>"), "{page_body}");
}

#[test]
fn a_title_is_written_once_as_text_at_the_template_s_top_level() {
    for source in [
        "<Section title=\"S\"><title>T</title></Section>",
        "{#if data.a}<title>T</title>{/if}",
        "<title>T</title><title>U</title>",
        "<title class=\"x\">T</title>",
    ] {
        let error = parse_error(source).to_string();
        assert!(
            error.contains("`<title>` names the page"),
            "{source}: {error}"
        );
    }
    let error = parse_error("<title><b>T</b></title>").to_string();
    assert!(error.contains("holds only text"), "{error}");
}

#[test]
fn an_svg_s_title_names_its_image_and_stays_in_the_body() {
    let icon = "<svg role=\"img\"><title>Cart</title></svg>";
    for (source, page_title) in [
        (format!("{icon}<title>Orders</title>"), "Orders"),
        (icon.to_owned(), "Fallback"),
    ] {
        let body = render(&source, &json!({}));
        let html = Page {
            title: "Fallback",
            lang: "en",
            body: &body,
            notice: &[],
            nonce: "n0nce",
            islands: None,
            script_notices: sofie_engine::ScriptNotices::plain(),
        }
        .render();

        let (head, page_body) = html.split_once("</head>").unwrap();
        assert!(
            head.contains(&format!("<title>{page_title}</title>")),
            "{head}"
        );
        assert_eq!(head.matches("<title>").count(), 1, "{head}");
        assert!(page_body.contains(icon), "{page_body}");
    }
}

#[test]
fn a_page_can_open_with_a_notice() {
    let html = Page {
        title: "Orders",
        lang: "en",
        body: "<p>x</p>",
        notice: &["Total <must> be positive".to_owned()],
        nonce: "n0nce",
        islands: None,
        script_notices: sofie_engine::ScriptNotices::plain(),
    }
    .render();
    assert!(
        html.contains(
            "<body><div id=\"sofie-notice\" role=\"status\" aria-live=\"polite\">\
<p>Total &lt;must&gt; be positive</p></div><p>x</p></body>"
        ),
        "{html}"
    );
}

#[test]
fn a_notice_is_one_escaped_paragraph_per_message() {
    assert_eq!(
        notice(&["Total must be positive", "<b>&</b>"]),
        "<p>Total must be positive</p><p>&lt;b&gt;&amp;&lt;/b&gt;</p>"
    );
    assert_eq!(notice(&[] as &[&str]), "");
}

#[test]
fn quoted_values_fill_in_their_parts() {
    let data = json!({"order": {"id": "o 1/../x?y", "state": "<b>"}, "n": 7});
    let html = render(
        "<a href=\"/orders/{data.order.id}/cancel?n={data.n}\" \
         title=\"{data.order.state} #{data.n}\">x</a>",
        &data,
    );
    assert_eq!(
        html,
        "<a href=\"/orders/o%201%2F..%2Fx%3Fy/cancel?n=7\" title=\"&lt;b&gt; #7\">x</a>"
    );
}

#[test]
fn an_action_in_a_loop_posts_to_its_row() {
    let data = json!({"orders": [{"id": "a1"}, {"id": "b2"}]});
    let html = render(
        "{#each data.orders as order}<Action post=\"/orders/{order.id}/cancel\" label=\"Cancel\" target=\"orders\" />{/each}",
        &data,
    );
    assert_eq!(
        html,
        "<button type=\"button\" hx-post=\"/orders/a1/cancel\" hx-target=\"#orders\" hx-sync=\"this:drop\" hx-swap=\"outerHTML\">Cancel</button>\
<button type=\"button\" hx-post=\"/orders/b2/cancel\" hx-target=\"#orders\" hx-sync=\"this:drop\" hx-swap=\"outerHTML\">Cancel</button>"
    );
}

#[test]
fn filled_parts_cannot_make_a_script_url_or_escape_the_id() {
    let data = json!({"s": "javascript", "id": "x\" onclick=\"y"});
    assert_eq!(
        render("<a href=\"{data.s}:alert(1)\">x</a>", &data),
        "<a href=\"#\">x</a>"
    );
    assert_eq!(
        render("<p id=\"row-{data.id}\"></p>", &data),
        "<p id=\"row-x&quot; onclick=&quot;y\"></p>"
    );
}

#[test]
fn a_fragment_can_be_found_by_an_interpolated_id() {
    let source = "{#each data.rows as row}<p id=\"row-{row.id}\">{row.id}</p>{/each}";
    let data = json!({"rows": [{"id": 1}, {"id": 2}]});
    assert_eq!(
        fragment(source, &data, "row-2").expect("found"),
        "<p id=\"row-2\">2</p>"
    );
}

#[test]
fn quoted_value_errors() {
    assert_parse_error("<a href=\"/x/{item.id}\">", 1, 14, "unknown name `item`");
    assert_parse_error("<a href=\"/x/{data.id\">", 1, 21, "expected `}`");
    assert_parse_error(
        "<Table id=\"t\" of={data.rows} columns=\"{data.c}\" />",
        1,
        30,
        "without `{…}`",
    );
}

#[test]
fn odd_inputs_never_panic() {
    let sources = [
        "",
        "<",
        "{",
        "}",
        "<>",
        "</",
        "{#",
        "{/",
        "{:",
        "{#if",
        "{#each",
        "{#each data.a as",
        "<a href=",
        "<a href={",
        "<A",
        "<Table",
        "\u{0}",
        "é<é>",
        "{\"\\",
        "<!--",
        "<p {data}>",
        "{-}",
        "{data..a}",
        "{#if data.a}{:else}{:else}{/if}",
        "<br></br>",
        "<Section />",
        "{data.a.b.c.d.e}",
        "<p\n\n\t>",
        "<Form",
        "<Field",
        "<Form id=\"f\" post=\"/x\"><Field",
        "<Form id=\"f\" post=\"/x\"><Field name=\"a\" currency={data.a} value={data.a} /></Form>",
        "<Form id=\"f\" post=\"/x\"><Field name=\"a\" options={data.a} value={data.a} /></Form>",
        "<Form id=\"f\" post=\"/x\"><Field name=\"a\" value={data.a} required /></Form>",
    ];
    let described = [MONEY, SELECT, CHECKBOX].into_iter().flat_map(|input| {
        sources[sources.len() - 3..]
            .iter()
            .filter_map(move |source| describe_one(source, input).ok())
    });
    let parsed = sources
        .iter()
        .filter_map(|source| Template::parse("odd", source).ok());
    for template in parsed.chain(described) {
        for data in [
            json!(null),
            json!([]),
            json!({"a": [1, {"b": null}]}),
            json!("s"),
        ] {
            let _ = template.render(&data);
            let _ = template.render_fragment(&data, "x");
        }
    }
    for length in 0..60 {
        let source: String = "<{#/}a=\"é:x "
            .chars()
            .cycle()
            .skip(length)
            .take(length)
            .collect();
        let _ = Template::parse("generated", &source);
    }
}

#[test]
fn renders_a_large_table_quickly() {
    let rows: Vec<Value> = (0..2_000)
        .map(|index| json!({"name": format!("row {index}"), "total": {"minor": index * 7, "currency": "EUR"}, "note": "<&>"}))
        .collect();
    let data = json!({"rows": rows});
    let screen = template(
        "<Section title=\"All\"><Table id=\"t\" of={data.rows} columns=\"name total note\" /></Section>",
    );
    let started = Instant::now();
    for _ in 0..20 {
        let html = screen.render(&data).expect("renders");
        assert!(html.len() > 100_000);
    }
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
}

const FORM_OPEN: &str =
    "<form id=\"f\" method=\"post\" action=\"/x\" hx-post=\"/x\" hx-swap=\"outerHTML\">";

/// Controls the tests describe a field `a` with, as a value's `Present` would.
const SELECT: Input = Input::new(Control::Select);
const CHECKBOX: Input = Input::new(Control::Checkbox);
const MONEY: Input = Input::new(Control::Money);

fn describe_one(source: &str, input: Input) -> Result<Template, TemplateError> {
    let mut template = Template::parse("test", source)?;
    let name = source
        .split("<Field name=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("one named field");
    let field = Box::leak(Box::new([FormField {
        name: Box::leak(name.to_owned().into_boxed_str()),
        input,
        required: source.contains(" required"),
        constraints: &[],
    }]));
    template.describe_form("f", field)?;
    Ok(template)
}

fn described_field(source: &str, input: Input, data: &Value) -> String {
    let html = describe_one(
        &format!("<Form id=\"f\" post=\"/x\">{source}</Form>"),
        input,
    )
    .and_then(|template| template.render(data))
    .unwrap_or_else(|error| panic!("{error}"));
    strip_form(&html)
}

fn field(source: &str, data: &Value) -> String {
    strip_form(&render(
        &format!("<Form id=\"f\" post=\"/x\">{source}</Form>"),
        data,
    ))
}

fn strip_form(html: &str) -> String {
    html.strip_prefix(FORM_OPEN)
        .and_then(|rest| rest.strip_suffix("</form>"))
        .unwrap_or_else(|| panic!("not a form around one field: {html}"))
        .to_owned()
}

fn wrapped(name: &str, label: &str, control: &str) -> String {
    format!(
        "<div class=\"sofie-field\"><label for=\"f-{name}\">{label}</label>{control}\
<p class=\"sofie-field-error\" id=\"f-{name}-error\" aria-live=\"polite\"></p></div>"
    )
}

#[test]
fn form_posts_with_htmx_and_renders_its_parts() {
    let html = render(
        "<Form id=\"place-order\" post=\"/orders\" target=\"orders\">\n\
           <Field name=\"email\" required />\n\
           <Submit label=\"Place order\" />\n\
         </Form>",
        &json!({}),
    );
    assert_eq!(
        html,
        "<form id=\"place-order\" method=\"post\" action=\"/orders\" hx-post=\"/orders\" \
hx-target=\"#orders\" hx-sync=\"this:drop\" hx-swap=\"outerHTML\">\
<div class=\"sofie-field\"><label for=\"place-order-email\">Email</label>\
<input type=\"text\" id=\"place-order-email\" name=\"email\" \
aria-describedby=\"place-order-email-error\" required>\
<p class=\"sofie-field-error\" id=\"place-order-email-error\" aria-live=\"polite\"></p></div>\
<button type=\"submit\">Place order</button></form>"
    );
}

#[test]
fn form_urls_and_targets_are_guarded() {
    let data = json!({"url": "javascript:alert(1)", "target": "x, body"});
    let html = render(
        "<Form id=\"f\" post={data.url}><Submit label=\"Go\" /></Form>",
        &data,
    );
    assert!(
        html.starts_with("<form id=\"f\" method=\"post\" action=\"#\" hx-post=\"#\""),
        "{html}"
    );
    let html = render(
        "<Form id=\"f\" post=\"/x\" target={data.target}></Form>",
        &data,
    );
    assert!(html.contains(" hx-target=\"#\" "), "{html}");
}

#[test]
fn text_field_escapes_its_label_and_value() {
    let data = json!({"label": "<L>", "name": "\"x\" & <y>"});
    assert_eq!(
        field(
            "<Field name=\"name\" label={data.label} value={data.name} />",
            &data
        ),
        wrapped(
            "name",
            "&lt;L&gt;",
            "<input type=\"text\" id=\"f-name\" name=\"name\" aria-describedby=\"f-name-error\" \
value=\"&quot;x&quot; &amp; &lt;y&gt;\">"
        )
    );
}

#[test]
fn a_date_time_control_shows_an_instant_to_the_minute_or_with_its_seconds() {
    let date_time = |at| {
        described_field(
            "<Field name=\"at\" value={data.at} />",
            Input::typed(&[("type", "datetime-local")]),
            &json!({ "at": at }),
        )
    };
    assert_eq!(
        date_time("2026-10-11T09:30:00Z"),
        wrapped(
            "at",
            "At",
            "<input type=\"datetime-local\" id=\"f-at\" name=\"at\" \
aria-describedby=\"f-at-error\" value=\"2026-10-11T09:30\">"
        )
    );
    assert_eq!(
        date_time("2026-10-11T09:30:15.5Z"),
        wrapped(
            "at",
            "At",
            "<input type=\"datetime-local\" step=\"any\" id=\"f-at\" name=\"at\" \
aria-describedby=\"f-at-error\" value=\"2026-10-11T09:30:15.5\">"
        )
    );
}

#[test]
fn described_inputs_render_their_type() {
    assert_eq!(
        described_field(
            "<Field name=\"quantity\" value={data.q} />",
            Input::typed(&[("type", "number")]),
            &json!({"q": 3})
        ),
        wrapped(
            "quantity",
            "Quantity",
            "<input type=\"number\" id=\"f-quantity\" name=\"quantity\" \
aria-describedby=\"f-quantity-error\" value=\"3\">"
        )
    );
    assert_eq!(
        described_field(
            "<Field name=\"due_on\" required />",
            Input::typed(&[("type", "date")]),
            &json!({})
        ),
        wrapped(
            "due_on",
            "Due on",
            "<input type=\"date\" id=\"f-due_on\" name=\"due_on\" \
aria-describedby=\"f-due_on-error\" required>"
        )
    );
    assert_eq!(
        described_field(
            "<Field name=\"password\" required />",
            Input::new(Control::Password),
            &json!({})
        ),
        wrapped(
            "password",
            "Password",
            "<input type=\"password\" id=\"f-password\" name=\"password\" \
aria-describedby=\"f-password-error\" required>"
        )
    );
}

#[test]
fn a_hidden_field_has_no_label() {
    assert_eq!(
        described_field(
            "<Field name=\"role\" value={data.role} />",
            Input::typed(&[("type", "hidden")]),
            &json!({"role": "r-1"})
        ),
        "<div class=\"sofie-field\"><input type=\"hidden\" id=\"f-role\" name=\"role\" \
aria-describedby=\"f-role-error\" value=\"r-1\">\
<p class=\"sofie-field-error\" id=\"f-role-error\" aria-live=\"polite\"></p></div>"
    );
}

#[test]
fn a_password_field_never_shows_a_value() {
    let error = describe_one(
        "<Form id=\"f\" post=\"/x\"><Field name=\"p\" value={data.p} /></Form>",
        Input::new(Control::Password),
    )
    .expect_err("the value is refused");
    assert_eq!((error.line, error.column), (1, 24));
    assert!(
        error
            .message
            .contains("a `password` field takes no `value`"),
        "{error}"
    );
}

#[test]
fn a_file_field_sends_its_form_as_multipart_and_shows_no_value() {
    let html = describe_one(
        "<Form id=\"f\" post=\"/x\"><Field name=\"photo\" required /></Form>",
        Input::new(Control::File),
    )
    .and_then(|template| template.render(&json!({})))
    .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        html,
        format!(
            "<form id=\"f\" method=\"post\" enctype=\"multipart/form-data\" action=\"/x\" \
hx-post=\"/x\" hx-swap=\"outerHTML\">{}</form>",
            wrapped(
                "photo",
                "Photo",
                "<input type=\"file\" id=\"f-photo\" name=\"photo\" \
aria-describedby=\"f-photo-error\" required>"
            )
        )
    );
    let error = describe_one(
        "<Form id=\"f\" post=\"/x\"><Field name=\"photo\" value={data.p} /></Form>",
        Input::new(Control::File),
    )
    .expect_err("the value is refused");
    assert!(
        error.message.contains("a file field takes no `value`"),
        "{error}"
    );
}

#[test]
fn textarea_field_escapes_its_content() {
    assert_eq!(
        described_field(
            "<Field name=\"note\" value={data.note} />",
            Input::new(Control::Textarea),
            &json!({"note": "</textarea><b>"})
        ),
        wrapped(
            "note",
            "Note",
            "<textarea id=\"f-note\" name=\"note\" aria-describedby=\"f-note-error\">\n\
&lt;/textarea&gt;&lt;b&gt;</textarea>"
        )
    );
}

#[test]
fn a_textarea_keeps_a_leading_newline_of_its_value() {
    let html = described_field(
        "<Field name=\"note\" value={data.note} />",
        Input::new(Control::Textarea),
        &json!({"note": "\nindented"}),
    );

    assert!(
        html.contains("aria-describedby=\"f-note-error\">\n\nindented</textarea>"),
        "{html}"
    );
}

#[test]
fn checkbox_field_always_posts_its_name() {
    let source = "<Field name=\"gift\" value={data.gift} />";
    let control = |checked: &str| {
        wrapped(
            "gift",
            "Gift",
            &format!(
                "<input type=\"hidden\" name=\"gift\" value=\"false\"><input type=\"checkbox\" \
id=\"f-gift\" name=\"gift\" aria-describedby=\"f-gift-error\" value=\"true\"{checked}>"
            ),
        )
    };
    let field = |data| described_field(source, CHECKBOX, &data);
    assert_eq!(field(json!({"gift": true})), control(" checked"));
    assert_eq!(field(json!({"gift": false})), control(""));
    assert_eq!(field(json!({})), control(""));
}

#[test]
fn only_true_checks_a_bound_checkbox() {
    let source = "<Field name=\"gift\" value={data.gift} />";
    for gift in [json!("false"), json!("true"), json!(1), json!("no")] {
        let html = described_field(source, CHECKBOX, &json!({ "gift": gift }));

        assert!(!html.contains("checked"), "{gift}: {html}");
    }
}

struct Assignee {
    people: Vec<sofie_core::Uuid>,
    person: sofie_core::Uuid,
}

impl sofie_core::Show for Assignee {
    fn show(&self) -> sofie_core::Shown<'_> {
        sofie_core::Shown::Record(self)
    }
}

impl sofie_core::show::Fields for Assignee {
    fn name(&self, index: usize) -> Option<&str> {
        ["people", "person"].get(index).copied()
    }

    fn get(&self, name: &str) -> Option<sofie_core::Shown<'_>> {
        match name {
            "people" => Some(sofie_core::Shown::List(&self.people)),
            "person" => Some(sofie_core::Shown::Uuid(self.person)),
            _ => None,
        }
    }
}

#[test]
fn select_field_offers_ids_from_a_typed_read_model() {
    let (first, second) = (
        sofie_core::Uuid::from_u128(1),
        sofie_core::Uuid::from_u128(2),
    );
    let data = Assignee {
        people: vec![first, second],
        person: second,
    };

    let html = describe_one(
        "<Form id=\"f\" post=\"/x\"><Field name=\"person\" options={data.people} value={data.person} /></Form>",
        SELECT,
    )
    .and_then(|template| template.render(&data))
    .unwrap_or_else(|error| panic!("{error}"));

    assert!(
        html.contains(&format!("<option value=\"{first}\">{first}</option>")),
        "{html}"
    );
    assert!(
        html.contains(&format!("<option value=\"{second}\" selected>")),
        "{html}"
    );
}

#[test]
fn select_field_marks_the_current_choice() {
    let data = json!({"statuses": ["PENDING", "PAID", "<x>", 7], "status": "PAID"});
    assert_eq!(
        described_field(
            "<Field name=\"status\" options={data.statuses} value={data.status} />",
            SELECT,
            &data
        ),
        wrapped(
            "status",
            "Status",
            "<select id=\"f-status\" name=\"status\" aria-describedby=\"f-status-error\">\
<option value=\"\"></option><option value=\"PENDING\">PENDING</option>\
<option value=\"PAID\" selected>PAID</option><option value=\"&lt;x&gt;\">&lt;x&gt;</option>\
<option value=\"7\">7</option></select>"
        )
    );
    let required = described_field(
        "<Field name=\"status\" options={data.statuses} required />",
        SELECT,
        &data,
    );
    assert!(
        required.contains("required><option value=\"PENDING\">PENDING</option>"),
        "{required}"
    );
    assert!(!required.contains("selected"), "{required}");
}

#[test]
fn select_field_offers_and_selects_numbers() {
    let html = described_field(
        "<Field name=\"size\" options={data.sizes} value={data.size} />",
        SELECT,
        &json!({"sizes": [1, 2.5, 3], "size": 2.5}),
    );

    assert!(
        html.contains(
            "<option value=\"1\">1</option><option value=\"2.5\" selected>2.5</option>\
<option value=\"3\">3</option>"
        ),
        "{html}"
    );
}

#[test]
fn money_field_posts_an_amount_and_its_currency() {
    assert_eq!(
        described_field(
            "<Field name=\"total\" currency=\"EUR\" required />",
            MONEY,
            &json!({})
        ),
        wrapped(
            "total",
            "Total",
            "<div class=\"sofie-money-input\"><input type=\"text\" inputmode=\"decimal\" \
id=\"f-total\" name=\"total.amount\" aria-describedby=\"f-total-error\" required>\
<span class=\"sofie-currency\">EUR</span><input type=\"hidden\" name=\"total.currency\" value=\"EUR\"></div>"
        )
    );
}

#[test]
fn percentage_field_shows_basis_points_as_a_percent() {
    let percentage = Input::new(Control::Percentage);
    let field = "<Field name=\"rate\" value={data.rate} />";

    assert_eq!(
        described_field(field, percentage, &json!({"rate": 1950})),
        wrapped(
            "rate",
            "Rate",
            "<div class=\"sofie-percentage-input\"><input type=\"text\" inputmode=\"decimal\" \
id=\"f-rate\" name=\"rate\" aria-describedby=\"f-rate-error\" value=\"19.50\">\
<span class=\"sofie-unit\">%</span></div>"
        )
    );
    assert!(
        described_field(field, percentage, &json!({"rate": -5})).contains("value=\"-0.05\""),
        "a share below one percent keeps its sign"
    );
}

#[test]
fn a_value_of_parts_posts_each_part_under_the_field_s_name() {
    const WEIGHT: Input = Input::new(Control::Parts(&[
        Part {
            name: "quantity",
            attributes: &[("type", "text")],
            options: &[],
        },
        Part {
            name: "unit",
            attributes: &[],
            options: &["GRAM", "KILOGRAM"],
        },
    ]));

    assert_eq!(
        described_field(
            "<Field name=\"weight\" value={data.weight} required />",
            WEIGHT,
            &json!({"weight": {"quantity": "1.5", "unit": "KILOGRAM"}})
        ),
        wrapped(
            "weight",
            "Weight",
            "<div class=\"sofie-parts\"><input type=\"text\" id=\"f-weight\" name=\"weight.quantity\" \
aria-describedby=\"f-weight-error\" required value=\"1.5\"><select aria-label=\"Unit\" \
name=\"weight.unit\" aria-describedby=\"f-weight-error\" required>\
<option value=\"GRAM\">GRAM</option><option value=\"KILOGRAM\" selected>KILOGRAM</option></select></div>"
        )
    );
}

#[test]
fn money_field_takes_amount_and_currency_from_a_money_value() {
    let html = described_field(
        "<Field name=\"total\" currency=\"EUR\" value={data.total} />",
        MONEY,
        &json!({"total": {"minor": 1005, "currency": "KWD"}}),
    );
    assert!(
        html.contains(
            "name=\"total.amount\" aria-describedby=\"f-total-error\" value=\"1.005\">\
<span class=\"sofie-currency\">KWD</span><input type=\"hidden\" name=\"total.currency\" value=\"KWD\">"
        ),
        "{html}"
    );
}

#[test]
fn money_field_reads_a_bound_currency() {
    let source = "<Field name=\"total\" currency={data.currency} value={data.total} />";
    let html = described_field(source, MONEY, &json!({"currency": "JPY", "total": null}));
    assert!(
        html.contains("<span class=\"sofie-currency\">JPY</span>") && !html.contains("value=\"\""),
        "{html}"
    );
    for data in [json!({"currency": "<b>"}), json!({})] {
        let message = describe_one(
            &format!("<Form id=\"f\" post=\"/x\">{source}</Form>"),
            MONEY,
        )
        .unwrap()
        .render(&data)
        .expect_err("the render fails")
        .message;
        assert!(message.contains("has no currency"), "{message}");
    }
}

/// A new record's form has no currency yet; it is typed, as with no `currency` at all.
#[test]
fn money_field_bound_to_a_null_currency_takes_it_typed() {
    let source = "<Field name=\"total\" currency={data.currency} value={data.total} />";
    let html = described_field(source, MONEY, &json!({"currency": null, "total": null}));
    let free = described_field("<Field name=\"total\" />", MONEY, &json!({}));

    assert_eq!(html, free);
}

#[test]
fn money_field_without_a_currency_takes_it_typed() {
    let html = described_field("<Field name=\"total\" />", MONEY, &json!({}));
    assert!(
        html.contains(
            "name=\"total.amount\" aria-describedby=\"f-total-error\">\
<input type=\"text\" class=\"sofie-currency\" maxlength=\"3\" aria-label=\"Currency\" \
name=\"total.currency\"></div>"
        ),
        "{html}"
    );
}

#[test]
fn form_parse_errors() {
    assert_parse_error(
        "<Field name=\"a\" />",
        1,
        1,
        "`Field` belongs inside a `Form`",
    );
    assert_parse_error(
        "<Section title=\"x\">\n  <Submit label=\"Go\" />\n</Section>",
        2,
        3,
        "`Submit` belongs inside a `Form`",
    );
    assert_parse_error(
        "<Form id=\"f\" post=\"/x\"><p>x</p></Form>",
        1,
        1,
        "only `Field` and `Submit` children",
    );
    assert_parse_error(
        "<Form id=\"f\" post=\"/x\"><Field name=\"a\" />\n<Field name=\"a\" required /></Form>",
        2,
        1,
        "two fields named `a`",
    );
    assert_parse_error(
        "<Form id=\"f\" post=\"/x\"><Field name=\"a\" kind=\"email\" /></Form>",
        1,
        40,
        "`Field` has no prop `kind`",
    );
}

#[test]
fn a_described_field_refuses_props_its_control_cannot_use() {
    for (field, input, position, message) in [
        ("currency=\"euro\"", MONEY, (1, 40), "not an ISO 4217 code"),
        ("", SELECT, (1, 24), "`options` prop"),
        (
            "currency=\"EUR\"",
            Input::typed(&[("type", "text")]),
            (1, 40),
            "no prop `currency`",
        ),
    ] {
        let source = format!("<Form id=\"f\" post=\"/x\"><Field name=\"a\" {field} /></Form>");
        let error = describe_one(&source, input).expect_err("the field is refused");
        assert_eq!((error.line, error.column), position, "{error}");
        assert!(error.message.contains(message), "{error}");
    }
}

#[test]
fn form_prop_errors() {
    assert_parse_error(
        "<Form id=\"a b\" post=\"/x\"></Form>",
        1,
        7,
        "not an element id",
    );
    assert_parse_error(
        "<Form id={data.id} post=\"/x\"></Form>",
        1,
        7,
        "quoted literal",
    );
    assert_parse_error("<Form id=\"f\"></Form>", 1, 1, "`post` prop");
    assert_parse_error(
        "<Form id=\"f\" post=\"/x\"><Field name=\"a.b\" /></Form>",
        1,
        31,
        "not a field name",
    );
    assert_parse_error(
        "<Form id=\"f\" post=\"/x\"><Field name=\"a\" required=\"yes\" /></Form>",
        1,
        40,
        "takes no value",
    );
    assert_parse_error(
        "<Form id=\"f\" post=\"/x\"><Submit label=\"Go\">x</Submit></Form>",
        1,
        24,
        "takes no children",
    );
}

#[test]
fn bare_attributes_are_empty_values() {
    assert_eq!(
        render("<input disabled name=\"q\">", &json!({})),
        "<input disabled=\"\" name=\"q\">"
    );
}

#[test]
fn attributes_may_space_their_equals_sign_and_use_single_quotes() {
    assert_eq!(
        render(
            "<p title = 'say \"hi\"' class= \"a\" data-n ={data.n}></p>",
            &json!({"n": 7})
        ),
        "<p title=\"say &quot;hi&quot;\" class=\"a\" data-n=\"7\"></p>"
    );
    assert_parse_error("<p title='x>", 1, 10, "never closed with `'`");
}

#[test]
fn a_namespaced_attribute_name_is_refused_by_its_cause() {
    assert_parse_error(
        "<svg><use xlink:href=\"#a\" /></svg>",
        1,
        11,
        "`xlink:` attribute names are not supported",
    );
}

#[test]
fn field_errors_fill_the_slots_fields_render() {
    let html = field_errors(
        "f",
        &[
            ("email", "must contain an @".to_owned()),
            ("total", "<b>&</b>".to_owned()),
        ],
    );
    assert_eq!(
        html,
        "<div hx-swap-oob=\"innerHTML:#f .sofie-field-error\"></div>\
<div hx-swap-oob=\"innerHTML:#f-email-error\">must contain an @</div>\
<div hx-swap-oob=\"innerHTML:#f-total-error\">&lt;b&gt;&amp;&lt;/b&gt;</div>"
    );
    let form = render(
        "<Form id=\"f\" post=\"/x\"><Field name=\"email\" /><Field name=\"total\" /></Form>",
        &json!({}),
    );
    for slot in ["f-email-error", "f-total-error"] {
        assert!(
            form.contains(&format!("<p class=\"sofie-field-error\" id=\"{slot}\"")),
            "{slot}"
        );
    }
}

#[test]
fn a_template_names_the_fields_a_form_has_error_slots_for() {
    let template = template(
        "<Section title=\"S\"><Form id=\"f\" post=\"/x\"><Field name=\"email\" />\
<Field name=\"total\" /><Submit label=\"Go\" /></Form></Section>\
<Action id=\"go\" post=\"/go\" label=\"Go\" />",
    );

    assert_eq!(form_fields(&template, "f"), Some(vec!["email", "total"]));
    assert_eq!(form_fields(&template, "go"), None);
    assert_eq!(form_fields(&template, "missing"), None);
}

#[test]
fn a_form_in_both_branches_names_the_fields_of_each() {
    let template = template(
        "{#if data.a}<Form id=\"f\" post=\"/x\"><Field name=\"a\" /></Form>\
{:else}<Form id=\"f\" post=\"/x\"><Field name=\"b\" /></Form>{/if}",
    );

    assert_eq!(form_fields(&template, "f"), Some(vec!["a", "b"]));
}

#[test]
fn field_errors_without_errors_only_clear_the_form() {
    assert_eq!(
        field_errors("f", &[]),
        "<div hx-swap-oob=\"innerHTML:#f .sofie-field-error\"></div>"
    );
}

#[test]
fn field_errors_skip_names_that_could_widen_a_selector() {
    let x = || "x".to_owned();
    assert_eq!(field_errors("f, body", &[("a", x())]), "");
    assert_eq!(field_errors("", &[("a", x())]), "");
    assert_eq!(
        field_errors("f", &[("a, body", x()), ("a.b", x()), ("", x())]),
        "<div hx-swap-oob=\"innerHTML:#f .sofie-field-error\"></div>"
    );
}

#[test]
fn a_form_is_a_fragment_by_its_id() {
    let source =
        "<Section title=\"S\"><Form id=\"f\" post=\"/x\"><Field name=\"a\" /></Form></Section>";

    let form = fragment(source, &json!({}), "f").unwrap();

    assert!(form.starts_with("<form id=\"f\" method=\"post\""), "{form}");
    assert!(form.ends_with("</form>"), "{form}");
}

#[test]
fn an_out_of_band_fragment_marks_its_root() {
    let source = "<Form id=\"f\" post=\"/x\"><Field name=\"a\" /></Form>\
<Action id=\"go\" post=\"/go\" label=\"Go\" when={data.open} /><p id=\"p\">x</p>";
    let template = template(source);
    let rendered = |id| template.render_out_of_band(&json!({"open": false}), id);

    let form = rendered("f").unwrap();
    assert!(
        form.starts_with("<form hx-swap-oob=\"true\" id=\"f\" method=\"post\""),
        "{form}"
    );
    assert_eq!(
        rendered("p").unwrap(),
        "<p hx-swap-oob=\"true\" id=\"p\">x</p>"
    );
    assert_eq!(rendered("go").unwrap(), "");
    assert!(matches!(
        rendered("nowhere"),
        Err(RenderError::FragmentNotFound { .. })
    ));
}

const SHOP_ORDERS: &str =
    include_str!("../../../examples/shop/modules/order/src/areas/customer/screens/orders.html");

#[test]
fn the_shop_orders_screen_renders_its_form_above_the_table() {
    let data = json!([{
        "id": "a1",
        "status": "PENDING",
        "status_to": {"CANCELLED": true},
        "total": {"minor": 14990, "currency": "EUR"},
    }]);
    let mut screen = template(SHOP_ORDERS);
    screen
        .describe_form(
            "place-order",
            const { &[profile_field("total", MONEY, true)] },
        )
        .unwrap_or_else(|error| panic!("{error}"));
    let html = screen
        .render(&data)
        .unwrap_or_else(|error| panic!("{error}"));
    let form = html.find("<form id=\"place-order\"").expect("the form");
    let table = html.find("<table id=\"orders\">").expect("the table");
    assert!(form < table, "{html}");
    for expected in [
        "action=\"/account/orders\" hx-post=\"/account/orders\" hx-target=\"#orders\" hx-sync=\"this:drop\" hx-swap=\"outerHTML\">",
        "name=\"total.amount\" aria-describedby=\"place-order-total-error\" required>",
        "<input type=\"text\" class=\"sofie-currency\" maxlength=\"3\" aria-label=\"Currency\" name=\"total.currency\">",
        "<button type=\"submit\">Place order</button></form>",
        "<td class=\"sofie-money\">149.90 EUR</td>",
        "hx-post=\"/account/orders/a1/cancel\"",
    ] {
        assert!(html.contains(expected), "missing {expected} in {html}");
    }
    assert!(
        !html.contains("name=\"email\""),
        "the account's email is used: {html}"
    );
    let fragment = common::loaded("order/customer/orders", SHOP_ORDERS)
        .render_fragment(&data, "orders")
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(fragment.starts_with("<table id=\"orders\">") && !fragment.contains("<form"));
}

fn page_with_config() -> String {
    Page {
        title: "Orders",
        lang: "en",
        body: "",
        notice: &[],
        nonce: "n0nce",
        islands: None,
        script_notices: sofie_engine::ScriptNotices::plain(),
    }
    .render()
}

fn response_handling() -> Vec<Value> {
    let page = page_with_config();
    let start = page.find("content='").unwrap() + "content='".len();
    let config: Value =
        serde_json::from_str(&page[start..start + page[start..].find('\'').unwrap()]).unwrap();
    config["responseHandling"].as_array().unwrap().clone()
}

/// The first rule whose code pattern matches `status`, as htmx reads them.
fn handling_of(status: &str) -> Value {
    response_handling()
        .into_iter()
        .find(|rule| {
            let pattern = rule["code"].as_str().unwrap();
            let mut chars = pattern.chars();
            let mut status = status.chars();
            let mut matches = true;
            while let Some(expected) = chars.next() {
                let digit = status.next();
                matches &= match expected {
                    '.' => digit.is_some(),
                    '[' => {
                        let set: String = chars.by_ref().take_while(|c| *c != ']').collect();
                        digit.is_some_and(|digit| set.contains(digit))
                    }
                    other => digit == Some(other),
                };
            }
            matches && status.next().is_none()
        })
        .unwrap_or_else(|| panic!("no rule for {status}"))
}

#[test]
fn expected_refusals_swap_without_counting_as_errors() {
    for status in ["400", "404", "409", "422"] {
        let rule = handling_of(status);
        assert_eq!(rule["swap"], true, "{status}");
        assert!(
            rule.get("error").is_none(),
            "{status} is not an error: {rule}"
        );
    }
    for status in ["500", "503"] {
        assert_eq!(handling_of(status)["error"], true, "{status}");
    }
    assert_eq!(handling_of("204")["swap"], false);
    assert!(handling_of("200").get("error").is_none());
}

#[test]
fn the_page_loads_the_engine_script_under_its_nonce() {
    let script = format!(
        "<script src=\"{}\" nonce=\"n0nce\" defer></script>",
        ENGINE_SCRIPT.path()
    );
    assert!(page_with_config().contains(&script));
}

const PROFILE_FORM: &str = r#"<Form id="profile" post="/profile/{data.id}">
  <Field name="email" value={data.email} />
  <Field name="total" currency="EUR" />
  <Field name="note" value={data.note} />
  <Field name="gift" value={data.gift} />
  <Field name="size" options={data.sizes} />
</Form>
<Form id="search" post="/search"><Field name="email" value={data.email} /></Form>"#;

struct Profile;

impl FormFields for Profile {
    const FIELDS: &'static [FormField] = &[
        profile_field("email", Input::typed(&[("type", "email")]), true),
        profile_field("password", Input::new(Control::Password), false),
        profile_field("total", MONEY, false),
        profile_field("note", Input::new(Control::Textarea), false),
        profile_field("gift", CHECKBOX, false),
        profile_field("size", SELECT, true),
    ];
}

const fn profile_field(name: &'static str, input: Input, required: bool) -> FormField {
    FormField {
        name,
        input,
        required,
        constraints: &[],
    }
}

fn profile_template() -> Template {
    let mut template = template(PROFILE_FORM);
    template
        .describe_form("profile", Profile::FIELDS)
        .unwrap_or_else(|error| panic!("{error}"));
    template
}

fn submitted_page(errors: &[(&str, String)]) -> String {
    let data = json!({ "id": 7, "email": "old@shop.test", "note": "kept", "gift": true, "sizes": ["S", "M"] });
    let input = json!({
        "email": "typed <@>",
        "password": "hunter22",
        "total": { "amount": "1.234\"", "currency": "EUR" },
        "gift": "false",
        "size": "M",
    });
    let submitted = Submitted {
        action: "/profile/7",
        input: &input,
        errors,
    };
    profile_template()
        .render_submitted(&data, &submitted)
        .unwrap_or_else(|error| panic!("{error}"))
}

#[test]
fn a_refused_form_keeps_what_was_typed_but_the_password() {
    let html = submitted_page(&[]);
    let (profile, search) = html.split_at(html.find("<form id=\"search\"").unwrap());

    assert!(profile.contains("name=\"email\" aria-describedby=\"profile-email-error\" required value=\"typed &lt;@&gt;\">"), "{profile}");
    assert!(!profile.contains("hunter22"), "{profile}");
    assert!(
        profile.contains(
            "name=\"total.amount\" aria-describedby=\"profile-total-error\" value=\"1.234&quot;\">"
        ),
        "{profile}"
    );
    assert!(profile.contains("<textarea id=\"profile-note\" name=\"note\" aria-describedby=\"profile-note-error\">\n</textarea>"), "{profile}");
    assert!(!profile.contains(" checked"), "{profile}");
    assert!(
        profile.contains("<option value=\"M\" selected>"),
        "{profile}"
    );
    assert!(
        search.contains("value=\"old@shop.test\""),
        "another form is left alone: {search}"
    );
    assert!(!html.contains("aria-invalid"), "{html}");
}

#[test]
fn a_refused_field_shows_its_message_and_is_marked_invalid() {
    let html = submitted_page(&[
        ("email", "must contain an @".to_owned()),
        ("total", "<at most 2 decimals>".to_owned()),
    ]);

    assert!(
        html.contains("aria-describedby=\"profile-email-error\" required aria-invalid=\"true\""),
        "{html}"
    );
    assert!(
        html.contains("aria-describedby=\"profile-total-error\" aria-invalid=\"true\""),
        "{html}"
    );
    assert_eq!(html.matches("aria-invalid").count(), 2, "{html}");
    assert!(
        html.contains("id=\"profile-email-error\" aria-live=\"polite\">must contain an @</p>"),
        "{html}"
    );
    assert!(
        html.contains("aria-live=\"polite\">&lt;at most 2 decimals&gt;</p>"),
        "{html}"
    );
}

#[test]
fn without_a_refused_post_fields_show_their_bound_values() {
    let html = profile_template()
        .render(&json!({ "id": 7, "email": "old@shop.test", "note": "kept", "gift": true }))
        .unwrap_or_else(|error| panic!("{error}"));

    assert!(html.contains("value=\"old@shop.test\""), "{html}");
    assert!(html.contains(">\nkept</textarea>"), "{html}");
    assert!(html.contains(" checked"), "{html}");
}

#[cfg(not(feature = "i18n"))]
#[test]
fn without_i18n_a_translation_and_a_date_parse_and_number_names_the_feature_it_needs() {
    assert_eq!(
        render("<p>{t(\"orders.title\")}</p>", &json!({})),
        "<p>orders.title</p>"
    );
    assert_eq!(
        render(
            "<p>{date(data.at)}, {date(data.day)}, {date(data.local)}</p>",
            &json!({"at": "2026-10-11T22:30:00.5Z", "day": "2026-10-11", "local": "2026-10-11T09:30"})
        ),
        "<p>2026-10-11 22:30 UTC, 2026-10-11, 2026-10-11T09:30</p>"
    );
    assert_parse_error(
        "<p>{number(data.total)}</p>",
        1,
        5,
        "needs the `i18n` feature",
    );
}

struct SignUp;

impl FormFields for SignUp {
    const FIELDS: &'static [FormField] = &[
        FormField {
            name: "email",
            input: <Email as Present>::INPUT,
            required: true,
            constraints: &[Constraint::MaxLength(254)],
        },
        FormField {
            name: "password",
            input: <Secret<String> as Present>::INPUT,
            required: true,
            constraints: &[Constraint::MinLength(12)],
        },
        FormField {
            name: "nickname",
            input: <Option<String> as Present>::INPUT,
            required: false,
            constraints: &[Constraint::Max(9)],
        },
        FormField {
            name: "deposit",
            input: <Money as Present>::INPUT,
            required: true,
            constraints: &[],
        },
        FormField {
            name: "plan",
            input: Input::select(&["FREE", "PRO"]),
            required: true,
            constraints: &[],
        },
    ];
}

fn described(source: &str) -> Result<Template, TemplateError> {
    let mut template = template(source);
    template
        .describe_form("f", SignUp::FIELDS)
        .map(|()| template)
}

#[test]
fn a_described_field_takes_its_control_from_its_value_type() {
    let source = "<Section title=\"S\">{#if data.open}<Form id=\"f\" post=\"/x\">\
<Field name=\"email\" /><Field name=\"password\" />\
<Field name=\"nickname\" required /><Field name=\"deposit\" currency=\"EUR\" />\
<Field name=\"plan\" /></Form>{/if}</Section>";
    let html = described(source)
        .unwrap_or_else(|error| panic!("{error}"))
        .render(&json!({"open": true}))
        .unwrap_or_else(|error| panic!("{error}"));

    for control in [
        "<input type=\"email\" autocomplete=\"email\" id=\"f-email\" name=\"email\" \
aria-describedby=\"f-email-error\" required maxlength=\"254\">",
        "<input type=\"password\" id=\"f-password\" name=\"password\" \
aria-describedby=\"f-password-error\" required minlength=\"12\">",
        "<input type=\"text\" id=\"f-nickname\" name=\"nickname\" \
aria-describedby=\"f-nickname-error\" max=\"9\">",
        "<input type=\"text\" inputmode=\"decimal\" id=\"f-deposit\" name=\"deposit.amount\" \
aria-describedby=\"f-deposit-error\" required><span class=\"sofie-currency\">EUR</span>",
        "<select id=\"f-plan\" name=\"plan\" aria-describedby=\"f-plan-error\" required>\
<option value=\"FREE\">FREE</option><option value=\"PRO\">PRO</option></select>",
    ] {
        assert!(html.contains(control), "{control} in {html}");
    }
}

#[test]
fn a_description_is_checked_against_the_form() {
    let refusal = |source: &str| match described(source) {
        Ok(_) => panic!("{source} was described"),
        Err(error) => error.message,
    };

    assert!(refusal("<p>x</p>").contains("no `Form` with id `f`"));
    assert!(
        refusal("<Form id=\"f\" post=\"/x\"><Field name=\"age\" /></Form>")
            .contains("`Field` `age` is not a field of the input form `f` posts; its fields are email, password, nickname, deposit, plan")
    );
    assert!(
        refusal("<Form id=\"f\" post=\"/x\"><Field name=\"email\" currency=\"EUR\" /></Form>")
            .contains("no prop `currency`")
    );
    assert!(
        refusal("<Form id=\"f\" post=\"/x\"><Field name=\"password\" value={data.p} /></Form>")
            .contains("a `password` field takes no `value`")
    );
}

#[cfg(feature = "layout")]
#[test]
fn layout_components_frame_their_children() {
    let html = render(
        "<Grid id=\"products\">{#each data.products as product}\
<Card href=\"/products/{product.slug}\"><Image src={product.image} alt={product.name} />\
<Money value={product.price} /></Card>{/each}<Card><p>More soon</p></Card></Grid>",
        &json!({"products": [{
            "slug": "a b", "image": "javascript:x", "name": "<Tea>",
            "price": {"minor": 450, "currency": "EUR"}
        }]}),
    );

    assert_eq!(
        html,
        "<div class=\"sofie-grid\" id=\"products\"><a class=\"sofie-card\" href=\"/products/a%20b\">\
<img class=\"sofie-image\" src=\"#\" alt=\"&lt;Tea&gt;\" loading=\"lazy\">\
<span class=\"sofie-money\">4.50 EUR</span></a><div class=\"sofie-card\"><p>More soon</p></div></div>"
    );
    assert_parse_error("<Image src=\"/a.png\" />", 1, 1, "`alt` prop");
}

#[cfg(feature = "layout")]
#[test]
fn a_linked_card_refuses_interactive_children() {
    for inside in [
        "<a href=\"/b\">B</a>",
        "{#if data.open}<button>Buy</button>{/if}",
        "<Grid><Card href=\"/c\">C</Card></Grid>",
        "<Action label=\"Buy\" post=\"/buy\" />",
    ] {
        assert_parse_error(
            &format!("<Card href=\"/a\">{inside}</Card>"),
            1,
            1,
            "inside a `Card` with `href`",
        );
    }
    render(
        "<Card><a href=\"/b\">B</a></Card><Card href=\"/a\"><abbr>A</abbr></Card>",
        &json!({}),
    );
}
