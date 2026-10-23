use sofie::core::{Cast, Error, FieldError, RowReader, SqlType, SqlValue, Variants};
use sofie::db::{
    Aggregate, Column, Event, Id, Input, Meta, Outcome, Selected, SoftDeletes, Source, Timestamped,
};
use sofie::serde_json::json;
use sofie::{Aggregate, Enum, Event, Input, ReadModel, Value};

fn names(selection: &[Selected]) -> Vec<String> {
    selection
        .iter()
        .map(|selected| {
            let debug = format!("{selected:?}");
            debug
                .trim_start_matches("Selected(Column(\"")
                .trim_end_matches("\"))")
                .to_owned()
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Value)]
struct Quantity(i64);

impl Quantity {
    fn try_new(count: i64) -> Result<Self, String> {
        if count > 0 {
            return Ok(Self(count));
        }
        Err(format!("{count} is not a positive quantity"))
    }
}

#[derive(Debug, Clone, PartialEq, Value)]
struct Sku {
    code: String,
}

impl Sku {
    fn try_new(code: String) -> Result<Self, &'static str> {
        if code.is_empty() {
            return Err("a sku cannot be empty");
        }
        Ok(Self { code })
    }
}

#[derive(Debug, Clone, PartialEq, Value)]
struct Barcode(String);

impl Barcode {
    fn try_new(digits: String) -> sofie::core::Result<Self> {
        if digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return Ok(Self(digits));
        }
        Err(Error::invalid("barcode", "must be digits only"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Enum)]
enum LineStatus {
    #[to(Reserved, Cancelled)]
    Open,
    #[to(PartlyShipped, Cancelled)]
    Reserved,
    PartlyShipped,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Enum)]
enum Colour {
    Red,
    DarkBlue,
}

#[derive(Debug, Aggregate)]
struct OrderLine {
    meta: Meta,
    sku: Sku,
    quantity: Quantity,
    status: LineStatus,
    note: Option<String>,
}

#[derive(Debug, ReadModel)]
#[read(from = OrderLine)]
struct LineRow {
    id: Id<OrderLine>,
    status: LineStatus,
    quantity: Quantity,
}

#[derive(Debug, PartialEq, Input)]
struct AddLine {
    sku: Sku,
    quantity: Quantity,
    note: Option<String>,
}

#[derive(Debug, PartialEq, Input)]
struct Feedback {
    errors: Vec<String>,
    object: Option<String>,
}

fn line() -> OrderLine {
    OrderLine {
        meta: Meta::new().unwrap(),
        sku: Sku::try_new("A-1".to_owned()).unwrap(),
        quantity: Quantity::try_new(3).unwrap(),
        status: LineStatus::Open,
        note: None,
    }
}

struct Shopper;

#[derive(Aggregate)]
struct Basket {
    meta: Meta,
    #[owner]
    shopper: Id<Shopper>,
    items: i64,
}

#[test]
fn an_owner_field_names_the_column_owned_reads_filter_on() {
    let shopper = Id::from_uuid(uuid_of(7));
    let basket = Basket {
        meta: Meta::new().unwrap(),
        shopper,
        items: 1,
    };

    assert_eq!(Basket::OWNER, Some("shopper"));
    assert_eq!(basket.owner(), Some(shopper.uuid()));
    assert_eq!(basket.items, 1);
    assert_eq!(<OrderLine as Aggregate>::OWNER, None);
    assert_eq!(line().owner(), None);
}

#[derive(Aggregate)]
struct Payer {
    meta: Meta,
    name: String,
    #[sensitive]
    tax_number: String,
}

#[test]
fn a_sensitive_field_is_masked_in_debug_and_listed_for_logging() {
    let payer = Payer {
        meta: Meta::new().unwrap(),
        name: "Ada".to_owned(),
        tax_number: "12345678901".to_owned(),
    };

    let shown = format!("{payer:?}");

    assert!(shown.contains("name: \"Ada\", tax_number: ***"), "{shown}");
    assert!(!shown.contains("12345678901"), "{shown}");
    assert_eq!(
        <Payer as sofie::values::SensitiveFields>::SENSITIVE_FIELDS,
        ["tax_number"]
    );
}

#[derive(Clone, PartialEq, Value)]
#[sensitive]
struct TaxNumber(String);

impl TaxNumber {
    fn try_new(text: String) -> sofie::core::Result<Self> {
        if text.is_empty() {
            return Err(Error::invalid("tax_number", "must not be empty"));
        }
        Ok(Self(text))
    }
}

#[derive(Aggregate)]
struct Payee {
    meta: Meta,
    #[sensitive]
    tax_number: TaxNumber,
    #[sensitive]
    note: String,
}

#[test]
fn an_edit_keeps_a_marked_field_only_when_the_server_can_read_it_back() {
    let handed_back = |name| {
        <Payee as Aggregate>::FIELDS
            .iter()
            .find(|field| field.name == name)
            .map(|field| (field.handed_back)())
    };

    assert_eq!(handed_back("tax_number"), Some(true));
    assert_eq!(handed_back("note"), Some(false));
}

#[derive(Clone, PartialEq, Value)]
struct Sensitivity(i64);

impl Sensitivity {
    fn try_new(level: i64) -> Result<Self, String> {
        if (0..=3).contains(&level) {
            return Ok(Self(level));
        }
        Err(format!("{level} is not a sensitivity level"))
    }
}

#[derive(ReadModel)]
struct RiskSummary {
    level: Sensitivity,
}

#[test]
fn a_computed_read_model_may_hold_a_type_named_as_the_derive_s_own() {
    let summary = RiskSummary {
        level: Sensitivity::try_new(2).unwrap(),
    };

    assert_eq!(summary.level.0, 2);
}

#[test]
fn a_listing_neither_filters_nor_sorts_by_a_sensitive_field() {
    use sofie::db::{Condition, Sort};
    use sofie::values::Operator;

    assert!(Condition::<Payer>::on_field("tax_number", Operator::Equals, "1").is_err());
    assert!(Sort::<Payer>::by_field("tax_number", false).is_none());
    assert!(Condition::<Payer>::on_field("name", Operator::Equals, "Ada").is_ok());
}

#[test]
fn a_listing_filtered_by_an_unknown_variant_names_the_accepted_ones() {
    use sofie::db::Condition;
    use sofie::values::Operator;

    let refused = Condition::<OrderLine>::on_field("status", Operator::Equals, "pending");

    assert!(
        matches!(&refused, Err(Error::Invalid(fields)) if fields[0].field == "status"
            && fields[0].message.contains("OPEN, RESERVED, PARTLY_SHIPPED, CANCELLED")),
        "{:?}",
        refused.err()
    );
    assert!(Condition::<OrderLine>::on_field("status", Operator::Equals, "OPEN").is_ok());
}

#[derive(Aggregate)]
#[owner]
struct Profile {
    meta: Meta,
    name: String,
}

#[test]
fn an_aggregate_can_be_its_own_owner() {
    let profile = Profile {
        meta: Meta::new().unwrap(),
        name: "Ada".to_owned(),
    };

    assert_eq!(Profile::OWNER, Some("id"));
    assert_eq!(profile.owner(), Some(profile.meta.id()));
    assert_eq!(profile.name, "Ada");
}

fn uuid_of(number: u128) -> sofie::db::Uuid {
    sofie::db::Uuid::from_u128(number)
}

#[test]
fn aggregate_describes_its_table_and_fields() {
    assert_eq!(OrderLine::NAME, "order_line");
    assert_eq!(OrderLine::MODULE, "derives");
    let fields: Vec<(&str, bool)> = OrderLine::FIELDS
        .iter()
        .map(|field| (field.name, field.nullable))
        .collect();
    assert_eq!(
        fields,
        [
            ("sku", false),
            ("quantity", false),
            ("status", false),
            ("note", true)
        ]
    );
    assert_eq!(OrderLine::FIELDS[2].columns[0].ty, SqlType::Text);
    assert_eq!(
        names(&<OrderLine as Source>::columns()),
        [
            "id",
            "version",
            "created_at",
            "updated_at",
            "sku",
            "quantity",
            "status",
            "note"
        ]
    );
}

#[derive(Aggregate)]
#[aggregate(behaviors(SoftDelete))]
struct Shelf {
    meta: Meta,
    label: String,
}

#[test]
fn a_named_behavior_adds_its_columns_after_the_default_ones() {
    let mut shelf = Shelf {
        meta: Meta::new().unwrap(),
        label: "top".to_owned(),
    };
    shelf.delete();

    assert_eq!(
        names(&<Shelf as Source>::columns()),
        [
            "id",
            "version",
            "created_at",
            "updated_at",
            "deleted_at",
            "label"
        ]
    );
    let _: Column<Shelf, Option<sofie::core::Instant>> = Shelf::DELETED_AT;
    assert!(shelf.is_deleted());
    assert_eq!((shelf.label.as_str(), shelf.created_at()), ("top", None));
}

#[test]
fn a_save_moving_a_state_along_an_edge_its_enum_does_not_list_is_refused() {
    let loaded = [
        SqlValue::Text("A-1".to_owned()),
        SqlValue::Int(3),
        SqlValue::Text("OPEN".to_owned()),
        SqlValue::Null,
    ];
    let mut skipped = line();
    skipped.status = LineStatus::PartlyShipped;
    let mut reserved = line();
    reserved.status = LineStatus::Reserved;

    let refused = skipped.refuse_unlisted_moves(&mut RowReader::new(&loaded));

    assert_refused_move(
        &refused,
        "status",
        "cannot move from OPEN to PARTLY_SHIPPED",
    );
    assert!(
        reserved
            .refuse_unlisted_moves(&mut RowReader::new(&loaded))
            .is_ok()
    );
}

mod payment {
    #[derive(Debug, Clone, Copy, PartialEq, sofie::Enum)]
    pub enum Status {
        #[to(Paid)]
        Pending,
        Paid,
    }
}

mod purchase {
    #[derive(Debug, Clone, Copy, PartialEq, sofie::Enum)]
    pub enum Status {
        #[to(Confirmed)]
        Pending,
        #[to(Paid)]
        Confirmed,
        Paid,
    }
}

#[derive(Debug, Aggregate)]
struct Purchase {
    meta: Meta,
    status: purchase::Status,
}

#[test]
fn a_move_of_another_enum_of_the_same_name_explains_no_save() {
    payment::Status::Pending.to(payment::Status::Paid).unwrap();
    let purchase = Purchase {
        meta: Meta::new().unwrap(),
        status: purchase::Status::Paid,
    };
    let loaded = [SqlValue::Text("PENDING".to_owned())];

    let refused = purchase.refuse_unlisted_moves(&mut RowReader::new(&loaded));

    assert_refused_move(&refused, "status", "cannot move from PENDING to PAID");
}

#[test]
fn aggregate_round_trips_through_its_columns() {
    let original = line();
    let mut values = Vec::new();
    Aggregate::encode(&original, &mut values);
    assert_eq!(
        values,
        [
            SqlValue::Text("A-1".to_owned()),
            SqlValue::Int(3),
            SqlValue::Text("OPEN".to_owned()),
            SqlValue::Null,
        ]
    );

    let meta = Meta::new().unwrap();
    let id = meta.id();
    let decoded = <OrderLine as Aggregate>::decode(meta, &mut RowReader::new(&values)).unwrap();

    assert_eq!(decoded.meta().id(), id);
    assert_eq!(decoded.sku, original.sku);
    assert_eq!(decoded.quantity, original.quantity);
    assert_eq!(decoded.status, original.status);
    assert_eq!(decoded.note, None);
}

#[test]
fn aggregate_replies_with_its_id_and_version() {
    let original = line();
    let id = original.meta().id().to_string();

    assert_eq!(
        original.reply().unwrap().into_json().unwrap(),
        json!({ "id": id, "version": 0 })
    );
}

#[test]
fn aggregate_has_a_typed_column_per_field() {
    let _: Column<OrderLine, Sku> = OrderLine::SKU;
    let _: Column<OrderLine, Quantity> = OrderLine::QUANTITY;
    let _: Column<OrderLine, LineStatus> = OrderLine::STATUS;
    let _: Column<OrderLine, Option<String>> = OrderLine::NOTE;
}

#[test]
fn value_round_trips_and_refuses_what_try_new_refuses() {
    assert_eq!(sofie::serde_json::to_value(Quantity(4)).unwrap(), json!(4));
    assert_eq!(
        sofie::serde_json::from_value::<Sku>(json!("B-2")).unwrap(),
        Sku {
            code: "B-2".to_owned()
        }
    );
    let refused = sofie::serde_json::from_value::<Quantity>(json!(0)).unwrap_err();
    assert!(refused.to_string().contains("0 is not a positive quantity"));
}

#[test]
fn a_value_refused_with_a_framework_error_shows_only_its_message() {
    let refused = sofie::serde_json::from_value::<Barcode>(json!("12a")).unwrap_err();

    assert_eq!(refused.to_string(), "must be digits only");
    assert_eq!(
        sofie::serde_json::from_value::<Barcode>(json!("123")).unwrap(),
        Barcode("123".to_owned())
    );
}

#[test]
fn value_decodes_through_try_new() {
    let values = vec![SqlValue::Int(5), SqlValue::Int(-1)];
    let mut row = RowReader::new(&values);

    assert_eq!(Quantity::decode(&mut row).unwrap(), Quantity(5));
    assert!(matches!(
        Quantity::decode(&mut row),
        Err(Error::Internal { context, .. }) if context.contains("Quantity")
    ));
}

#[test]
fn enum_is_stored_and_serialized_by_name() {
    assert_eq!(LineStatus::PartlyShipped.name(), "PARTLY_SHIPPED");
    assert_eq!(Colour::DarkBlue.name(), "DARK_BLUE");
    assert_eq!(
        sofie::serde_json::to_value(LineStatus::PartlyShipped).unwrap(),
        json!("PARTLY_SHIPPED")
    );
    assert_eq!(
        sofie::serde_json::from_value::<Colour>(json!("RED")).unwrap(),
        Colour::Red
    );
    let values = vec![SqlValue::Text("CANCELLED".to_owned())];
    let mut row = RowReader::new(&values);
    assert_eq!(LineStatus::decode(&mut row).unwrap(), LineStatus::Cancelled);
}

#[test]
fn enum_refuses_an_unknown_name_listing_the_accepted_ones() {
    let refused = sofie::serde_json::from_value::<Colour>(json!("GREEN"))
        .unwrap_err()
        .to_string();

    assert!(refused.contains("GREEN"), "{refused}");
    assert!(
        refused.contains("RED") && refused.contains("DARK_BLUE"),
        "{refused}"
    );
}

#[test]
fn enum_allows_only_the_listed_transitions() {
    assert_eq!(
        LineStatus::Open.to(LineStatus::Reserved).unwrap(),
        LineStatus::Reserved
    );
    assert_eq!(
        LineStatus::Reserved.to(LineStatus::Cancelled).unwrap(),
        LineStatus::Cancelled
    );

    let refused = LineStatus::Cancelled.to(LineStatus::Open);

    assert_refused_move(
        &refused,
        "line_status",
        "cannot move from CANCELLED to OPEN",
    );
}

#[test]
fn read_model_selects_its_fields_from_the_base() {
    assert_eq!(names(&LineRow::columns()), ["id", "status", "quantity"]);
    assert_eq!(
        LineRow::column_types(),
        [Some(SqlType::Uuid), Some(SqlType::Text), Some(SqlType::Int)]
    );
}

#[test]
fn read_model_decodes_and_replies_with_its_fields() {
    let id = Meta::new().unwrap().id();
    let values = vec![
        SqlValue::Uuid(id),
        SqlValue::Text("RESERVED".to_owned()),
        SqlValue::Int(2),
    ];
    let mut row = RowReader::new(&values);
    let read = <LineRow as Source>::decode(&mut row).unwrap();

    assert_eq!(read.id, Id::from_uuid(id));
    assert_eq!(
        read.reply().unwrap().into_json().unwrap(),
        json!({
            "id": id.to_string(),
            "status": "RESERVED",
            "status_to": { "PARTLY_SHIPPED": true, "CANCELLED": true },
            "quantity": 2,
        })
    );
}

#[test]
fn read_model_replies_no_moves_from_a_terminal_state() {
    let values = vec![
        SqlValue::Uuid(Meta::new().unwrap().id()),
        SqlValue::Text("CANCELLED".to_owned()),
        SqlValue::Int(2),
    ];
    let mut row = RowReader::new(&values);
    let read = <LineRow as Source>::decode(&mut row).unwrap();

    let reply = read.reply().unwrap().into_json().unwrap();
    assert_eq!(reply["status_to"], json!({}));
    assert_eq!(sofie::core::moves::targets(&Colour::Red), None);
}

#[derive(Debug)]
struct Unwritable(i64);

impl sofie::core::Value for Unwritable {
    type Inner = i64;

    fn inner(&self) -> &i64 {
        &self.0
    }

    fn try_from_inner(inner: i64) -> Result<Self, impl std::fmt::Display> {
        Ok::<_, Error>(Self(inner))
    }
}

impl sofie::serde::Serialize for Unwritable {
    fn serialize<S: sofie::serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
        Err(sofie::serde::ser::Error::custom("not writable"))
    }
}

/// Readable, as every field of an aggregate with a derived factory must be.
impl<'de> sofie::serde::Deserialize<'de> for Unwritable {
    fn deserialize<D: sofie::serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        sofie::core::value::deserialize(deserializer)
    }
}

#[derive(Aggregate)]
struct Vault {
    meta: Meta,
    secret: Unwritable,
}

#[derive(ReadModel)]
#[read(from = Vault)]
struct VaultRow {
    secret: Unwritable,
}

#[test]
fn a_read_model_field_that_cannot_be_serialized_fails_the_reply() {
    let values = vec![SqlValue::Int(1)];
    let mut row = RowReader::new(&values);
    let read = <VaultRow as Source>::decode(&mut row).unwrap();

    let reply = read.reply().unwrap();
    assert!(matches!(
        reply.write_json(&mut Vec::new()),
        Err(Error::Internal { .. })
    ));
    assert!(matches!(reply.into_json(), Err(Error::Internal { .. })));
    let vault = Vault {
        meta: Meta::new().unwrap(),
        secret: Unwritable(2),
    };
    assert_eq!(vault.secret.0, 2);
}

#[derive(Debug, PartialEq, Event)]
struct LineShipped {
    line: Id<OrderLine>,
    quantity: Quantity,
}

#[test]
fn event_is_named_after_its_crate_and_round_trips_its_fields() {
    let event = LineShipped {
        line: Id::from_uuid(Meta::new().unwrap().id()),
        quantity: Quantity::try_new(2).unwrap(),
    };

    let payload = sofie::serde_json::to_value(&event).unwrap();

    assert_eq!(LineShipped::NAME, "derives.LineShipped");
    assert_eq!(
        payload,
        json!({ "line": event.line.to_string(), "quantity": 2 })
    );
    assert_eq!(LineShipped::from_json(payload).unwrap(), event);
    assert!(matches!(
        LineShipped::from_json(json!({ "line": "x", "quantity": 0 })),
        Err(Error::Invalid(errors)) if errors.len() == 2
    ));
}

#[test]
fn input_reads_every_field() {
    let input = AddLine::from_json(json!({ "sku": "C-3", "quantity": 1 })).unwrap();

    assert_eq!(
        input,
        AddLine {
            sku: Sku::try_new("C-3".to_owned()).unwrap(),
            quantity: Quantity(1),
            note: None,
        }
    );
}

#[test]
fn input_collects_every_field_error() {
    let refused = AddLine::from_json(json!({ "quantity": 0, "colour": "RED" })).unwrap_err();

    let Error::Invalid(errors) = refused else {
        panic!("expected invalid input, got {refused:?}");
    };
    let fields: Vec<(&str, &str)> = errors
        .iter()
        .map(|FieldError { field, message, .. }| (field.as_str(), message.as_str()))
        .collect();
    assert_eq!(
        fields,
        [
            ("sku", "is required"),
            ("quantity", "0 is not a positive quantity"),
            ("colour", "is not accepted"),
        ]
    );
}

#[test]
fn input_refuses_anything_but_an_object() {
    assert!(matches!(
        AddLine::from_json(json!([1, 2])),
        Err(Error::BadRequest(_))
    ));
}

#[test]
fn input_fields_may_share_names_with_the_reader_internals() {
    let input = Feedback::from_json(json!({ "errors": ["late"] })).unwrap();

    assert_eq!(
        input,
        Feedback {
            errors: vec!["late".to_owned()],
            object: None,
        }
    );
}

fn assert_refused_move<T: std::fmt::Debug>(refused: &Result<T, Error>, field: &str, message: &str) {
    let Err(Error::Invalid(fields)) = refused else {
        panic!("expected a refused move, got {refused:?}");
    };
    assert_eq!(
        (
            fields[0].field.as_str(),
            fields[0].key.as_ref(),
            fields[0].message.as_str()
        ),
        (field, "validation.move", message)
    );
}
