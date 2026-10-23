use jiff::civil;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sofie_core::variants::{self, Variants};
use sofie_core::{Cast, ColumnReader, ColumnSpec, Result, Show, Shown, SqlValue};

#[cfg(feature = "web")]
use crate::Conventions;
use crate::{Input, Present};

/// Stored and sent by its name, as an enum's variant is; its labels are the `weekday` enum's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Weekday {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

const NAMES: [&str; 7] = [
    "MONDAY",
    "TUESDAY",
    "WEDNESDAY",
    "THURSDAY",
    "FRIDAY",
    "SATURDAY",
    "SUNDAY",
];

const IN_ORDER: [Weekday; 7] = [
    Weekday::Monday,
    Weekday::Tuesday,
    Weekday::Wednesday,
    Weekday::Thursday,
    Weekday::Friday,
    Weekday::Saturday,
    Weekday::Sunday,
];

impl Weekday {
    /// For a day written in code in lowercase, such as a schedule's `monday`: a wrong one fails
    /// the build in a constant.
    #[must_use]
    pub const fn constant(name: &str) -> Self {
        let mut index = 0;
        while index < NAMES.len() {
            if lowercase_equals(NAMES[index].as_bytes(), name.as_bytes()) {
                return IN_ORDER[index];
            }
            index += 1;
        }
        panic!("a weekday is written in English and lowercase, such as monday");
    }
}

const fn lowercase_equals(upper: &[u8], written: &[u8]) -> bool {
    if upper.len() != written.len() {
        return false;
    }
    let mut index = 0;
    while index < upper.len() {
        if upper[index].to_ascii_lowercase() != written[index] {
            return false;
        }
        index += 1;
    }
    true
}

impl From<civil::Weekday> for Weekday {
    fn from(weekday: civil::Weekday) -> Self {
        let offset = weekday.to_monday_zero_offset().unsigned_abs();
        IN_ORDER[usize::from(offset)]
    }
}

impl Variants for Weekday {
    const TYPE: &'static str = "Weekday";
    const VARIANTS: &'static [(&'static str, Self)] = &[
        (NAMES[0], Self::Monday),
        (NAMES[1], Self::Tuesday),
        (NAMES[2], Self::Wednesday),
        (NAMES[3], Self::Thursday),
        (NAMES[4], Self::Friday),
        (NAMES[5], Self::Saturday),
        (NAMES[6], Self::Sunday),
    ];
}

impl Show for Weekday {
    fn show(&self) -> Shown<'_> {
        Shown::Text(self.name())
    }
}

impl Cast for Weekday {
    const COLUMNS: &'static [ColumnSpec] = &[variants::COLUMNS[0].with_values(&NAMES)];

    fn encode(&self, out: &mut Vec<SqlValue>) {
        variants::encode(*self, out);
    }

    fn decode(row: &mut dyn ColumnReader) -> Result<Self> {
        variants::decode(row)
    }
}

impl Serialize for Weekday {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        variants::serialize(*self, serializer)
    }
}

impl<'de> Deserialize<'de> for Weekday {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        variants::deserialize(deserializer)
    }
}

impl Present for Weekday {
    const INPUT: Input = Input::select(&NAMES);
    const SEARCH: crate::Search = crate::Search::Exact;

    fn csv(&self) -> String {
        self.name().to_owned()
    }

    #[cfg(feature = "web")]
    fn write_localized(&self, out: &mut String, conventions: &dyn Conventions) {
        conventions.write_variant(out, "weekday", self.name());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Date;
    use crate::testing::round_trip;

    #[test]
    fn a_day_falls_on_its_weekday() {
        let sunday = Date::parse("2026-10-25").unwrap();

        assert_eq!(sunday.weekday(), Weekday::Sunday);
        assert_eq!(
            Date::parse("2026-10-26").unwrap().weekday(),
            Weekday::Monday
        );
    }

    #[test]
    fn a_constant_is_its_lowercase_english_name() {
        assert_eq!(Weekday::constant("monday"), Weekday::Monday);
        assert_eq!(Weekday::constant("sunday"), Weekday::Sunday);
    }

    #[test]
    #[should_panic(expected = "a weekday is written in English and lowercase")]
    fn a_wrong_constant_day_fails() {
        let _ = Weekday::constant("Monday");
    }

    #[test]
    fn round_trips_through_its_name_like_an_enum() {
        assert_eq!(
            round_trip(&Weekday::Wednesday),
            vec![SqlValue::Text("WEDNESDAY".into())]
        );
        assert_eq!(Weekday::COLUMNS[0].values, NAMES);
        let json = serde_json::to_value(Weekday::Friday).unwrap();
        assert_eq!(json, serde_json::json!("FRIDAY"));
        assert_eq!(
            serde_json::from_value::<Weekday>(json).unwrap(),
            Weekday::Friday
        );
        assert!(serde_json::from_value::<Weekday>(serde_json::json!("friday")).is_err());
        assert_eq!(Weekday::INPUT.options, NAMES);
        assert_eq!(Weekday::Saturday.csv(), "SATURDAY");
    }
}
