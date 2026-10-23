use serde_json::json;
use sofie_core::show::Fields;
use sofie_core::{DisplayZone, Instant, Show, Shown};
use sofie_engine::{Submitted, Template};
use sofie_values::{FormField, Input};

const FIELDS: &[FormField] = &[FormField {
    name: "at",
    input: Input::typed(&[("type", "datetime-local")]),
    required: false,
    constraints: &[],
}];

fn install_berlin() {
    DisplayZone::named("Europe/Berlin")
        .unwrap()
        .install()
        .unwrap();
}

fn form() -> Template {
    install_berlin();
    let mut template = Template::parse(
        "edit",
        "<Form id=\"f\" post=\"/x\"><Field name=\"at\" value={data.at} /></Form>",
    )
    .unwrap();
    template.describe_form("f", FIELDS).unwrap();
    template
}

fn first_input(html: &str) -> String {
    let start = html.find("<input").unwrap();
    let end = html[start..].find('>').unwrap() + start + 1;
    html[start..end].to_owned()
}

fn rendered_control(at: &str) -> String {
    first_input(&form().render(&json!({ "at": at })).unwrap())
}

/// As shown again after another field of the form failed.
fn reposted_control(posted: &str) -> String {
    let input = json!({ "at": posted });
    let errors = [("other", "is taken".to_owned())];
    let submitted = Submitted {
        action: "/x",
        input: &input,
        errors: &errors,
    };
    let html = form()
        .render_submitted(&json!({ "at": null }), &submitted)
        .unwrap();
    first_input(&html)
}

fn posted_value(control: &str) -> &str {
    let start = control.find(" value=\"").unwrap() + 8;
    let end = control[start..].find('"').unwrap() + start;
    &control[start..end]
}

#[test]
fn an_instant_in_a_repeated_hour_is_shown_with_its_offset_and_posts_back_unchanged() {
    for (at, shown) in [
        ("2026-10-25T00:30:00Z", "2026-10-25T02:30:00+02:00"),
        ("2026-10-25T01:30:00Z", "2026-10-25T02:30:00+01:00"),
    ] {
        let control = rendered_control(at);

        assert!(control.starts_with("<input type=\"text\" "), "{control}");
        assert_eq!(posted_value(&control), shown);
        assert_eq!(
            Instant::parse(posted_value(&control)).unwrap(),
            Instant::parse(at).unwrap()
        );
    }
}

#[test]
fn an_instant_outside_a_repeated_hour_keeps_its_date_time_control() {
    let control = rendered_control("2026-10-25T02:30:00Z");

    assert!(
        control.starts_with("<input type=\"datetime-local\" "),
        "{control}"
    );
    assert_eq!(posted_value(&control), "2026-10-25T03:30");
    assert_eq!(
        Instant::parse(posted_value(&control)).unwrap(),
        Instant::parse("2026-10-25T02:30:00Z").unwrap()
    );
}

#[test]
fn an_instant_finer_than_milliseconds_is_shown_exactly_with_its_offset() {
    let at = "2026-10-11T22:30:15.123456Z";

    let control = rendered_control(at);

    assert!(control.starts_with("<input type=\"text\" "), "{control}");
    assert_eq!(posted_value(&control), "2026-10-12T00:30:15.123456+02:00");
    assert_eq!(
        Instant::parse(posted_value(&control)).unwrap(),
        Instant::parse(at).unwrap()
    );
    let milliseconds = rendered_control("2026-10-11T22:30:15.123Z");
    assert!(milliseconds.contains(" step=\"any\""), "{milliseconds}");
    assert_eq!(posted_value(&milliseconds), "2026-10-12T00:30:15.123");
}

#[test]
fn a_posted_instant_shown_again_keeps_the_control_its_text_needs() {
    let offset = reposted_control("2026-10-25T02:30:00+01:00");
    let seconds = reposted_control("2026-10-12T00:30:15");
    let minutes = reposted_control("2026-10-12T00:30");

    assert!(offset.starts_with("<input type=\"text\" "), "{offset}");
    assert_eq!(posted_value(&offset), "2026-10-25T02:30:00+01:00");
    assert!(seconds.contains(" step=\"any\""), "{seconds}");
    assert_eq!(posted_value(&seconds), "2026-10-12T00:30:15");
    assert!(
        minutes.starts_with("<input type=\"datetime-local\" ") && !minutes.contains("step"),
        "{minutes}"
    );
    assert_eq!(posted_value(&minutes), "2026-10-12T00:30");
    let repeated = reposted_control("2026-10-25T02:30");
    assert!(repeated.starts_with("<input type=\"text\" "), "{repeated}");
}

/// Text that is no wall clock keeps a text box, which a `datetime-local` control would empty.
#[test]
fn posted_text_that_is_no_wall_clock_is_shown_again_in_a_text_box() {
    for posted in [
        "yesterday",
        "2026-10-25T02:30:00+25:00",
        "2026-10-12T00:30:15.1234",
    ] {
        let control = reposted_control(posted);

        assert!(control.starts_with("<input type=\"text\" "), "{control}");
        assert_eq!(posted_value(&control), posted);
    }
}

struct Row {
    at: Instant,
}

impl Show for Row {
    fn show(&self) -> Shown<'_> {
        Shown::Record(self)
    }
}

impl Fields for Row {
    fn name(&self, index: usize) -> Option<&str> {
        ["at"].get(index).copied()
    }

    fn get(&self, name: &str) -> Option<Shown<'_>> {
        (name == "at").then(|| self.at.show())
    }
}

fn rows_around_berlin_s_repeated_hour() -> Vec<Row> {
    install_berlin();
    [
        "2026-10-25T00:30:00Z",
        "2026-10-25T01:30:00Z",
        "2026-10-25T02:30:00Z",
    ]
    .into_iter()
    .map(|at| Row {
        at: Instant::parse(at).unwrap(),
    })
    .collect()
}

#[test]
fn a_table_cell_in_a_repeated_hour_writes_the_offset_before_the_zone() {
    let rows = rows_around_berlin_s_repeated_hour();
    let template = Template::parse("t", "<Table id=\"t\" of={data} columns=\"at\" />").unwrap();

    let html = template.render(&rows).unwrap();

    assert!(
        html.contains(
            "<td>2026-10-25 02:30 +02:00 Europe/Berlin</td></tr><tr><td>2026-10-25 02:30 +01:00 \
             Europe/Berlin</td></tr><tr><td>2026-10-25 03:30 Europe/Berlin</td>"
        ),
        "{html}"
    );
}

#[cfg(not(feature = "i18n"))]
#[test]
fn without_i18n_date_writes_an_instant_as_a_table_cell_does() {
    let rows = rows_around_berlin_s_repeated_hour();
    let template = Template::parse("t", "{#each data as row}<p>{date(row.at)}</p>{/each}").unwrap();

    let html = template.render(&rows).unwrap();

    assert_eq!(
        html,
        "<p>2026-10-25 02:30 +02:00 Europe/Berlin</p><p>2026-10-25 02:30 +01:00 Europe/Berlin</p>\
         <p>2026-10-25 03:30 Europe/Berlin</p>"
    );
}
