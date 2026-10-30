use sofie_core::Instant;

use crate::{Currency, Email, Money};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operator {
    Equals,
    NotEquals,
    GreaterThan,
    LessThan,
    AtLeast,
    AtMost,
    Between,
    Contains,
    IsEmpty,
}

impl Operator {
    /// As a list's query names it: `total.gte=100`; equality is the bare field.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        Some(match name {
            "ne" => Self::NotEquals,
            "gt" => Self::GreaterThan,
            "lt" => Self::LessThan,
            "gte" => Self::AtLeast,
            "lte" => Self::AtMost,
            "between" => Self::Between,
            "contains" => Self::Contains,
            "empty" => Self::IsEmpty,
            _ => return None,
        })
    }
}

pub trait Filter {
    const OPERATORS: &'static [Operator];
}

pub const ORDERED: &[Operator] = &[
    Operator::Equals,
    Operator::GreaterThan,
    Operator::LessThan,
    Operator::AtLeast,
    Operator::AtMost,
    Operator::Between,
];
pub const TEXT: &[Operator] = &[Operator::Equals, Operator::NotEquals, Operator::Contains];
/// Codes, flags and sensitive values, whose parts must not be probed.
pub const EXACT: &[Operator] = &[Operator::Equals, Operator::NotEquals];

macro_rules! filters {
    ($operators:expr => $($value:ty),+) => {$(
        impl Filter for $value {
            const OPERATORS: &'static [Operator] = $operators;
        }
    )+};
}

filters!(ORDERED => i16, i32, i64, u16, u32, Money, Instant);
filters!(TEXT => String);
filters!(EXACT => bool, Currency, Email);

impl<V: crate::SurfacesFromInner<Inner: Filter>> Filter for V {
    const OPERATORS: &'static [Operator] = if <V as crate::SurfacesFromInner>::SENSITIVE {
        EXACT
    } else {
        V::Inner::OPERATORS
    };
}

impl<T: Filter> Filter for Option<T> {
    const OPERATORS: &'static [Operator] = T::OPERATORS;
}

#[cfg(feature = "time")]
filters!(ORDERED => crate::Date, crate::TimeOfDay);
// A wall clock or a quantity compares only within one zone or unit, which each row names.
#[cfg(feature = "time")]
filters!(EXACT => crate::LocalDateTime, crate::TimeZone, crate::Weekday);
#[cfg(feature = "phone")]
filters!(EXACT => crate::PhoneNumber);
#[cfg(feature = "person")]
filters!(EXACT => crate::PersonName);
#[cfg(feature = "geo")]
filters!(EXACT => crate::Coordinate);
#[cfg(feature = "percentage")]
filters!(ORDERED => crate::Percentage);
#[cfg(feature = "web")]
filters!(EXACT => crate::Slug, crate::Url, crate::Locale);

#[cfg(feature = "measure")]
impl<U: crate::Unit> Filter for crate::Measurement<U> {
    const OPERATORS: &'static [Operator] = EXACT;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_offers_the_operators_of_its_kind() {
        assert_eq!(Money::OPERATORS, ORDERED);
        assert_eq!(<Option<Money>>::OPERATORS, ORDERED);
        assert_eq!(String::OPERATORS, TEXT);
        assert!(!Email::OPERATORS.contains(&Operator::Contains));
    }

    #[cfg(feature = "time")]
    #[test]
    fn a_wall_clock_in_its_own_zone_filters_only_exactly() {
        assert_eq!(crate::LocalDateTime::OPERATORS, EXACT);
    }
}
