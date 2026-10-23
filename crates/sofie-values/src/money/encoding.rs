use sofie_core::show::Fields;
use std::fmt;

use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sofie_core::{Cast, ColumnReader, ColumnSpec, Error, Result, Show, Shown, SqlType, SqlValue};

use crate::{Currency, Money};

impl Cast for Money {
    const COLUMNS: &'static [ColumnSpec] = &[
        ColumnSpec::new("", SqlType::Int),
        ColumnSpec::new("_currency", SqlType::Text).unordered(),
    ];

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.minor.encode(out);
        self.currency.encode(out);
    }

    fn decode(row: &mut dyn ColumnReader) -> Result<Self> {
        let minor = i64::decode(row)?;
        let currency = Currency::decode(row)?;
        Ok(Self::new(minor, currency))
    }
}

impl Fields for Money {
    fn name(&self, index: usize) -> Option<&str> {
        ["minor", "currency"].get(index).copied()
    }

    fn get(&self, name: &str) -> Option<Shown<'_>> {
        match name {
            "minor" => Some(Shown::Integer(self.minor)),
            "currency" => Some(self.currency.show()),
            _ => None,
        }
    }
}

impl Show for Money {
    fn show(&self) -> Shown<'_> {
        Shown::Record(self)
    }
}

const FIELDS: &[&str] = &["minor", "amount", "currency"];

impl Serialize for Money {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut fields = serializer.serialize_struct("Money", 2)?;
        fields.serialize_field("minor", &self.minor)?;
        fields.serialize_field("currency", &self.currency)?;
        fields.end()
    }
}

/// Minor units from code, or major units a person typed into a form.
#[derive(Default)]
struct MoneyInput {
    minor: Option<i64>,
    amount: Option<String>,
    currency: Option<Currency>,
}

impl MoneyInput {
    fn into_money(self) -> Result<Money> {
        let currency = self
            .currency
            .ok_or_else(|| Error::invalid("currency", "is required"))?;
        match (self.minor, self.amount) {
            (Some(minor), None) => Ok(Money::new(minor, currency)),
            (None, Some(amount)) => Money::parse_major(&amount, currency),
            (None, None) => Err(Error::invalid("amount", "is required")),
            (Some(_), Some(_)) => Err(Error::invalid(
                "amount",
                "needs either `minor` or `amount`, not both",
            )),
        }
    }
}

/// Written by hand so the core values need no `serde_derive`.
impl<'de> Visitor<'de> for MoneyInput {
    type Value = Money;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an object of `currency` and `minor` or `amount`")
    }

    fn visit_map<A: MapAccess<'de>>(mut self, mut map: A) -> Result<Money, A::Error> {
        while let Some(field) = map.next_key::<String>()? {
            match field.as_str() {
                "minor" => self.minor = Some(map.next_value()?),
                "amount" => self.amount = Some(map.next_value()?),
                "currency" => self.currency = Some(map.next_value()?),
                _ => return Err(de::Error::unknown_field(&field, FIELDS)),
            }
        }
        self.into_money().map_err(Error::into_deserialize_error)
    }
}

impl<'de> Deserialize<'de> for Money {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_struct("Money", FIELDS, MoneyInput::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{euros, round_trip, suffixes};

    #[test]
    fn spans_an_amount_and_a_currency_column() {
        assert_eq!(suffixes::<Money>(), ["", "_currency"]);
    }

    #[test]
    fn round_trips_through_its_columns() {
        let money = euros(14990);
        assert_eq!(
            round_trip(&money),
            vec![SqlValue::Int(14990), SqlValue::Text("EUR".into())]
        );
    }

    #[test]
    fn round_trips_through_json() {
        let json = serde_json::to_value(euros(14990)).unwrap();

        assert_eq!(json, serde_json::json!({"minor": 14990, "currency": "EUR"}));
        assert_eq!(serde_json::from_value::<Money>(json).unwrap(), euros(14990));
    }

    #[test]
    fn json_deserialization_validates_the_currency() {
        let json = serde_json::json!({"minor": 1, "currency": "euro"});

        assert!(serde_json::from_value::<Money>(json).is_err());
    }

    #[test]
    fn json_deserialization_refuses_a_float_amount() {
        let json = serde_json::json!({"minor": 149.9, "currency": "EUR"});

        assert!(serde_json::from_value::<Money>(json).is_err());
    }

    #[test]
    fn json_deserialization_reads_a_typed_amount() {
        let json = serde_json::json!({"amount": "149.90", "currency": "EUR"});

        assert_eq!(serde_json::from_value::<Money>(json).unwrap(), euros(14990));
    }

    #[test]
    fn json_deserialization_validates_a_typed_amount() {
        for json in [
            serde_json::json!({"amount": "149.999", "currency": "EUR"}),
            serde_json::json!({"amount": "1e3", "currency": "EUR"}),
            serde_json::json!({"amount": 149.9, "currency": "EUR"}),
            serde_json::json!({"amount": "1", "currency": "euro"}),
            serde_json::json!({"amount": "1"}),
        ] {
            assert!(
                serde_json::from_value::<Money>(json.clone()).is_err(),
                "{json}"
            );
        }
        let error = serde_json::from_value::<Money>(
            serde_json::json!({"amount": "149.999", "currency": "EUR"}),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "EUR amounts have at most 2 decimals");
        let missing = serde_json::from_value::<Money>(serde_json::json!({"currency": "EUR"}));
        assert_eq!(missing.unwrap_err().to_string(), "is required");
    }

    #[test]
    fn json_deserialization_needs_exactly_one_amount_shape() {
        for json in [
            serde_json::json!({"minor": 1, "amount": "0.01", "currency": "EUR"}),
            serde_json::json!({"currency": "EUR"}),
            serde_json::json!({"minor": 1, "currency": "EUR", "note": "x"}),
        ] {
            assert!(
                serde_json::from_value::<Money>(json.clone()).is_err(),
                "{json}"
            );
        }
    }
}
