use sofie::Value;
use sofie::core::{Cast, Error, RowReader, SqlType, SqlValue};
use sofie::serde_json::json;

#[derive(Debug, Clone, PartialEq, Value)]
struct PriceRange {
    low: i64,
    high: i64,
}

impl PriceRange {
    fn try_new(low: i64, high: i64) -> Result<Self, String> {
        if low > high {
            return Err(format!("{low} is above {high}"));
        }
        Ok(Self { low, high })
    }
}

#[derive(Debug, Clone, PartialEq, Value)]
struct Address {
    line: String,
    detail: Option<String>,
}

impl Address {
    #[expect(
        clippy::unnecessary_wraps,
        reason = "the derive reads through `try_new`"
    )]
    fn try_new(line: String, detail: Option<String>) -> Result<Self, String> {
        Ok(Self { line, detail })
    }
}

#[derive(Clone, PartialEq, Value)]
#[sensitive]
struct Iban(String);

impl Iban {
    fn try_new(text: String) -> sofie::core::Result<Self> {
        if text.starts_with("TR") {
            return Ok(Self(text));
        }
        Err(Error::invalid("iban", "must start with a country code"))
    }
}

#[derive(Clone, PartialEq, Value)]
#[sensitive]
struct Card {
    holder: String,
    last_digits: String,
}

impl Card {
    fn try_new(holder: String, last_digits: String) -> Result<Self, &'static str> {
        if last_digits.len() != 4 {
            return Err("needs the last four digits");
        }
        Ok(Self {
            holder,
            last_digits,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Value)]
struct Remark {
    text: Option<String>,
    tag: Option<String>,
}

impl Remark {
    #[expect(
        clippy::unnecessary_wraps,
        reason = "the derive reads through `try_new`"
    )]
    fn try_new(text: Option<String>, tag: Option<String>) -> Result<Self, String> {
        Ok(Self { text, tag })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, sofie::Enum)]
enum Stage {
    #[to(Packed)]
    Open,
    #[to(Sent)]
    Packed,
    Sent,
}

#[derive(Debug, Clone, PartialEq, Value)]
struct Parcel {
    label: String,
    stage: Stage,
}

impl Parcel {
    #[expect(
        clippy::unnecessary_wraps,
        reason = "the derive reads through `try_new`"
    )]
    fn try_new(label: String, stage: Stage) -> Result<Self, String> {
        Ok(Self { label, stage })
    }
}

#[test]
fn a_value_of_several_fields_is_nullable_only_when_every_field_is() {
    let nullable = [
        <Remark as Cast>::NULLABLE,
        <Address as Cast>::NULLABLE,
        <PriceRange as Cast>::NULLABLE,
    ];

    assert_eq!(nullable, [true, false, false]);
}

#[test]
fn a_state_inside_a_value_of_several_fields_moves_only_as_its_enum_lists() {
    let parcel = |stage| Parcel {
        label: "box".to_owned(),
        stage,
    };
    let refuse = |stored, current| {
        <Parcel as Cast>::refuse_unlisted_move("parcel", &parcel(stored), &parcel(current))
    };

    const { assert!(<Parcel as Cast>::HOLDS_STATES) };
    assert!(refuse(Stage::Open, Stage::Packed).is_ok());
    assert!(refuse(Stage::Open, Stage::Open).is_ok());
    assert!(refuse(Stage::Open, Stage::Sent).is_err());
}

#[test]
fn a_value_of_several_fields_spans_one_column_per_field() {
    let suffixes: Vec<_> = PriceRange::COLUMNS
        .iter()
        .map(|column| (column.suffix, column.ty))
        .collect();

    assert_eq!(suffixes, [("_low", SqlType::Int), ("_high", SqlType::Int)]);
}

#[test]
fn a_value_of_several_fields_decodes_through_try_new() {
    let range = PriceRange::try_new(100, 250).unwrap();
    let mut values = Vec::new();
    range.encode(&mut values);
    values.extend([SqlValue::Int(9), SqlValue::Int(1)]);
    let mut row = RowReader::new(&values);

    assert_eq!(PriceRange::decode(&mut row).unwrap(), range);
    assert!(matches!(
        PriceRange::decode(&mut row),
        Err(Error::Internal { context, .. }) if context.contains("PriceRange")
    ));
}

#[test]
fn a_value_of_several_fields_is_an_object_read_through_try_new() {
    let range = PriceRange::try_new(100, 250).unwrap();

    assert_eq!(
        sofie::serde_json::to_value(&range).unwrap(),
        json!({"low": 100, "high": 250})
    );
    assert_eq!(
        sofie::serde_json::from_value::<PriceRange>(json!({"low": 100, "high": "250"})).unwrap(),
        range
    );
    let refused = sofie::serde_json::from_value::<PriceRange>(json!({"low": 3, "high": 2}));
    assert_eq!(refused.unwrap_err().to_string(), "3 is above 2");
    let unknown =
        sofie::serde_json::from_value::<PriceRange>(json!({"low": 1, "high": 2, "mid": 1}));
    assert_eq!(unknown.unwrap_err().to_string(), "is not accepted");
    let missing = sofie::serde_json::from_value::<PriceRange>(json!({"low": 1}));
    assert_eq!(missing.unwrap_err().to_string(), "is required");
}

#[test]
fn an_optional_member_of_a_value_may_be_left_out() {
    let address = sofie::serde_json::from_value::<Address>(json!({"line": "Main St 1"})).unwrap();

    assert_eq!(address.detail, None);
}

#[test]
fn a_sensitive_value_is_masked_in_debug() {
    let iban = Iban::try_new("TR330006100519786457841326".to_owned()).unwrap();
    let card = Card::try_new("Ada Lovelace".to_owned(), "4242".to_owned()).unwrap();

    assert_eq!(format!("{iban:?}"), "Iban(***)");
    assert_eq!(format!("{card:?}"), "Card(***)");
    assert_eq!(
        sofie::serde_json::to_value(&card).unwrap(),
        json!({"holder": "Ada Lovelace", "last_digits": "4242"})
    );
    assert!(sofie::serde_json::from_value::<Iban>(json!("DE89")).is_err());
}

#[test]
fn a_sensitive_value_of_one_field_is_typed_as_a_password_and_written_as_that_field() {
    use sofie::values::{Present, SurfacesFromInner};

    let iban = Iban::try_new("TR33".to_owned()).unwrap();

    assert_eq!(Iban::INPUT.control, sofie::values::Control::Password);
    assert_eq!(iban.csv(), "TR33");
    const { assert!(<Iban as SurfacesFromInner>::SENSITIVE) };
}

#[cfg(feature = "resources")]
mod resources {
    use serde_json::json;
    use sofie::Value;
    use sofie::core::{Error, SqlValue};
    use sofie::db::{Db, File, Migrations};
    use sofie::prelude::*;
    use sofie_testing::TestApp;

    #[derive(Clone, PartialEq, Value)]
    #[sensitive]
    pub struct Iban(String);

    impl Iban {
        fn try_new(text: String) -> sofie::core::Result<Self> {
            if text.starts_with("TR") {
                return Ok(Self(text));
            }
            Err(Error::invalid("iban", "must start with a country code"))
        }
    }

    #[derive(Aggregate)]
    pub struct Payee {
        meta: Meta,
        #[required]
        name: String,
        #[required]
        iban: Iban,
    }

    #[derive(Aggregate)]
    pub struct Reading {
        meta: Meta,
        #[required]
        rate: sofie::values::Percentage,
        weight: sofie::values::Weight,
        spot: sofie::values::Coordinate,
    }

    const STORED: &str = "TR330006100519786457841326";

    const MIGRATIONS: &[File] = &[
        File::new("0001_payee.sql", b"CREATE TABLE payee (id uuid PRIMARY KEY, version bigint NOT NULL, name text NOT NULL, \
             iban text NOT NULL, created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL);"),
        File::new("0002_reading.sql", b"CREATE TABLE reading (id uuid PRIMARY KEY, version bigint NOT NULL, rate bigint NOT NULL, \
             weight double precision NOT NULL, weight_unit text NOT NULL, \
             spot_latitude double precision NOT NULL, spot_longitude double precision NOT NULL, \
             created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL);"),
    ];

    async fn started() -> TestApp {
        sofie_testing::start(
            |db: Db| async move {
                db.schema()
                    .migrate(&[Migrations::new("values", MIGRATIONS)])
                    .await?;
                Ok(App::new(db).area::<Web>("values", |routes| {
                    routes.resource::<Payee>();
                    routes.resource::<Reading>();
                }))
            },
            sofie::worker::Worker::new,
        )
        .await
    }

    #[tokio::test]
    async fn a_sensitive_field_is_edited_blank_and_a_blank_submission_keeps_it() {
        let app = started().await;
        let created = app
            .post("/payees/new")
            .json(&json!({ "name": "Ada", "iban": STORED }))
            .await;
        assert_eq!(created.status, 200, "{}", created.body);
        let id = created.json()["id"].as_str().unwrap().to_owned();

        let shown = app.get(&format!("/payees/{id}")).await;
        let listed = app.get("/payees").await;
        let form = app.get(&format!("/payees/{id}/edit")).await;
        let saved = app
            .post(&format!("/payees/{id}/edit"))
            .json(&json!({ "name": "Ada Lovelace", "iban": "" }))
            .await;

        assert!(!shown.body.contains(STORED), "{}", shown.body);
        assert!(!listed.body.contains(STORED), "{}", listed.body);
        assert_eq!(form.status, 200, "{}", form.body);
        let iban_input = form.body.split("name=\"iban\"").nth(1).unwrap();
        let iban_input = &iban_input[..iban_input.find('>').unwrap()];
        assert!(form.body.contains("type=\"password\""), "{}", form.body);
        assert!(!iban_input.contains("required"), "{iban_input}");
        assert!(!form.body.contains(STORED), "{}", form.body);
        assert_eq!(saved.status, 200, "{}", saved.body);
        let rows = app.query("SELECT name, iban FROM values.payee", &[]).await;
        assert_eq!(
            rows,
            [vec![
                SqlValue::Text("Ada Lovelace".to_owned()),
                SqlValue::Text(STORED.to_owned())
            ]]
        );
    }

    #[tokio::test]
    async fn a_record_shows_a_percentage_and_a_value_of_parts_as_text() {
        let app = started().await;
        let created = app
            .post("/readings/new")
            .json(&json!({
                "rate": 1250,
                "weight": { "quantity": "72.5", "unit": "KILOGRAM" },
                "spot": { "latitude": 41.01, "longitude": 28.97 },
            }))
            .await;
        assert_eq!(created.status, 200, "{}", created.body);
        let id = created.json()["id"].as_str().unwrap().to_owned();

        let shown = app.get(&format!("/readings/{id}")).await;

        assert!(shown.body.contains("<dd>12.50 %</dd>"), "{}", shown.body);
        assert!(
            shown.body.contains("<dd>72.500 KILOGRAM</dd>"),
            "{}",
            shown.body
        );
        assert!(
            shown.body.contains("<dd>41.01 28.97</dd>"),
            "{}",
            shown.body
        );
    }
}
