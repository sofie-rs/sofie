#![cfg(feature = "listing")]

mod common;

use serde_json::json;
use sofie_engine::Template;

const SCREEN: &str =
    r#"<Table id="orders" of={data} columns="email total" sort="total" filter="email" />"#;

#[test]
fn a_page_links_to_its_neighbours_keeping_its_order_and_filters() {
    let template = common::loaded("orders", SCREEN);
    let page = json!({
        "items": [{"email": "a@b.c", "total": 3}],
        "page": 2,
        "size": 20,
        "has_next": true,
        "sort": "total",
        "filter": {"email": "a b"},
    });

    let html = template.render(&page).unwrap();

    assert!(html.starts_with("<div class=\"sofie-listing\" id=\"orders\"><form class=\"sofie-filter\" method=\"get\" hx-get=\"?\" hx-target=\"#orders\""), "{html}");
    assert!(html.contains("name=\"email\" value=\"a b\""), "{html}");
    assert!(
        html.contains(
            "<th scope=\"col\" aria-sort=\"ascending\"><a href=\"?sort=-total&amp;email=a%20b\""
        ),
        "a sorted column links to the other order: {html}"
    );
    assert!(html.contains("<td>a@b.c</td><td>3</td>"), "{html}");
    assert!(
        html.contains("<a href=\"?sort=total&amp;email=a%20b\" hx-get=\"?sort=total&amp;email=a%20b\" hx-target=\"#orders\" hx-swap=\"outerHTML\" hx-push-url=\"true\">Previous</a><span>2</span><a href=\"?page=3&amp;sort=total&amp;email=a%20b\""),
        "{html}"
    );
    assert_eq!(template.render_fragment(&page, "orders").unwrap(), html);
}

#[test]
fn a_page_keeps_its_loader_s_query_parameters_on_every_link_and_in_its_filter_form() {
    let template = common::loaded("orders", SCREEN);
    let page = json!({
        "items": [{"email": "a@b.c", "total": 3}],
        "page": 2,
        "size": 20,
        "has_next": true,
        "query": {"near": "Ulm Mitte"},
        "filter": {"email": "a"},
    });

    let html = template.render(&page).unwrap();

    assert!(
        html.contains("<input type=\"hidden\" name=\"near\" value=\"Ulm Mitte\"><button"),
        "{html}"
    );
    assert!(
        html.contains("<a href=\"?sort=total&amp;near=Ulm%20Mitte&amp;email=a\""),
        "{html}"
    );
    assert!(
        html.contains("<a href=\"?near=Ulm%20Mitte&amp;email=a\""),
        "{html}"
    );
    assert!(
        html.contains("<a href=\"?page=3&amp;near=Ulm%20Mitte&amp;email=a\""),
        "{html}"
    );
}

#[test]
fn a_page_of_another_size_keeps_it_on_every_link_and_in_its_filter_form() {
    let template = common::loaded("orders", SCREEN);
    let page = json!({
        "items": [{"email": "a@b.c", "total": 3}],
        "page": 2,
        "size": 50,
        "has_next": true,
        "sort": "total",
    });

    let html = template.render(&page).unwrap();

    assert!(
        html.contains(
            "<input type=\"hidden\" name=\"size\" value=\"50\"><input type=\"hidden\" name=\"sort\""
        ),
        "{html}"
    );
    assert!(
        html.contains("<a href=\"?size=50&amp;sort=-total\""),
        "{html}"
    );
    assert!(
        html.contains("<a href=\"?size=50&amp;sort=total\""),
        "{html}"
    );
    assert!(
        html.contains("<a href=\"?page=3&amp;size=50&amp;sort=total\""),
        "{html}"
    );
}

#[test]
fn a_table_sorts_only_by_its_own_columns() {
    let error = Template::parse(
        "orders",
        r#"<Table id="orders" of={data} columns="email" sort="total" />"#,
    )
    .unwrap_err();

    assert!(error.message.contains("sorts by `total`"), "{error}");
}

#[test]
fn a_plain_list_stays_a_plain_table() {
    let template = common::loaded(
        "orders",
        r#"<Table id="orders" of={data} columns="email" />"#,
    );

    let html = template.render(&json!([{"email": "a@b.c"}])).unwrap();

    assert!(html.starts_with("<table id=\"orders\"><thead>"), "{html}");
}

#[test]
fn the_last_possible_page_links_to_no_next_one() {
    let template = common::loaded("orders", SCREEN);
    let page = json!({
        "items": [{"email": "a@b.c", "total": 3}],
        "page": u64::MAX,
        "size": 20,
        "has_next": true,
    });

    let html = template.render(&page).unwrap();

    assert!(!html.contains("page=0"), "{html}");
    assert!(html.contains(&format!("page={}", u64::MAX - 1)), "{html}");
}

#[test]
fn an_instant_filter_given_with_an_offset_is_a_text_box_holding_it() {
    let template = common::loaded(
        "bookings",
        r#"<Table id="bookings" of={data} columns="starts_at" filter="starts_at" />"#,
    );
    let page = json!({
        "items": [],
        "filter": {"starts_at": "2026-10-25T02:30:00+01:00"},
        "controls": {"starts_at": {"type": "datetime-local"}},
    });

    let html = template.render(&page).unwrap();

    assert!(
        html.contains(
            "<input type=\"text\" name=\"starts_at\" value=\"2026-10-25T02:30:00+01:00\">"
        ),
        "{html}"
    );
}

#[test]
fn a_refused_instant_filter_is_a_text_box_with_its_error_beside_it() {
    let template = common::loaded(
        "bookings",
        r#"<Table id="bookings" of={data} columns="starts_at" filter="starts_at" />"#,
    );
    let page = json!({
        "items": [],
        "filter": {"starts_at": "2026-10-25T02:30"},
        "controls": {"starts_at": {"type": "datetime-local"}},
        "errors": {"starts_at": "is repeated: <+02:00> or <+01:00>"},
    });

    let html = template.render(&page).unwrap();

    assert!(
        html.contains(
            "<input type=\"text\" name=\"starts_at\" value=\"2026-10-25T02:30\" \
             aria-invalid=\"true\"><span class=\"sofie-field-error\">is repeated: \
             &lt;+02:00&gt; or &lt;+01:00&gt;</span></label>"
        ),
        "{html}"
    );
}

/// An operator filter, or a Table without `filter`, has no slot: its error is told above the table.
#[test]
fn an_error_with_no_filter_slot_is_told_above_the_table() {
    let page = json!({
        "items": [],
        "page": 1,
        "filter": {"total.gt": "a<b", "status": "x"},
        "errors": {"total.gt": "must be a <number>", "status": "is no status"},
    });
    let unfiltered = common::loaded(
        "orders",
        r#"<Table id="orders" of={data} columns="status total" />"#,
    );
    let filtered = common::loaded(
        "orders",
        r#"<Table id="orders" of={data} columns="status total" filter="status" />"#,
    );

    let bare = unfiltered.render(&page).unwrap();
    let with_form = filtered.render(&page).unwrap();

    let told =
        "<p class=\"sofie-field-error\" role=\"alert\">total.gt: must be a &lt;number&gt;</p>";
    assert!(
        bare.contains(&format!(
            "<div class=\"sofie-listing\" id=\"orders\">\
         <p class=\"sofie-field-error\" role=\"alert\">status: is no status</p>{told}<table>"
        )),
        "{bare}"
    );
    assert!(with_form.contains(&format!("{told}<form")), "{with_form}");
    assert!(!with_form.contains("status: is no status"), "{with_form}");
}

/// The list names what each field holds: an enum's variants, or an instant.
#[test]
fn a_filter_offers_an_enum_s_variants_and_a_date_time_picker() {
    let template = common::loaded(
        "bookings",
        r#"<Table id="bookings" of={data} columns="status starts_at" filter="status starts_at" />"#,
    );
    let page = json!({
        "items": [],
        "page": 1,
        "size": 20,
        "has_next": false,
        "sort": null,
        "filter": {"status": "PAID", "starts_at": "2026-10-20T09:30"},
        "controls": {
            "status": {"values": ["OPEN", "PAID"]},
            "starts_at": {"type": "datetime-local"},
        },
    });

    let html = template.render(&page).unwrap();

    assert!(
        html.contains(
            "<select name=\"status\"><option value=\"\"></option><option value=\"OPEN\">OPEN</option>\
             <option value=\"PAID\" selected>PAID</option></select>"
        ),
        "{html}"
    );
    assert!(
        html.contains(
            "<input type=\"datetime-local\" name=\"starts_at\" value=\"2026-10-20T09:30\">"
        ),
        "{html}"
    );
}

#[cfg(all(feature = "clients", feature = "i18n"))]
#[test]
fn a_client_area_s_refused_filters_are_written_in_the_viewer_s_language() {
    use sofie_engine::Translations;
    use sofie_engine::rendering::Rendering;
    let template = common::loaded("orders", SCREEN);
    let translations = Translations::new(&["en", "tr"]).unwrap();
    let mut data = json!({
        "items": [],
        "errors": {"email": {
            "key": "validation.max",
            "arguments": {"max": "5"},
            "message": "must be at most 5",
        }},
    });

    Rendering::new(&template)
        .language(translations.negotiate(None, Some("tr")))
        .translate_listing_errors(&mut data);

    assert_eq!(
        data["errors"]["email"],
        json!({
            "key": "validation.max",
            "arguments": {"max": "5"},
            "message": "en fazla 5 olmalı",
        })
    );
}
