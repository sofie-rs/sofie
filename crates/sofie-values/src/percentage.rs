use std::convert::Infallible;
use std::fmt;

use serde::de::{self, Unexpected, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sofie_core::value::{self, Value};
use sofie_core::{Ordered, Result};

#[cfg(feature = "web")]
use crate::Conventions;
use crate::{Control, Input, Money, Present};

const BASIS_POINTS_PER_WHOLE: i64 = 10_000;

/// Basis points, so it is exact; may be negative or above 100 %. A JSON number is basis points,
/// text is a percent as a person types it: `"19.5"` is 1950.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Percentage {
    basis_points: i64,
}

impl Percentage {
    #[must_use]
    pub fn from_basis_points(basis_points: i64) -> Self {
        Self { basis_points }
    }

    #[must_use]
    pub fn basis_points(self) -> i64 {
        self.basis_points
    }

    /// Rounded half away from zero to the minor unit.
    pub fn of(self, amount: Money) -> Result<Money> {
        amount.scale(self.basis_points, BASIS_POINTS_PER_WHOLE)
    }
}

impl Value for Percentage {
    type Inner = i64;

    fn inner(&self) -> &i64 {
        &self.basis_points
    }

    fn try_from_inner(basis_points: i64) -> Result<Self, impl std::fmt::Display> {
        Ok::<_, Infallible>(Self::from_basis_points(basis_points))
    }
}

impl Ordered for Percentage {}

impl Serialize for Percentage {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        value::serialize(self, serializer)
    }
}

impl<'de> Deserialize<'de> for Percentage {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(PercentageVisitor)
    }
}

struct PercentageVisitor;

impl Visitor<'_> for PercentageVisitor {
    type Value = Percentage;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("whole basis points, or a percent with up to two decimals as text")
    }

    fn visit_i64<E: de::Error>(self, basis_points: i64) -> Result<Percentage, E> {
        Ok(Percentage::from_basis_points(basis_points))
    }

    fn visit_u64<E: de::Error>(self, basis_points: u64) -> Result<Percentage, E> {
        i64::try_from(basis_points)
            .map(Percentage::from_basis_points)
            .map_err(|_| E::invalid_value(Unexpected::Unsigned(basis_points), &self))
    }

    fn visit_str<E: de::Error>(self, percent: &str) -> Result<Percentage, E> {
        crate::decimal::parse_scaled(percent, 2)
            .map(Percentage::from_basis_points)
            .map_err(|refusal| E::custom(refusal.message()))
    }
}

impl Present for Percentage {
    const INPUT: Input = Input::new(Control::Percentage);
    const SEARCH: crate::Search = crate::Search::Number;

    fn csv(&self) -> String {
        crate::percent_text(self.basis_points)
    }

    #[cfg(feature = "web")]
    fn write_localized(&self, out: &mut String, conventions: &dyn Conventions) {
        conventions.write_number(out, &self.csv());
        out.push_str(" %");
    }
}

#[cfg(test)]
mod tests {
    use sofie_core::SqlValue;

    use super::*;
    use crate::testing::{euros, round_trip};

    #[test]
    fn takes_its_share_of_an_amount_rounded_half_away_from_zero() {
        let rate = Percentage::from_basis_points(1900);
        let share = Percentage::from_basis_points;

        assert_eq!(rate.of(euros(1005)).unwrap(), euros(191));
        assert_eq!(rate.of(euros(-1005)).unwrap(), euros(-191));
        assert_eq!(share(100).of(euros(50)).unwrap(), euros(1));
        assert_eq!(share(100).of(euros(49)).unwrap(), euros(0));
        assert_eq!(share(15_000).of(euros(1000)).unwrap(), euros(1500));
    }

    #[test]
    fn is_stored_and_serialized_as_basis_points() {
        let rate = Percentage::from_basis_points(1250);
        assert_eq!(round_trip(&rate), vec![SqlValue::Int(1250)]);
        assert_eq!(serde_json::to_value(rate).unwrap(), serde_json::json!(1250));
        assert_eq!(
            serde_json::from_value::<Percentage>(serde_json::json!(-50)).unwrap(),
            Percentage::from_basis_points(-50)
        );
    }

    #[test]
    fn refuses_a_fraction_of_a_basis_point() {
        assert!(serde_json::from_value::<Percentage>(serde_json::json!(12.5)).is_err());
        assert!(serde_json::from_value::<Percentage>(serde_json::json!("12.555")).is_err());
    }

    #[test]
    fn reads_text_as_a_typed_percent() {
        let typed = |text: &str| serde_json::from_value::<Percentage>(serde_json::json!(text));

        assert_eq!(typed("19").unwrap(), Percentage::from_basis_points(1900));
        assert_eq!(typed("-0.5").unwrap(), Percentage::from_basis_points(-50));
        assert!(typed("19%").is_err());
        assert_eq!(Percentage::INPUT.control, Control::Percentage);
    }

    #[test]
    fn orders_by_basis_points() {
        assert!(Percentage::from_basis_points(-1) < Percentage::from_basis_points(0));
        assert_eq!(Percentage::from_basis_points(7).basis_points(), 7);
    }
}
