mod common;

use serde_json::json;
use sofie_engine::Template;

fn render(source: &str, data: &serde_json::Value) -> String {
    common::loaded("slots", source)
        .render(data)
        .unwrap_or_else(|error| panic!("{error}"))
}

fn parse_error(source: &str) -> String {
    Template::parse("slots", source)
        .expect_err("the template is invalid")
        .to_string()
}

#[test]
fn an_action_in_a_loop_posts_each_item_s_fields_as_hidden_inputs() {
    let source = "{#each data.slots as slot}<Action id=\"book-{slot.id}\" post=\"/slots/book\" \
        label=\"Book\" with={{ slot: slot.id, note: \"a&b\" }} />{/each}";
    let data = json!({"slots": [{"id": 7}, {"id": "x\"<y>"}]});

    assert_eq!(
        render(source, &data),
        "<form id=\"book-7\" method=\"post\" action=\"/slots/book\" hx-post=\"/slots/book\" \
         hx-swap=\"outerHTML\"><input type=\"hidden\" name=\"slot\" value=\"7\"><input \
         type=\"hidden\" name=\"note\" value=\"a&amp;b\"><button type=\"submit\">Book</button>\
         </form><form id=\"book-x&quot;&lt;y&gt;\" method=\"post\" action=\"/slots/book\" \
         hx-post=\"/slots/book\" hx-swap=\"outerHTML\"><input type=\"hidden\" name=\"slot\" \
         value=\"x&quot;&lt;y&gt;\"><input type=\"hidden\" name=\"note\" value=\"a&amp;b\">\
         <button type=\"submit\">Book</button></form>"
    );
}

#[test]
fn an_action_in_a_table_row_posts_its_row_and_hides_where_its_condition_fails() {
    let source = "<Table id=\"slots\" of={data.slots} columns=\"time\" as=\"slot\"><Action \
        post=\"/slots/{slot.id}/hold\" label=\"Hold\" target=\"slots\" with={{slot: slot.id, \
        seats: 2}} when={slot.open} /></Table>";
    let data = json!({"slots": [
        {"id": 1, "time": "09:00", "open": true},
        {"id": 2, "time": "10:00", "open": false},
    ]});

    let html = render(source, &data);

    assert!(
        html.contains(
            "<td class=\"sofie-actions\"><form method=\"post\" action=\"/slots/1/hold\" \
             hx-post=\"/slots/1/hold\" hx-target=\"#slots\" hx-sync=\"this:drop\" \
             hx-swap=\"outerHTML\"><input type=\"hidden\" name=\"slot\" value=\"1\"><input \
             type=\"hidden\" name=\"seats\" value=\"2\"><button type=\"submit\">Hold</button>\
             </form></td>"
        ),
        "{html}"
    );
    assert!(!html.contains("/slots/2/hold"), "{html}");
}

#[test]
fn an_action_without_fields_stays_a_button() {
    assert_eq!(
        render("<Action post=\"/go\" label=\"Go\" />", &json!({})),
        "<button type=\"button\" hx-post=\"/go\" hx-swap=\"outerHTML\">Go</button>"
    );
}

#[test]
fn posted_fields_are_data_named_once_and_only_on_an_action() {
    for (source, message) in [
        (
            "<Action post=\"/x\" label=\"x\" with={{slot: data.a, slot: data.b}} />",
            "names the field `slot` twice",
        ),
        (
            "<Action post=\"/x\" label=\"x\" with={{slot: data.a}} with={{b: data.b}} />",
            "`with` is given twice",
        ),
        (
            "<Action post=\"/x\" label=\"x\" with={{: data.a}} />",
            "expected a field name in `with`",
        ),
        (
            "<Action post=\"/x\" label=\"x\" with={{slot: data.a} />",
            "expected `}}` after the fields",
        ),
        (
            "{#each data.slots as slot}<Action post=\"/x\" label=\"x\" with={{slot: slt.id}} />{/each}",
            "unknown name `slt`",
        ),
        (
            "<p with={{slot: data.a}}></p>",
            "posted fields belong on an `Action`",
        ),
        (
            "<Section title=\"x\" with={{slot: data.a}}></Section>",
            "`Section` has no prop `with`",
        ),
    ] {
        let error = parse_error(source);
        assert!(error.contains(message), "{source}: {error}");
    }
}

#[test]
fn a_literal_id_or_a_form_repeats_in_a_loop_so_both_are_refused() {
    let literal_id = parse_error(
        "{#each data.slots as slot}<Action id=\"book\" post=\"/x\" label=\"x\" \
         with={{slot: slot.id}} />{/each}",
    );
    let form = parse_error(
        "{#each data.slots as slot}<Form id=\"book\" post=\"/x\"><Submit label=\"x\" />\
         </Form>{/each}",
    );

    assert!(literal_id.contains("would repeat"), "{literal_id}");
    assert!(form.contains("with an `Action`"), "{form}");
}
