use std::fmt::{self, Debug};
use std::hash::Hash;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sofie_core::show::Fields;
use sofie_core::{Cast, ColumnReader, ColumnSpec, Error, Result, Show, Shown, SqlType, SqlValue};

#[cfg(feature = "web")]
use crate::Conventions;
use crate::decimal::{Refusal, divide_rounded, parse_scaled, scaled_text};
use crate::{Control, Input, Part, Present};

const DECIMALS: u8 = 3;
const THOUSANDTHS_PER_UNIT: f64 = 1000.0;
/// Dividing by 1000 into the float column and multiplying back each err by at most half an ulp;
/// up to 2^50 their sum stays below half a thousandth, so rounding restores the integer.
const MAXIMUM_THOUSANDTHS: i64 = 1 << 50;

/// Each unit is a whole number of the quantity's smallest step: conversion is integer arithmetic.
pub trait Unit: Copy + Eq + Hash + Debug + 'static {
    const ALL: &'static [Self];
    const NAMES: &'static [&'static str];

    fn name(self) -> &'static str;

    fn steps(self) -> i64;
}

/// Equality compares the unit too: convert before comparing across units.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Measurement<U> {
    quantity: Thousandths,
    unit: U,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Thousandths(i64);

impl fmt::Display for Thousandths {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&scaled_text(self.0, DECIMALS))
    }
}

impl<U: Unit> Measurement<U> {
    pub fn try_new(thousandths: i64, unit: U) -> Result<Self> {
        if !(-MAXIMUM_THOUSANDTHS..=MAXIMUM_THOUSANDTHS).contains(&thousandths) {
            return Err(Error::invalid("quantity", "is out of range"));
        }
        Ok(Self {
            quantity: Thousandths(thousandths),
            unit,
        })
    }

    pub fn parse(quantity: &str, unit: U) -> Result<Self> {
        let thousandths = parse_scaled(quantity, DECIMALS).map_err(|refusal| {
            let message = match refusal {
                Refusal::TooManyDecimals => "has at most 3 decimals",
                other => other.message(),
            };
            Error::invalid("quantity", message)
        })?;
        Self::try_new(thousandths, unit)
    }

    #[must_use]
    pub fn thousandths(self) -> i64 {
        self.quantity.0
    }

    #[must_use]
    pub fn unit(self) -> U {
        self.unit
    }

    #[must_use]
    pub fn quantity(self) -> String {
        self.quantity.to_string()
    }

    pub fn convert(self, unit: U) -> Result<Self> {
        let steps = i128::from(self.quantity.0) * i128::from(self.unit.steps());
        divide_rounded(steps, i128::from(unit.steps()))
            .and_then(|thousandths| Self::try_new(thousandths, unit).ok())
            .ok_or_else(|| {
                Error::Rule(format!(
                    "converting {} {} to {} leaves the range of a quantity",
                    self.quantity,
                    self.unit.name(),
                    unit.name()
                ))
            })
    }
}

fn unit_named<U: Unit>(name: &str) -> Result<U> {
    U::ALL
        .iter()
        .copied()
        .find(|unit| unit.name() == name)
        .ok_or_else(|| Error::invalid("unit", format!("must be one of {}", U::NAMES.join(", "))))
}

impl<U: Unit> Show for Measurement<U> {
    fn show(&self) -> Shown<'_> {
        Shown::Record(self)
    }
}

impl<U: Unit> Fields for Measurement<U> {
    fn name(&self, index: usize) -> Option<&str> {
        ["quantity", "unit"].get(index).copied()
    }

    fn get(&self, name: &str) -> Option<Shown<'_>> {
        match name {
            "quantity" => Some(Shown::Formatted(&self.quantity)),
            "unit" => Some(Shown::Text(self.unit.name())),
            _ => None,
        }
    }
}

/// Whole thousandths survive the float column exactly. Unordered: the quantity alone would put
/// 500 grams after 2 kilograms.
impl<U: Unit> Cast for Measurement<U> {
    const COLUMNS: &'static [ColumnSpec] = &[
        ColumnSpec::new("", SqlType::Float).unordered(),
        ColumnSpec::new("_unit", SqlType::Text).unordered(),
    ];

    fn encode(&self, out: &mut Vec<SqlValue>) {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a quantity is at most 2^50 thousandths, which a float holds exactly"
        )]
        let quantity = self.quantity.0 as f64 / THOUSANDTHS_PER_UNIT;
        out.push(SqlValue::Float(quantity));
        out.push(SqlValue::Text(self.unit.name().to_owned()));
    }

    fn decode(row: &mut dyn ColumnReader) -> Result<Self> {
        let thousandths = (f64::decode(row)? * THOUSANDTHS_PER_UNIT).round();
        let unit = unit_named(row.text()?)
            .map_err(|error| Error::internal("a stored unit is not known", error))?;
        if !thousandths.is_finite() {
            return Err(Error::internal_message("a stored quantity is not a number"));
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a cast saturates, and `try_new` refuses whatever lies beyond 2^50"
        )]
        Self::try_new(thousandths as i64, unit)
            .map_err(|error| Error::internal("a stored quantity is out of range", error))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MeasurementFields {
    quantity: String,
    unit: String,
}

impl<U: Unit> Serialize for Measurement<U> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let fields = MeasurementFields {
            quantity: self.quantity(),
            unit: self.unit.name().to_owned(),
        };
        fields.serialize(serializer)
    }
}

impl<'de, U: Unit> Deserialize<'de> for Measurement<U> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let fields = MeasurementFields::deserialize(deserializer)?;
        unit_named(&fields.unit)
            .and_then(|unit| Self::parse(&fields.quantity, unit))
            .map_err(Error::into_deserialize_error)
    }
}

macro_rules! units {
    ($(#[$doc:meta])* $unit:ident { $($variant:ident = $name:literal, $steps:literal;)+ }) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $unit {
            $($variant),+
        }

        impl Unit for $unit {
            const ALL: &'static [Self] = &[$(Self::$variant),+];
            const NAMES: &'static [&'static str] = &[$($name),+];

            fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $name),+
                }
            }

            fn steps(self) -> i64 {
                match self {
                    $(Self::$variant => $steps),+
                }
            }
        }
    };
}

units! {
    /// Steps of a nanogram: an ounce is not a whole microgram.
    WeightUnit {
        Milligram = "MILLIGRAM", 1_000_000;
        Gram = "GRAM", 1_000_000_000;
        Kilogram = "KILOGRAM", 1_000_000_000_000;
        Ounce = "OUNCE", 28_349_523_125;
        Pound = "POUND", 453_592_370_000;
    }
}

units! {
    EnergyUnit {
        Joule = "JOULE", 1_000;
        Kilojoule = "KILOJOULE", 1_000_000;
        Calorie = "CALORIE", 4_184;
        Kilocalorie = "KILOCALORIE", 4_184_000;
        KilowattHour = "KILOWATT_HOUR", 3_600_000_000;
    }
}

units! {
    LengthUnit {
        Millimetre = "MILLIMETRE", 1_000;
        Centimetre = "CENTIMETRE", 10_000;
        Metre = "METRE", 1_000_000;
        Kilometre = "KILOMETRE", 1_000_000_000;
        Inch = "INCH", 25_400;
        Foot = "FOOT", 304_800;
        Mile = "MILE", 1_609_344_000;
    }
}

pub type Weight = Measurement<WeightUnit>;
pub type Energy = Measurement<EnergyUnit>;
pub type Length = Measurement<LengthUnit>;

impl<U: Unit> Present for Measurement<U> {
    const INPUT: Input = Input::new(Control::Parts(&[
        Part {
            name: "quantity",
            attributes: &[("type", "text"), ("inputmode", "decimal")],
            options: &[],
        },
        Part {
            name: "unit",
            attributes: &[],
            options: U::NAMES,
        },
    ]));
    const SEARCH: crate::Search = crate::Search::Number;

    fn csv(&self) -> String {
        format!("{} {}", self.quantity, self.unit.name())
    }

    #[cfg(feature = "web")]
    fn write_localized(&self, out: &mut String, conventions: &dyn Conventions) {
        conventions.write_number(out, &self.quantity());
        out.push(' ');
        out.push_str(self.unit.name());
    }
}

#[cfg(test)]
mod tests {
    use sofie_core::RowReader;

    use super::*;
    use crate::testing::{round_trip, suffixes};

    fn kilograms(quantity: &str) -> Weight {
        Weight::parse(quantity, WeightUnit::Kilogram).unwrap()
    }

    #[test]
    fn no_column_sorts_a_list_since_units_differ_row_to_row() {
        assert!(Weight::COLUMNS.iter().all(|column| !column.orders));
    }

    #[test]
    fn reads_and_writes_a_quantity_to_a_thousandth() {
        assert_eq!(kilograms("1.5").thousandths(), 1_500);
        assert_eq!(kilograms("1.5").quantity(), "1.500");
        assert_eq!(kilograms("-0.001").thousandths(), -1);
        for (text, message) in [
            ("1.0005", "has at most 3 decimals"),
            (
                "1,5",
                "must use one `.` before the decimals and no other separator",
            ),
            ("", "must be a number such as 149.90"),
        ] {
            let refused = Weight::parse(text, WeightUnit::Gram).unwrap_err();
            assert_eq!(format!("{refused:#}"), message, "{text:?}");
        }
    }

    #[test]
    fn converts_exactly_then_rounds_once() {
        let pound = Weight::parse("1", WeightUnit::Pound).unwrap();

        assert_eq!(
            pound.convert(WeightUnit::Kilogram).unwrap(),
            kilograms("0.454")
        );
        assert_eq!(
            pound.convert(WeightUnit::Ounce).unwrap(),
            Weight::parse("16", WeightUnit::Ounce).unwrap()
        );
        let mile = Length::parse("1", LengthUnit::Mile).unwrap();
        assert_eq!(
            mile.convert(LengthUnit::Foot).unwrap().quantity(),
            "5280.000"
        );
        assert_eq!(
            mile.convert(LengthUnit::Inch).unwrap().quantity(),
            "63360.000"
        );
        let snack = Energy::parse("250", EnergyUnit::Kilocalorie).unwrap();
        assert_eq!(
            snack.convert(EnergyUnit::Kilojoule).unwrap().quantity(),
            "1046.000"
        );
        assert_eq!(
            Energy::parse("1", EnergyUnit::KilowattHour)
                .unwrap()
                .convert(EnergyUnit::Joule)
                .unwrap()
                .quantity(),
            "3600000.000"
        );
    }

    #[test]
    fn refuses_a_conversion_that_overflows() {
        let huge = Length::try_new(1 << 50, LengthUnit::Kilometre).unwrap();

        assert!(matches!(
            huge.convert(LengthUnit::Millimetre),
            Err(Error::Rule(_))
        ));
        assert_eq!(huge.convert(LengthUnit::Kilometre).unwrap(), huge);
        assert!(Length::parse("-1125899906842.625", LengthUnit::Metre).is_err());
    }

    #[test]
    fn round_trips_through_a_quantity_and_a_unit_column() {
        let weight = kilograms("1.5");
        assert_eq!(
            round_trip(&weight),
            vec![SqlValue::Float(1.5), SqlValue::Text("KILOGRAM".into())]
        );
        assert_eq!(suffixes::<Weight>(), ["", "_unit"]);
    }

    #[test]
    fn every_accepted_quantity_round_trips_through_the_float_column() {
        for thousandths in [1 << 50, (1 << 50) - 1, (1 << 50) - 7, -(1 << 50)] {
            round_trip(&Length::try_new(thousandths, LengthUnit::Metre).unwrap());
        }
        assert!(Length::try_new(9_007_199_254_540_993, LengthUnit::Metre).is_err());
        assert!(Length::try_new((1 << 50) + 1, LengthUnit::Metre).is_err());
    }

    #[test]
    fn refuses_a_stored_unit_of_another_quantity() {
        let stored = vec![SqlValue::Float(1.0), SqlValue::Text("METRE".into())];
        let infinite = vec![
            SqlValue::Float(f64::INFINITY),
            SqlValue::Text("GRAM".into()),
        ];

        assert!(Weight::decode(&mut RowReader::new(&stored)).is_err());
        assert!(Weight::decode(&mut RowReader::new(&infinite)).is_err());
    }

    #[test]
    fn round_trips_through_json() {
        let weight = kilograms("0.25");
        let json = serde_json::to_value(weight).unwrap();

        assert_eq!(
            json,
            serde_json::json!({"quantity": "0.250", "unit": "KILOGRAM"})
        );
        assert_eq!(serde_json::from_value::<Weight>(json).unwrap(), weight);
        let refused =
            serde_json::from_value::<Weight>(serde_json::json!({"quantity": "1", "unit": "METRE"}))
                .unwrap_err();
        assert_eq!(
            refused.to_string(),
            "must be one of MILLIGRAM, GRAM, KILOGRAM, OUNCE, POUND"
        );
    }
}
