#![cfg(feature = "islands")]

mod common;

use serde_json::json;
use sofie_engine::islands::{IslandBundle, IslandLinks};
use sofie_engine::{Page, Template};

const SCREEN: &str = r#"<section><h2>Orders</h2><Island id="chart" name="order-chart" props={data.summary} /></section>"#;

#[test]
fn an_island_mounts_with_the_bound_value_as_its_props() {
    let template = common::loaded("order/member/orders", SCREEN);
    let data = json!({"summary": {"count": 2, "note": "</script>"}});

    let html = template.render(&data).unwrap();

    assert_eq!(
        html,
        "<section><h2>Orders</h2><order-chart id=\"chart\"><script type=\"application/json\">\
         {\"count\":2,\"note\":\"\\u003c/script\\u003e\"}</script></order-chart></section>"
    );
    let fragment = template.render_fragment(&data, "chart").unwrap();
    assert!(
        fragment.starts_with("<order-chart id=\"chart\">"),
        "{fragment}"
    );
}

#[test]
fn props_the_data_lacks_are_an_empty_object() {
    let template = common::loaded("t", r#"<Island name="order-chart" props={data.none} />"#);

    assert_eq!(
        template.render(&json!({})).unwrap(),
        "<order-chart><script type=\"application/json\">{}</script></order-chart>"
    );
}

#[test]
fn an_island_needs_a_custom_element_name_and_its_props() {
    for (source, expected) in [
        (
            r#"<Island name="chart" props={data} />"#,
            "`chart` is no island name",
        ),
        (
            r#"<Island name="{data.tag}" props={data} />"#,
            "needs `name` as a quoted literal",
        ),
        (r#"<Island name="order-chart" />"#, "needs the `props` prop"),
    ] {
        let error = Template::parse("t", source).unwrap_err().to_string();
        assert!(error.contains(expected), "{source}: {error}");
    }
}

#[test]
fn a_page_with_islands_loads_its_areas_bundle() {
    let bundle = IslandBundle::new("member", b"export {};", b"");
    let links = IslandLinks::of(&bundle);
    let page = Page {
        title: "Orders",
        lang: "en",
        body: "<p>x</p>",
        notice: &[],
        nonce: "n0nce",
        islands: Some(&links),
        script_notices: sofie_engine::ScriptNotices::plain(),
    };

    let html = page.render();

    let script = format!(
        "<script type=\"module\" src=\"{}\" nonce=\"n0nce\" data-sofie-islands></script></head>",
        links.script
    );
    assert!(html.contains(&script), "{html}");
    assert!(html.contains(&format!(
        "<link rel=\"stylesheet\" href=\"{}\" data-sofie-islands>",
        links.stylesheet
    )));
    let plain = Page {
        islands: None,
        ..page
    };
    assert!(!plain.render().contains("data-sofie-islands"));
}

#[cfg(feature = "parse")]
#[test]
fn an_island_screen_is_the_island_parsed() {
    let data = json!({"count": 1});
    let built = sofie_engine::islands::island_screen("t", "order-chart").unwrap();
    let parsed = Template::parse("t", r#"<Island name="order-chart" props={data} />"#).unwrap();

    assert_eq!(built.render(&data).unwrap(), parsed.render(&data).unwrap());
    assert!(sofie_engine::islands::island_screen("t", "Chart").is_err());
}
