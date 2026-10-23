use sofie::core::show::Fields;
use sofie::core::{Instant, Shown, Uuid};
use sofie::db::{Db, Id, Meta, Outcome, Reply};
use sofie::engine::Template;
use sofie::http::{App, Routes, Web};
use sofie::serde_json;
use sofie::values::{Currency, FormField, Input, Money, Present};
use sofie::{Aggregate, Enum, ReadModel, Value};
use sofie_testing::{FakeDriver, TestServer};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
enum Status {
    #[to(Paid, Cancelled, Expired)]
    Pending,
    #[to(Shipped, Refunded)]
    Paid,
    Shipped,
    Cancelled,
    Refunded,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Value)]
struct Size {
    width: i64,
    label: String,
}

impl Size {
    fn try_new(width: i64, label: String) -> Result<Self, &'static str> {
        if width < 0 {
            return Err("a width cannot be negative");
        }
        Ok(Self { width, label })
    }
}

#[derive(Aggregate)]
struct Parcel {
    meta: Meta,
    status: Status,
    total: Money,
    discount: Option<Money>,
    note: Option<String>,
    size: Size,
    ratio: f64,
    placed_at: Instant,
}

#[derive(ReadModel)]
#[read(from = Parcel)]
struct ParcelRow {
    id: Id<Parcel>,
    status: Status,
    total: Money,
    discount: Option<Money>,
    note: Option<String>,
    size: Size,
    ratio: f64,
    placed_at: Instant,
}

fn euros(minor: i64) -> Money {
    Money::new(minor, Currency::parse("EUR").unwrap())
}

fn row(status: Status, note: Option<&str>, discount: Option<Money>) -> ParcelRow {
    ParcelRow {
        id: Id::from_uuid(Uuid::new_v4().unwrap()),
        status,
        total: euros(14990),
        discount,
        note: note.map(str::to_owned),
        size: Size::try_new(3, "<big> & \"wide\"".to_owned()).unwrap(),
        ratio: 0.25,
        placed_at: Instant::parse("2026-10-11T09:30:15.5Z").unwrap(),
    }
}

fn rows() -> Reply {
    vec![
        row(Status::Pending, Some("ring <twice>"), Some(euros(500))),
        row(Status::Shipped, None, None),
        row(Status::Paid, Some(""), None),
    ]
    .reply()
    .unwrap()
}

fn renders_alike(template: &Template, reply: &Reply) -> String {
    assert!(reply.shown().is_some(), "derived data is shown typed");
    let typed = template.render(reply);
    let json = template.render(&reply.to_json().unwrap());
    let typed = typed.unwrap();
    assert_eq!(typed, json.unwrap());
    typed
}

#[test]
fn the_shop_orders_screen_renders_typed_rows_as_it_renders_their_json() {
    let source =
        include_str!("../../../examples/shop/modules/order/src/areas/customer/screens/orders.html");
    let template = Template::parse("orders", source).unwrap();

    let html = renders_alike(&template, &rows());

    assert_eq!(html.matches("/cancel").count(), 1, "{html}");
    assert!(html.contains("149.90 EUR"), "{html}");
    renders_alike(&template, &Vec::<ParcelRow>::new().reply().unwrap());
}

#[test]
fn records_lists_options_money_and_transitions_render_alike() {
    let template = Template::parse(
        "parcels",
        "{#each data as parcel, position}<p id=\"p-{parcel.id}\">{position} {parcel.status} \
         {parcel.status_to.CANCELLED}{#if parcel.status_to}moves{:else}stays{/if} \
         {parcel.size.width}x{parcel.size.label} {parcel.note}{#if parcel.note}noted{/if} \
         {parcel.ratio} {parcel.placed_at} {parcel.total.minor}{parcel.total.currency} \
         {parcel.size} {parcel.missing.deeper}{#if parcel.discount}off{/if}</p>\
         <Money value={parcel.total} /><Money value={parcel.discount} />\
         <a href=\"/parcels/{parcel.id}/{parcel.size.label}?at={parcel.placed_at}\">open</a>{/each}\
         <Table id=\"t\" of={data} columns=\"id status total discount note size ratio\" />",
    )
    .unwrap();

    let html = renders_alike(&template, &rows());

    assert!(html.contains("0 PENDING true"), "{html}");
    assert!(
        html.contains("3x&lt;big&gt; &amp; &quot;wide&quot;"),
        "{html}"
    );
}

#[test]
fn a_table_writes_a_typed_instant_on_the_wall_clock_and_its_json_text_as_it_is() {
    let template = Template::parse(
        "parcels",
        "<Table id=\"t\" of={data} columns=\"placed_at\" />",
    )
    .unwrap();
    let reply = rows();

    let typed = template.render(&reply).unwrap();
    let json = template.render(&reply.to_json().unwrap()).unwrap();

    assert!(typed.contains("<td>2026-10-11 09:30 UTC</td>"), "{typed}");
    assert!(json.contains("<td>2026-10-11T09:30:15.5Z</td>"), "{json}");
}

#[test]
fn a_fragment_is_found_by_a_typed_id() {
    let template = Template::parse(
        "parcel",
        "{#each data as parcel}<div id={parcel.id}>{parcel.status}</div>{/each}",
    )
    .unwrap();
    let reply = rows();
    let Some(Shown::List(list)) = reply.shown() else {
        panic!("a list of rows is shown as a list");
    };
    let Some(Shown::Record(first)) = list.item(0) else {
        panic!("a row is shown as a record");
    };
    let Some(Shown::Uuid(id)) = first.get("id") else {
        panic!("an id is shown as one");
    };

    let typed = template.render_fragment(&reply, &id.to_string()).unwrap();

    assert_eq!(
        typed,
        template
            .render_fragment(&reply.to_json().unwrap(), &id.to_string())
            .unwrap()
    );
    assert!(typed.ends_with(">PENDING</div>"), "{typed}");
}

#[test]
fn bound_form_fields_read_typed_values_as_their_json() {
    static FIELDS: [FormField; 4] = [
        FormField {
            name: "placed_at",
            input: Input::typed(&[("type", "datetime-local")]),
            required: false,
            constraints: &[],
        },
        FormField {
            name: "total",
            input: Money::INPUT,
            required: true,
            constraints: &[],
        },
        FormField {
            name: "status",
            input: Status::INPUT,
            required: true,
            constraints: &[],
        },
        FormField {
            name: "note",
            input: Input::typed(&[("type", "text")]),
            required: false,
            constraints: &[],
        },
    ];
    let mut template = Template::parse(
        "edit",
        "<Form id=\"f\" post=\"/parcels/{data.id}\"><Field name=\"placed_at\" value={data.placed_at} />\
         <Field name=\"total\" value={data.total} /><Field name=\"status\" value={data.status} />\
         <Field name=\"note\" value={data.note} /><Submit label=\"Save\" /></Form>",
    )
    .unwrap();
    template.describe_form("f", &FIELDS).unwrap();

    let html = renders_alike(
        &template,
        &row(Status::Paid, Some("<n>"), None).reply().unwrap(),
    );

    assert!(html.contains("value=\"2026-10-11T09:30:15.5\""), "{html}");
    assert!(html.contains("value=\"149.90\""), "{html}");
    assert!(html.contains("<option value=\"PAID\" selected>"), "{html}");
}

#[test]
fn a_read_model_shows_the_fields_its_json_carries() {
    for status in [Status::Pending, Status::Shipped] {
        let parcel = row(status, None, None);
        let json = sofie::serde_json::to_value(&parcel).unwrap();
        let mut shown: Vec<&str> = (0..)
            .map_while(|index| parcel.name(index))
            .filter(|name| parcel.get(name).is_some())
            .collect();

        let mut keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        shown.sort_unstable();
        keys.sort_unstable();
        assert_eq!(shown, keys, "{status:?}");
    }
}

#[derive(ReadModel)]
struct StatusCount {
    status: Status,
    count: u64,
}

#[derive(ReadModel)]
struct ParcelSummary {
    by_status: Vec<StatusCount>,
    total: Money,
}

#[test]
fn a_computed_read_model_renders_as_its_json() {
    let template = Template::parse(
        "summary",
        "{#each data.by_status as line}<p>{line.status} {line.count}</p>{/each}\
         <Money value={data.total} />",
    )
    .unwrap();
    let summary = ParcelSummary {
        by_status: vec![
            StatusCount {
                status: Status::Paid,
                count: 3,
            },
            StatusCount {
                status: Status::Pending,
                count: u64::MAX,
            },
        ],
        total: euros(24550),
    };

    let html = renders_alike(&template, &summary.reply().unwrap());

    assert!(html.contains("<p>PAID 3</p>"), "{html}");
    assert!(html.contains(&format!("PENDING {}", u64::MAX)), "{html}");
    assert!(html.contains("245.50 EUR"), "{html}");
}

async fn parcel_summary() -> sofie::core::Result<ParcelSummary> {
    Ok(ParcelSummary {
        by_status: vec![StatusCount {
            status: Status::Paid,
            count: 3,
        }],
        total: euros(24550),
    })
}

#[tokio::test]
async fn a_screen_answers_a_computed_read_model_as_a_page_and_as_json() {
    let screens = sofie::http::screens_from_sources(&[(
        "shop/web/summary",
        "{#each data.by_status as line}<p>{line.status} {line.count}</p>{/each}",
    )])
    .unwrap();
    let app = App::new(Db::new(FakeDriver::default()))
        .screens(screens)
        .area("shop", |routes: &mut Routes<Web>| {
            routes.screen("/summary", "summary", parcel_summary);
        });
    let server = TestServer::start(app).await;

    let page = server.get("/summary").await;
    let json = server
        .get("/summary")
        .header("accept", "application/json")
        .await;

    assert!(page.body.contains("<p>PAID 3</p>"), "{}", page.body);
    let answered: serde_json::Value = serde_json::from_str(&json.body).unwrap();
    assert_eq!(answered["by_status"][0]["status"], "PAID");
    assert_eq!(answered["by_status"][0]["count"], 3);
    assert!(answered.get("total").is_some(), "{answered}");
}

#[derive(ReadModel)]
struct Nothing {}

#[test]
fn a_computed_read_model_may_have_no_fields() {
    let reply = Nothing {}.reply().unwrap();

    assert_eq!(reply.to_json().unwrap(), serde_json::json!({}));
}
