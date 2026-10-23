//! Request bodies made from an `Input` type's fields, valid or broken on
//! purpose, the same for the same seed.

use std::mem;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};
use sofie_core::Variants;
use sofie_core::encoding::percent;
use sofie_model::{EnumDeclaration, TypeName};
use sofie_values::fixture::field_json;
use sofie_values::{
    Currency, Date, Email, Instant, LocalDateTime, Money, Samples, Seed, TimeOfDay, TimeZone,
    Weekday,
};

const OVERSIZED_TEXT: usize = 100_000;
const NESTING: usize = 10_000;
/// Too large for any integer a field holds; written into the body as it is.
const HUGE_NUMBER: &str = "184467440737095516160000";
const RAW_MARKER: &str = "\u{1}raw\u{1}";
const JSON: &str = "application/json";
const FORM: &str = "application/x-www-form-urlencoded";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Text,
    Password,
    Integer,
    Boolean,
    Uuid,
    Enum(Vec<String>),
    Value(Valued),
    List(Box<Kind>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Valued {
    Money,
    Currency,
    Email,
    Instant,
    Date,
    TimeZone,
    LocalDateTime,
    TimeOfDay,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Field {
    pub(crate) name: String,
    pub(crate) kind: Kind,
    pub(crate) optional: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Case {
    pub(crate) label: String,
    pub(crate) content_type: &'static str,
    pub(crate) body: String,
}

#[derive(Clone, Copy)]
enum Breakage {
    Boundary,
    Malformed,
    Oversized,
    WrongType,
    Missing,
}

const BREAKAGES: [Breakage; 5] = [
    Breakage::Boundary,
    Breakage::Malformed,
    Breakage::Oversized,
    Breakage::WrongType,
    Breakage::Missing,
];

impl Breakage {
    fn name(self) -> &'static str {
        match self {
            Self::Boundary => "boundary",
            Self::Malformed => "malformed",
            Self::Oversized => "oversized",
            Self::WrongType => "wrong type",
            Self::Missing => "missing",
        }
    }
}

impl Field {
    pub(crate) fn of(name: &str, ty: &TypeName, enums: &[EnumDeclaration]) -> Self {
        let optional = ty.name == "Option";
        let inner = match ty.arguments.as_slice() {
            [inner] if optional => inner,
            _ => ty,
        };
        Self {
            name: name.to_owned(),
            kind: Kind::of(inner, enums),
            optional,
        }
    }
}

impl Kind {
    fn of(ty: &TypeName, enums: &[EnumDeclaration]) -> Self {
        match (ty.name.as_str(), ty.arguments.as_slice()) {
            ("Vec", [item]) => Self::List(Box::new(Self::of(item, enums))),
            ("Secret" | "Option", [inner]) => Self::of(inner, enums),
            ("Id" | "Uuid", _) => Self::Uuid,
            ("i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" | "usize", []) => {
                Self::Integer
            }
            ("bool", []) => Self::Boolean,
            ("Password", []) => Self::Password,
            ("Money", []) => Self::Value(Valued::Money),
            ("Currency", []) => Self::Value(Valued::Currency),
            ("Email", []) => Self::Value(Valued::Email),
            ("Instant", []) => Self::Value(Valued::Instant),
            ("Date", []) => Self::Value(Valued::Date),
            ("TimeZone", []) => Self::Value(Valued::TimeZone),
            ("LocalDateTime", []) => Self::Value(Valued::LocalDateTime),
            ("TimeOfDay", []) => Self::Value(Valued::TimeOfDay),
            ("Weekday", []) => Self::Enum(
                Weekday::VARIANTS
                    .iter()
                    .map(|(name, _)| (*name).to_owned())
                    .collect(),
            ),
            (name, []) => match enums.iter().find(|declared| declared.name == name) {
                Some(declared) => Self::Enum(declared.stored_names.clone()),
                None if ty.record().is_some() => Self::Uuid,
                None => Self::Text,
            },
            _ => Self::Text,
        }
    }

    pub(crate) fn valid(&self, seed: &mut Seed) -> Value {
        match self {
            Self::Text => field_json::<String>(seed, &[]),
            Self::Password => json!(format!("fuzz-password-{}", seed.between(100, 999))),
            Self::Integer => field_json::<i32>(seed, &[]),
            Self::Boolean => field_json::<bool>(seed, &[]),
            Self::Uuid => field_json::<sofie_core::Uuid>(seed, &[]),
            Self::Enum(names) if names.is_empty() => json!(""),
            Self::Enum(names) => json!(seed.pick(names)),
            Self::Value(valued) => valued.valid(seed),
            Self::List(item) => {
                let count = seed.between(1, 3);
                Value::Array((0..count).map(|_| item.valid(seed)).collect())
            }
        }
    }

    fn boundaries(&self) -> Vec<Value> {
        match self {
            Self::Text | Self::Password => vec![
                json!(""),
                json!("x"),
                json!("x".repeat(255)),
                json!("x".repeat(256)),
            ],
            Self::Integer => vec![
                json!(0),
                json!(-1),
                json!(i32::MAX),
                json!(i32::MIN),
                json!(i64::MAX),
                json!(i64::MIN),
            ],
            Self::Boolean => vec![json!(false), json!(true)],
            Self::Uuid => vec![
                json!("00000000-0000-0000-0000-000000000000"),
                json!("ffffffff-ffff-ffff-ffff-ffffffffffff"),
            ],
            Self::Enum(names) => names.iter().map(|name| json!(name)).collect(),
            Self::Value(valued) => valued.boundaries(),
            Self::List(_) => vec![json!([]), Value::Array(vec![Value::Null; 1000])],
        }
    }

    fn malformed(&self) -> Vec<Value> {
        let text = [
            "\u{0}",
            "   ",
            "<script>alert(1)</script>",
            "' OR '1'='1' --",
            "\u{202e}gnp.exe",
            "%00",
            "{{ secret }}",
        ];
        match self {
            Self::Text => text.iter().map(|sample| json!(sample)).collect(),
            Self::Password => vec![json!("short"), json!("\u{0}".repeat(12))],
            Self::Integer => vec![json!(1.5), json!(-0.0), json!(1e3)],
            Self::Boolean => vec![json!("yes"), json!(1)],
            Self::Uuid => vec![
                json!("not-an-id"),
                json!("00000000-0000-0000-0000-00000000000g"),
                json!(""),
            ],
            Self::Enum(names) => {
                let mut samples = vec![json!("NOT_A_STATE"), json!("")];
                samples.extend(names.iter().map(|name| json!(name.to_lowercase())));
                samples
            }
            Self::Value(valued) => valued.refused(),
            Self::List(item) => item
                .malformed()
                .into_iter()
                .map(|sample| json!([sample]))
                .collect(),
        }
    }

    fn oversized(&self) -> Vec<Value> {
        match self {
            Self::Integer => vec![json!(RAW_MARKER), json!(1e300), json!(-1e300)],
            Self::List(item) => {
                let mut seed = Seed::new(0);
                let one = item.valid(&mut seed);
                vec![Value::Array(vec![one; OVERSIZED_TEXT / 10])]
            }
            Self::Value(Valued::Money) => vec![
                json!({ "minor": RAW_MARKER, "currency": "EUR" }),
                json!({ "amount": "9".repeat(400), "currency": "EUR" }),
            ],
            _ => vec![json!("x".repeat(OVERSIZED_TEXT))],
        }
    }

    fn wrong_types(&self, seed: &mut Seed) -> Vec<Value> {
        let valid = self.valid(seed);
        [
            json!(12345),
            json!("text"),
            json!(true),
            json!([1, 2]),
            json!({ "nested": 1 }),
            Value::Null,
        ]
        .into_iter()
        .filter(|candidate| mem::discriminant(candidate) != mem::discriminant(&valid))
        .collect()
    }
}

impl Valued {
    fn valid(self, seed: &mut Seed) -> Value {
        match self {
            Self::Money => field_json::<Money>(seed, &[]),
            Self::Currency => field_json::<Currency>(seed, &[]),
            Self::Email => field_json::<Email>(seed, &[]),
            Self::Instant => field_json::<Instant>(seed, &[]),
            Self::Date => field_json::<Date>(seed, &[]),
            Self::TimeZone => field_json::<TimeZone>(seed, &[]),
            Self::LocalDateTime => field_json::<LocalDateTime>(seed, &[]),
            Self::TimeOfDay => field_json::<TimeOfDay>(seed, &[]),
        }
    }

    pub(crate) fn samples(self) -> (&'static [&'static str], &'static [&'static str]) {
        match self {
            Self::Money => (Money::ACCEPTED, Money::REFUSED),
            Self::Currency => (Currency::ACCEPTED, Currency::REFUSED),
            Self::Email => (Email::ACCEPTED, Email::REFUSED),
            Self::Instant => (Instant::ACCEPTED, Instant::REFUSED),
            Self::Date => (Date::ACCEPTED, Date::REFUSED),
            Self::TimeZone => (TimeZone::ACCEPTED, TimeZone::REFUSED),
            Self::LocalDateTime => (LocalDateTime::ACCEPTED, LocalDateTime::REFUSED),
            Self::TimeOfDay => (TimeOfDay::ACCEPTED, TimeOfDay::REFUSED),
        }
    }

    fn boundaries(self) -> Vec<Value> {
        let extremes = match self {
            Self::Money => vec![
                json!({ "minor": 0, "currency": "EUR" }),
                json!({ "minor": -1, "currency": "EUR" }),
                json!({ "minor": i64::MAX, "currency": "EUR" }),
                json!({ "minor": i64::MIN, "currency": "JPY" }),
            ],
            Self::Currency => vec![json!("XXX"), json!("XTS")],
            Self::Email => vec![json!(format!("{}@example.com", "a".repeat(64)))],
            Self::Instant => vec![
                json!("0001-01-01T00:00:00Z"),
                json!("9999-12-31T23:59:59Z"),
                json!("1970-01-01T00:00:00Z"),
            ],
            Self::Date => vec![json!("0001-01-01"), json!("9999-12-31")],
            Self::TimeOfDay => vec![json!("23:59")],
            Self::TimeZone | Self::LocalDateTime => Vec::new(),
        };
        let (accepted, _) = self.samples();
        accepted
            .iter()
            .map(|sample| parsed(sample))
            .chain(extremes)
            .collect()
    }

    fn refused(self) -> Vec<Value> {
        let (_, refused) = self.samples();
        refused.iter().map(|sample| parsed(sample)).collect()
    }
}

fn parsed(sample: &str) -> Value {
    serde_json::from_str(sample).expect("sofie-values' samples are JSON")
}

pub(crate) fn clock_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

pub(crate) fn valid_object(fields: &[Field], seed: &mut Seed) -> Map<String, Value> {
    fields
        .iter()
        .map(|field| (field.name.clone(), field.kind.valid(seed)))
        .collect()
}

pub(crate) fn valid_case(fields: &[Field], seed: &mut Seed) -> Case {
    json_case("valid".to_owned(), &valid_object(fields, seed))
}

/// One case in eight is valid; the rest break one field or the body itself.
pub(crate) fn fuzz_case(fields: &[Field], seed: &mut Seed) -> Case {
    let valid = valid_object(fields, seed);
    let choice = seed.between(0, 7);
    if choice == 0 {
        return json_case("valid".to_owned(), &valid);
    }
    if choice <= 4 && !fields.is_empty() {
        return broken_field(fields, valid, seed);
    }
    if choice == 5 {
        let mut with_unknown = valid;
        with_unknown.insert(format!("fuzz_{}", seed.between(1, 99)), json!("unexpected"));
        return json_case("unknown field".to_owned(), &with_unknown);
    }
    broken_body(&valid, seed)
}

fn broken_field(fields: &[Field], mut body: Map<String, Value>, seed: &mut Seed) -> Case {
    let field = seed.pick(fields);
    let breakage = *seed.pick(&BREAKAGES);
    let label = format!("{} {}", breakage.name(), field.name);
    let candidates = match breakage {
        Breakage::Missing => {
            body.remove(&field.name);
            return json_case(label, &body);
        }
        Breakage::Boundary => field.kind.boundaries(),
        Breakage::Malformed => field.kind.malformed(),
        Breakage::Oversized => field.kind.oversized(),
        Breakage::WrongType => field.kind.wrong_types(seed),
    };
    if !candidates.is_empty() {
        body.insert(field.name.clone(), seed.pick(&candidates).clone());
    }
    json_case(label, &body)
}

fn broken_body(valid: &Map<String, Value>, seed: &mut Seed) -> Case {
    let text = Value::Object(valid.clone()).to_string();
    let case = |label: &str, content_type, body: String| Case {
        label: label.to_owned(),
        content_type,
        body,
    };
    match seed.between(0, 6) {
        0 => case("empty body", JSON, String::new()),
        1 => {
            let cut = text.floor_char_boundary(text.len().saturating_sub(1));
            case("truncated JSON", JSON, text[..cut].to_owned())
        }
        2 => case("not JSON", JSON, "not json".to_owned()),
        3 => case("top-level array", JSON, format!("[{text}]")),
        4 => case(
            "deeply nested",
            JSON,
            format!("{}{}", "[".repeat(NESTING), "]".repeat(NESTING)),
        ),
        5 => case("as a form", FORM, form(valid)),
        _ => case("wrong content type", "text/plain", text),
    }
}

fn json_case(label: String, body: &Map<String, Value>) -> Case {
    let text = Value::Object(body.clone()).to_string();
    let marker = Value::from(RAW_MARKER).to_string();
    Case {
        label,
        content_type: JSON,
        body: text.replace(&marker, HUGE_NUMBER),
    }
}

/// Nested values as JSON text, which a form never carries.
fn form(body: &Map<String, Value>) -> String {
    let fields: Vec<(&str, String)> = body
        .iter()
        .map(|(name, value)| {
            let text = match value {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            (name.as_str(), text)
        })
        .collect();
    percent::form(fields.iter().map(|(name, text)| (*name, text.as_str())))
}

/// A query a GET route may read: paging, sorting and filtering gone wrong.
pub(crate) fn fuzz_query(seed: &mut Seed) -> String {
    const QUERIES: [&str; 8] = [
        "",
        "?page=0",
        "?page=-1",
        "?page=184467440737095516160000",
        "?sort=%27%20OR%201%3D1",
        "?sort=-nonexistent&filter%5Bnope%5D=1",
        "?q=%00%FF",
        "?page=2&page=3",
    ];
    (*seed.pick(&QUERIES)).to_owned()
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use sofie_model::TypeName;
    use sofie_values::Seed;

    use super::{Field, Kind, Valued, fuzz_case, valid_case};

    fn ty(name: &str, arguments: Vec<TypeName>) -> TypeName {
        TypeName {
            name: name.to_owned(),
            arguments,
        }
    }

    fn fields() -> Vec<Field> {
        vec![
            Field::of("total", &ty("Money", vec![]), &[]),
            Field::of("note", &ty("Option", vec![ty("String", vec![])]), &[]),
            Field::of("count", &ty("i32", vec![]), &[]),
        ]
    }

    #[test]
    fn a_field_is_read_from_its_type() {
        let fields = fields();

        assert_eq!(fields[0].kind, Kind::Value(Valued::Money));
        assert_eq!(
            (fields[1].kind.clone(), fields[1].optional),
            (Kind::Text, true)
        );
        assert_eq!(fields[2].kind, Kind::Integer);
        let id = Field::of("order", &ty("OrderId", vec![]), &[]);
        assert_eq!(id.kind, Kind::Uuid);
    }

    #[test]
    fn the_same_seed_makes_the_same_cases_and_another_seed_others() {
        let run = |seed| {
            let mut seed = Seed::new(seed);
            (0..40)
                .map(|_| fuzz_case(&fields(), &mut seed))
                .collect::<Vec<_>>()
        };

        assert_eq!(run(7), run(7));
        assert_ne!(run(7), run(8));
    }

    #[test]
    fn the_cases_cover_valid_broken_fields_and_broken_bodies() {
        let mut seed = Seed::new(1);
        let labels: Vec<String> = (0..400)
            .map(|_| fuzz_case(&fields(), &mut seed).label)
            .collect();

        for expected in [
            "valid",
            "unknown field",
            "boundary total",
            "malformed total",
            "oversized count",
            "wrong type note",
            "missing count",
            "empty body",
            "truncated JSON",
            "deeply nested",
            "as a form",
        ] {
            assert!(
                labels.iter().any(|label| label == expected),
                "{expected} in {labels:?}"
            );
        }
    }

    #[test]
    fn a_valid_case_holds_every_field_and_an_oversized_number_is_written_as_is() {
        let valid = valid_case(&fields(), &mut Seed::new(3));
        let body: Value = serde_json::from_str(&valid.body).unwrap();
        assert!(body["total"]["minor"].is_i64(), "{body}");
        assert!(body["note"].is_string() && body["count"].is_i64(), "{body}");

        let mut seed = Seed::new(5);
        let oversized = (0..2000)
            .map(|_| fuzz_case(&fields(), &mut seed))
            .find(|case| case.body.contains("184467440737095516160000"));
        assert!(oversized.is_some());
    }
}
