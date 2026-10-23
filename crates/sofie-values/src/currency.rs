use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sofie_core::{Cast, ColumnReader, ColumnSpec, Error, Result, Show, Shown, SqlValue};

use crate::parsed::deserialize_parsed;

/// ISO 4217 list one as published on 2026-09-17: the codes in use and the digits of each
/// one's minor unit, sorted for a binary search. Codes without a minor unit (metals, SDR,
/// testing) are left out: no amount of money is written in them.
#[rustfmt::skip]
const CODES: [([u8; 3], u8); 165] = [
    (*b"AED", 2), (*b"AFN", 2), (*b"ALL", 2), (*b"AMD", 2), (*b"AOA", 2), (*b"ARS", 2),
    (*b"AUD", 2), (*b"AWG", 2), (*b"AZN", 2), (*b"BAM", 2), (*b"BBD", 2), (*b"BDT", 2),
    (*b"BHD", 3), (*b"BIF", 0), (*b"BMD", 2), (*b"BND", 2), (*b"BOB", 2), (*b"BOV", 2),
    (*b"BRL", 2), (*b"BSD", 2), (*b"BTN", 2), (*b"BWP", 2), (*b"BYN", 2), (*b"BZD", 2),
    (*b"CAD", 2), (*b"CDF", 2), (*b"CHE", 2), (*b"CHF", 2), (*b"CHW", 2), (*b"CLF", 4),
    (*b"CLP", 0), (*b"CNY", 2), (*b"COP", 2), (*b"COU", 2), (*b"CRC", 2), (*b"CUP", 2),
    (*b"CVE", 2), (*b"CZK", 2), (*b"DJF", 0), (*b"DKK", 2), (*b"DOP", 2), (*b"DZD", 2),
    (*b"EGP", 2), (*b"ERN", 2), (*b"ETB", 2), (*b"EUR", 2), (*b"FJD", 2), (*b"FKP", 2),
    (*b"GBP", 2), (*b"GEL", 2), (*b"GHS", 2), (*b"GIP", 2), (*b"GMD", 2), (*b"GNF", 0),
    (*b"GTQ", 2), (*b"GYD", 2), (*b"HKD", 2), (*b"HNL", 2), (*b"HTG", 2), (*b"HUF", 2),
    (*b"IDR", 2), (*b"ILS", 2), (*b"INR", 2), (*b"IQD", 3), (*b"IRR", 2), (*b"ISK", 0),
    (*b"JMD", 2), (*b"JOD", 3), (*b"JPY", 0), (*b"KES", 2), (*b"KGS", 2), (*b"KHR", 2),
    (*b"KMF", 0), (*b"KPW", 2), (*b"KRW", 0), (*b"KWD", 3), (*b"KYD", 2), (*b"KZT", 2),
    (*b"LAK", 2), (*b"LBP", 2), (*b"LKR", 2), (*b"LRD", 2), (*b"LSL", 2), (*b"LYD", 3),
    (*b"MAD", 2), (*b"MDL", 2), (*b"MGA", 2), (*b"MKD", 2), (*b"MMK", 2), (*b"MNT", 2),
    (*b"MOP", 2), (*b"MRU", 2), (*b"MUR", 2), (*b"MVR", 2), (*b"MWK", 2), (*b"MXN", 2),
    (*b"MXV", 2), (*b"MYR", 2), (*b"MZN", 2), (*b"NAD", 2), (*b"NGN", 2), (*b"NIO", 2),
    (*b"NOK", 2), (*b"NPR", 2), (*b"NZD", 2), (*b"OMR", 3), (*b"PAB", 2), (*b"PEN", 2),
    (*b"PGK", 2), (*b"PHP", 2), (*b"PKR", 2), (*b"PLN", 2), (*b"PYG", 0), (*b"QAR", 2),
    (*b"RON", 2), (*b"RSD", 2), (*b"RUB", 2), (*b"RWF", 0), (*b"SAR", 2), (*b"SBD", 2),
    (*b"SCR", 2), (*b"SDG", 2), (*b"SEK", 2), (*b"SGD", 2), (*b"SHP", 2), (*b"SLE", 2),
    (*b"SOS", 2), (*b"SRD", 2), (*b"SSP", 2), (*b"STN", 2), (*b"SVC", 2), (*b"SYP", 2),
    (*b"SZL", 2), (*b"THB", 2), (*b"TJS", 2), (*b"TMT", 2), (*b"TND", 3), (*b"TOP", 2),
    (*b"TRY", 2), (*b"TTD", 2), (*b"TWD", 2), (*b"TZS", 2), (*b"UAH", 2), (*b"UGX", 0),
    (*b"USD", 2), (*b"USN", 2), (*b"UYI", 0), (*b"UYU", 2), (*b"UYW", 4), (*b"UZS", 2),
    (*b"VED", 2), (*b"VES", 2), (*b"VND", 0), (*b"VUV", 0), (*b"WST", 2), (*b"XAD", 2),
    (*b"XAF", 0), (*b"XCD", 2), (*b"XCG", 2), (*b"XOF", 0), (*b"XPF", 0), (*b"YER", 2),
    (*b"ZAR", 2), (*b"ZMW", 2), (*b"ZWG", 2),
];

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Currency {
    code: [u8; 3],
    minor_digits: u8,
}

impl Currency {
    pub fn parse(input: &str) -> Result<Self> {
        let index = CODES
            .binary_search_by(|(code, _)| code.as_slice().cmp(input.as_bytes()))
            .map_err(|_| invalid_currency())?;
        let (code, minor_digits) = CODES[index];
        Ok(Self { code, minor_digits })
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.code).expect("a currency code is ASCII letters")
    }

    #[must_use]
    pub fn minor_digits(&self) -> u8 {
        self.minor_digits
    }

    /// Every code in use, as a client runtime formats amounts with them.
    pub fn all() -> impl Iterator<Item = Self> {
        CODES
            .iter()
            .map(|&(code, minor_digits)| Self { code, minor_digits })
    }
}

fn invalid_currency() -> Error {
    Error::invalid(
        "currency",
        "must be an ISO 4217 code in use, in uppercase, such as EUR",
    )
}

impl fmt::Display for Currency {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl fmt::Debug for Currency {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Currency({})", self.as_str())
    }
}

impl Show for Currency {
    fn show(&self) -> Shown<'_> {
        Shown::Text(self.as_str())
    }
}

impl Cast for Currency {
    const COLUMNS: &'static [ColumnSpec] = String::COLUMNS;

    fn encode(&self, out: &mut Vec<SqlValue>) {
        out.push(SqlValue::Text(self.as_str().to_owned()));
    }

    fn decode(row: &mut dyn ColumnReader) -> Result<Self> {
        Self::parse(row.text()?)
            .map_err(|error| Error::internal("stored currency is not an ISO 4217 code", error))
    }
}

impl Serialize for Currency {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Currency {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize_parsed(deserializer, "an ISO 4217 currency code", Self::parse)
    }
}

#[cfg(test)]
mod tests {
    use serde::de::{self, Visitor};
    use sofie_core::RowReader;

    use super::*;
    use crate::testing::{assert_refused, round_trip};

    #[test]
    fn accepts_codes_in_use() {
        for code in ["EUR", "USD", "TRY", "JPY"] {
            assert_eq!(Currency::parse(code).unwrap().as_str(), code);
        }
    }

    #[test]
    fn rejects_anything_else() {
        assert_refused(
            Currency::parse,
            &[
                "", "EU", "EURO", "eur", "Eur", "E1R", " EUR", "ÉUR", "€", "QQQ", "XXX", "XAU",
                "DEM", "BGN",
            ],
        );
    }

    #[test]
    fn knows_the_digits_of_its_minor_unit() {
        for (code, digits) in [
            ("EUR", 2),
            ("USD", 2),
            ("JPY", 0),
            ("XPF", 0),
            ("KWD", 3),
            ("CLF", 4),
            ("UYW", 4),
            ("UYI", 0),
        ] {
            assert_eq!(
                Currency::parse(code).unwrap().minor_digits(),
                digits,
                "{code}"
            );
        }
    }

    #[test]
    fn displays_as_its_code() {
        assert_eq!(Currency::parse("GBP").unwrap().to_string(), "GBP");
    }

    #[test]
    fn round_trips_through_one_text_column() {
        let currency = Currency::parse("EUR").unwrap();
        assert_eq!(round_trip(&currency), vec![SqlValue::Text("EUR".into())]);
    }

    #[test]
    fn an_invalid_stored_code_is_an_internal_error() {
        let values = [SqlValue::Text("euro".into())];
        let mut row = RowReader::new(&values);

        assert!(matches!(
            Currency::decode(&mut row),
            Err(Error::Internal { .. })
        ));
    }

    #[test]
    fn round_trips_through_json_as_a_string() {
        let currency = Currency::parse("USD").unwrap();
        let json = serde_json::to_string(&currency).unwrap();

        assert_eq!(json, "\"USD\"");
        assert_eq!(serde_json::from_str::<Currency>(&json).unwrap(), currency);
    }

    #[test]
    fn json_deserialization_validates_the_code() {
        assert!(serde_json::from_str::<Currency>("\"usd\"").is_err());
    }

    /// Answers only a borrowed string, as a page's JSON does.
    struct BorrowedOnly<'de>(&'de str);

    impl<'de> Deserializer<'de> for BorrowedOnly<'de> {
        type Error = serde::de::value::Error;

        fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Self::Error> {
            Err(de::Error::custom("only a borrowed str is offered"))
        }

        fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
            visitor.visit_borrowed_str(self.0)
        }

        serde::forward_to_deserialize_any! {
            bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char string bytes byte_buf
            option unit unit_struct newtype_struct seq tuple tuple_struct map struct enum
            identifier ignored_any
        }
    }

    #[test]
    fn deserializes_from_a_borrowed_str() {
        let currency = Currency::deserialize(BorrowedOnly("JPY")).unwrap();

        assert_eq!(currency, Currency::parse("JPY").unwrap());
        assert!(Currency::deserialize(BorrowedOnly("jpy")).is_err());
    }

    #[test]
    fn every_listed_code_reads_back_as_written() {
        for (code, digits) in CODES {
            let code = std::str::from_utf8(&code).unwrap();
            let currency = Currency::parse(code).unwrap();

            assert_eq!(currency.as_str(), code);
            assert_eq!(currency.minor_digits(), digits, "{code}");
        }
    }

    #[test]
    fn the_listed_codes_are_sorted_uppercase_letters() {
        assert!(CODES.is_sorted_by(|(first, _), (second, _)| first < second));
        assert!(
            CODES
                .iter()
                .all(|(code, _)| code.iter().all(u8::is_ascii_uppercase))
        );
    }
}
