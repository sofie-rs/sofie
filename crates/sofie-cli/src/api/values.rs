//! Must match the `Serialize` and `Deserialize` impls in sofie-core and sofie-values.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scalar {
    Text,
    Uuid,
    Integer,
    Number,
    Boolean,
    DateTime,
    Date,
    Email,
    Uri,
}

impl Scalar {
    pub(crate) fn of(type_name: &str) -> Option<Self> {
        Some(match type_name {
            "String" | "PersonName" | "PhoneNumber" | "Slug" | "Locale" | "TimeZone"
            | "Currency" | "LocalDateTime" | "TimeOfDay" | "Weekday" | "Password" => Self::Text,
            "Uuid" => Self::Uuid,
            "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" | "usize" => Self::Integer,
            "f32" | "f64" | "Percentage" => Self::Number,
            "bool" => Self::Boolean,
            "Instant" => Self::DateTime,
            "Date" => Self::Date,
            "Email" => Self::Email,
            "Url" => Self::Uri,
            _ => return None,
        })
    }

    #[cfg(feature = "openapi")]
    pub(crate) fn format(self) -> Option<&'static str> {
        match self {
            Self::Uuid => Some("uuid"),
            Self::DateTime => Some("date-time"),
            Self::Date => Some("date"),
            Self::Email => Some("email"),
            Self::Uri => Some("uri"),
            Self::Text | Self::Integer | Self::Number | Self::Boolean => None,
        }
    }
}

pub(crate) type ValueShape = &'static [(&'static str, Scalar, bool)];

/// `Money` arrives as `minor` units from code or as the `amount` a person typed, and always leaves as `minor`.
pub(crate) const VALUE_SHAPES: [(&str, ValueShape); 5] = [
    (
        "Money",
        &[
            ("minor", Scalar::Integer, false),
            ("amount", Scalar::Text, false),
            ("currency", Scalar::Text, true),
        ],
    ),
    (
        "Coordinate",
        &[
            ("latitude", Scalar::Number, true),
            ("longitude", Scalar::Number, true),
        ],
    ),
    ("Length", MEASUREMENT),
    ("Weight", MEASUREMENT),
    ("Energy", MEASUREMENT),
];

const MEASUREMENT: ValueShape = &[
    ("quantity", Scalar::Number, true),
    ("unit", Scalar::Text, true),
];
