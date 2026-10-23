use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sofie_core::{Cast, ColumnReader, ColumnSpec, Error, Result, Show, Shown, SqlType, SqlValue};

use crate::parsed::deserialize_parsed;
use crate::{Input, Present};

/// Sorted for a binary search. Codes are prefix-free: `1` and `7` are the only one-digit codes and
/// every unlisted prefix starts a three-digit code, so no table of every country is needed.
const TWO_DIGIT_CODES: [u16; 44] = [
    20, 27, 30, 31, 32, 33, 34, 36, 39, 40, 41, 43, 44, 45, 46, 47, 48, 49, 51, 52, 53, 54, 55, 56,
    57, 58, 60, 61, 62, 63, 64, 65, 66, 81, 82, 84, 86, 90, 91, 92, 93, 94, 95, 98,
];
/// E.164 caps a number at 15 digits, its country code included.
const MAXIMUM_DIGITS: usize = 15;
const MINIMUM_NATIONAL_DIGITS: usize = 4;
const SEPARATORS: [char; 5] = [' ', '-', '.', '(', ')'];
/// Italy, San Marino, the Vatican, Côte d'Ivoire and the Republic of the Congo dial their leading
/// zero from abroad too.
const ZERO_KEPT: [u16; 5] = [39, 225, 242, 378, 379];

/// Personal data: `Debug` masks it.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct PhoneNumber {
    country_code: u16,
    e164: String,
}

impl PhoneNumber {
    pub fn try_new(country_code: u16, national: &str) -> Result<Self> {
        let national = if ZERO_KEPT.contains(&country_code) {
            national
        } else {
            without_trunk_zero(national)?
        };
        let code = country_code.to_string();
        if code_length(&code) != Some(code.len()) {
            return Err(invalid_phone("must start with a country calling code"));
        }
        if national.len() < MINIMUM_NATIONAL_DIGITS
            || !national.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(invalid_phone(
                "must have a number of at least 4 digits after the country code",
            ));
        }
        if code.len() + national.len() > MAXIMUM_DIGITS {
            return Err(invalid_phone("must have at most 15 digits"));
        }
        Ok(Self {
            country_code,
            e164: format!("+{code}{national}"),
        })
    }

    pub fn parse(text: &str) -> Result<Self> {
        let digits: String = text.trim().replace(SEPARATORS, "");
        let digits = digits
            .strip_prefix('+')
            .ok_or_else(|| invalid_phone("must start with + and the country code"))?;
        if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid_phone(
                "must have only digits after the +, apart from spaces, dots, hyphens and brackets",
            ));
        }
        let length = code_length(digits)
            .filter(|&length| digits.len() > length)
            .ok_or_else(|| invalid_phone("must start with a country calling code"))?;
        let (code, national) = digits.split_at(length);
        let country_code = code.parse().expect("at most three digits fit a u16");
        Self::try_new(country_code, national)
    }

    #[must_use]
    pub fn country_code(&self) -> u16 {
        self.country_code
    }

    #[must_use]
    pub fn national_number(&self) -> &str {
        let code_digits = self.country_code.ilog10() as usize + 1;
        &self.e164[1 + code_digits..]
    }

    #[must_use]
    pub fn e164(&self) -> &str {
        &self.e164
    }
}

fn without_trunk_zero(national: &str) -> Result<&str> {
    let national = national.strip_prefix('0').unwrap_or(national);
    if national.starts_with('0') {
        return Err(invalid_phone(
            "must not start with 0 after the country code, apart from one trunk zero",
        ));
    }
    Ok(national)
}

fn code_length(digits: &str) -> Option<usize> {
    match digits.as_bytes().first()? {
        b'1' | b'7' => return Some(1),
        b'0' => return None,
        _ => {}
    }
    let prefix = digits.get(..2)?.parse::<u16>().ok()?;
    if TWO_DIGIT_CODES.binary_search(&prefix).is_ok() {
        return Some(2);
    }
    Some(3)
}

fn invalid_phone(message: &str) -> Error {
    Error::invalid("phone", message)
}

impl fmt::Debug for PhoneNumber {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tail = &self.e164[self.e164.len() - 2..];
        write!(formatter, "PhoneNumber(+{} ***{tail})", self.country_code)
    }
}

impl Cast for PhoneNumber {
    const COLUMNS: &'static [ColumnSpec] = &[
        ColumnSpec::new("_code", SqlType::Int).unordered(),
        ColumnSpec::new("_number", SqlType::Text),
    ];

    fn encode(&self, out: &mut Vec<SqlValue>) {
        out.push(SqlValue::Int(i64::from(self.country_code)));
        out.push(SqlValue::Text(self.national_number().to_owned()));
    }

    fn decode(row: &mut dyn ColumnReader) -> Result<Self> {
        let code = i64::decode(row)?;
        let code = u16::try_from(code)
            .map_err(|error| Error::internal("a stored country code is out of range", error))?;
        Self::try_new(code, row.text()?)
            .map_err(|error| Error::internal("a stored phone number is not valid", error))
    }
}

impl Serialize for PhoneNumber {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.e164)
    }
}

impl Show for PhoneNumber {
    fn show(&self) -> Shown<'_> {
        Shown::Text(&self.e164)
    }
}

impl<'de> Deserialize<'de> for PhoneNumber {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize_parsed(deserializer, "an international phone number", Self::parse)
    }
}

impl Present for PhoneNumber {
    const INPUT: Input = Input::typed(&[("type", "tel"), ("autocomplete", "tel")]);
    const SEARCH: crate::Search = crate::Search::Exact;

    fn csv(&self) -> String {
        self.e164.clone()
    }
}

#[cfg(test)]
mod tests {
    use sofie_core::RowReader;

    use super::*;
    use crate::testing::{assert_refused, round_trip};

    fn phone(text: &str) -> PhoneNumber {
        PhoneNumber::parse(text).unwrap()
    }

    #[test]
    fn splits_the_country_code_by_its_prefix() {
        for (text, code, national) in [
            ("+1 (415) 555-0100", 1, "4155550100"),
            ("+7 912 345 67 89", 7, "9123456789"),
            ("+44 20 7946 0958", 44, "2079460958"),
            ("+90 532 123 45 67", 90, "5321234567"),
            ("+353 1 234 5678", 353, "12345678"),
            ("+971 50 123 4567", 971, "501234567"),
            ("+39 06 1234 5678", 39, "0612345678"),
        ] {
            let number = phone(text);
            assert_eq!(
                (number.country_code(), number.national_number()),
                (code, national),
                "{text}"
            );
        }
    }

    #[test]
    fn drops_one_trunk_zero_after_the_country_code() {
        assert_eq!(phone("+44 (0)20 7946 0958"), phone("+44 20 7946 0958"));
        assert_eq!(phone("+49 (0) 30 123456").e164(), "+4930123456");
        assert_eq!(phone("+90 (0532) 123 45 67").e164(), "+905321234567");
        assert_eq!(phone("+90 0532 123 45 67").e164(), "+905321234567");
        assert_eq!(
            PhoneNumber::try_new(90, "05321234567").unwrap().e164(),
            "+905321234567"
        );
        assert_eq!(phone("+44 20 7946 (0)958").e164(), "+442079460958");
    }

    #[test]
    fn refuses_a_national_number_still_starting_with_zero_after_the_trunk_zero() {
        assert_refused(
            PhoneNumber::parse,
            &["+44 00 2079 4609", "+90 (0)0532 123 45 67"],
        );
        assert!(PhoneNumber::try_new(44, "0020794609").is_err());
    }

    #[test]
    fn keeps_the_leading_zero_where_it_is_part_of_the_number() {
        assert_eq!(phone("+39 06 1234 5678").e164(), "+390612345678");
        assert_eq!(phone("+378 0549 123456").e164(), "+3780549123456");
        assert_eq!(phone("+379 06 6982 0000").e164(), "+3790669820000");
        assert_eq!(phone("+225 07 0123 4567").e164(), "+2250701234567");
        assert_eq!(phone("+242 06 123 4567").e164(), "+242061234567");
    }

    #[test]
    fn refuses_what_is_not_an_international_number() {
        assert_refused(
            PhoneNumber::parse,
            &[
                "",
                "05321234567",
                "+",
                "+0 123 4567",
                "+90",
                "+90 123",
                "+90 532 abc 45 67",
                "+90 5321 2345 6789 01",
                "+1/415/555/0100",
            ],
        );
    }

    #[test]
    fn refuses_a_second_plus_before_the_country_code() {
        assert_refused(
            PhoneNumber::parse,
            &["++90 532 123 45 67", "+(+90) 532 123 45 67"],
        );
    }

    #[test]
    fn checks_a_code_given_apart_from_the_number() {
        assert!(PhoneNumber::try_new(90, "5321234567").is_ok());
        assert!(PhoneNumber::try_new(353, "12345678").is_ok());
        for code in [0, 4, 445, 1000] {
            assert!(PhoneNumber::try_new(code, "12345678").is_err(), "{code}");
        }
    }

    #[test]
    fn holds_at_most_fifteen_digits() {
        assert!(PhoneNumber::parse("+90 1234567890123").is_ok());
        assert!(PhoneNumber::parse("+90 12345678901234").is_err());
    }

    #[test]
    fn masks_itself_in_debug_and_writes_e164_on_demand() {
        let number = phone("+90 532 123 45 67");

        assert_eq!(format!("{number:?}"), "PhoneNumber(+90 ***67)");
        assert_eq!(number.e164(), "+905321234567");
    }

    #[test]
    fn round_trips_through_a_code_and_a_number_column() {
        let number = phone("+90 532 123 45 67");
        assert_eq!(
            round_trip(&number),
            vec![SqlValue::Int(90), SqlValue::Text("5321234567".into())]
        );
        let stored_badly = vec![SqlValue::Int(-1), SqlValue::Text("5321234567".into())];
        assert!(PhoneNumber::decode(&mut RowReader::new(&stored_badly)).is_err());
    }

    #[test]
    fn round_trips_through_json_as_e164() {
        let number = phone("+90 532 123 45 67");
        let json = serde_json::to_value(&number).unwrap();

        assert_eq!(json, serde_json::json!("+905321234567"));
        assert_eq!(serde_json::from_value::<PhoneNumber>(json).unwrap(), number);
        assert!(serde_json::from_value::<PhoneNumber>(serde_json::json!("5321234567")).is_err());
    }
}
