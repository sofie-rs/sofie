use std::cmp::Ordering::{Greater, Less};

use crate::{FieldError, Value};

/// A bound counts characters for text and compares numbers; the derive picks by [`Measured::TEXT`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Constraint {
    Required,
    MinLength(i64),
    MaxLength(i64),
    Min(i64),
    Max(i64),
    /// Only the database can check it.
    Unique,
}

pub trait Measured {
    const TEXT: bool;

    fn size(&self) -> Option<i64>;

    fn is_blank(&self) -> bool {
        false
    }
}

impl Measured for String {
    const TEXT: bool = true;

    fn size(&self) -> Option<i64> {
        Some(i64::try_from(self.chars().count()).unwrap_or(i64::MAX))
    }

    fn is_blank(&self) -> bool {
        self.trim().is_empty()
    }
}

macro_rules! measured_number {
    ($($number:ty),*) => {$(
        impl Measured for $number {
            const TEXT: bool = false;

            fn size(&self) -> Option<i64> {
                Some(i64::from(*self))
            }
        }
    )*};
}

measured_number!(i8, i16, i32, i64, u8, u16, u32);

impl<T: Measured> Measured for Option<T> {
    const TEXT: bool = T::TEXT;

    fn size(&self) -> Option<i64> {
        self.as_ref().and_then(Measured::size)
    }

    fn is_blank(&self) -> bool {
        self.as_ref().is_none_or(Measured::is_blank)
    }
}

impl<V: Value<Inner: Measured>> Measured for V {
    const TEXT: bool = V::Inner::TEXT;

    fn size(&self) -> Option<i64> {
        self.inner().size()
    }

    fn is_blank(&self) -> bool {
        self.inner().is_blank()
    }
}

pub struct FieldRules<T> {
    pub constraints: &'static [Constraint],
    refuse: fn(&[Constraint], &str, &T) -> Option<FieldError>,
}

impl<T> Clone for FieldRules<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for FieldRules<T> {}

impl<T> FieldRules<T> {
    pub const NONE: Self = Self {
        constraints: &[],
        refuse: |_, _, _| None,
    };

    #[must_use]
    pub fn refusal(self, field: &str, value: &T) -> Option<FieldError> {
        (self.refuse)(self.constraints, field, value)
    }
}

impl<T: Measured> FieldRules<T> {
    #[must_use]
    pub const fn new(constraints: &'static [Constraint]) -> Self {
        Self {
            constraints,
            refuse: |constraints, field, value| {
                constraints
                    .iter()
                    .find_map(|constraint| constraint.refusal(field, value))
            },
        }
    }
}

impl Constraint {
    #[must_use]
    pub fn refusal<T: Measured>(self, field: &str, value: &T) -> Option<FieldError> {
        let (key, name, unit, bound, refused) = match self {
            Self::Required => {
                return value.is_blank().then(|| FieldError::required(field));
            }
            Self::Unique => return None,
            Self::MinLength(min) => ("validation.min_length", "min", " characters", min, Less),
            Self::MaxLength(max) => ("validation.max_length", "max", " characters", max, Greater),
            Self::Min(min) => ("validation.min", "min", "", min, Less),
            Self::Max(max) => ("validation.max", "max", "", max, Greater),
        };
        if value.size()?.cmp(&bound) != refused {
            return None;
        }
        let comparison = if refused == Less {
            "at least"
        } else {
            "at most"
        };
        let message = format!("must be {comparison} {bound}{unit}");
        Some(FieldError::new(field, key, message).with(name, bound.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;

    fn refused<T: Measured>(
        constraint: Constraint,
        value: &T,
    ) -> Option<(Cow<'static, str>, String)> {
        constraint
            .refusal("name", value)
            .map(|error| (error.key, error.message))
    }

    #[test]
    fn bounds_count_characters_of_text_and_compare_numbers() {
        let long = "ğğğğ".to_owned();

        assert_eq!(refused(Constraint::MaxLength(4), &long), None);
        assert_eq!(
            refused(Constraint::MaxLength(3), &long),
            Some((
                "validation.max_length".into(),
                "must be at most 3 characters".to_owned()
            ))
        );
        assert_eq!(
            refused(Constraint::Min(1), &0_i32),
            Some(("validation.min".into(), "must be at least 1".to_owned()))
        );
        assert_eq!(refused(Constraint::Max(10), &10_u8), None);
    }

    #[test]
    fn a_refusal_carries_its_bound_as_an_argument() {
        let error = Constraint::Max(5).refusal("count", &6_i64).unwrap();

        assert_eq!(error.field, "count");
        assert_eq!(error.arguments, [(Cow::from("max"), "5".to_owned())]);
    }

    #[test]
    fn required_refuses_blank_text_and_absent_values_only() {
        assert!(refused(Constraint::Required, &"  ".to_owned()).is_some());
        assert!(refused(Constraint::Required, &None::<i64>).is_some());
        assert!(refused(Constraint::Required, &Some(0_i64)).is_none());
        assert!(refused(Constraint::Required, &0_i64).is_none());
    }

    #[test]
    fn an_absent_optional_value_keeps_every_bound() {
        assert!(refused(Constraint::MinLength(3), &None::<String>).is_none());
        assert!(refused(Constraint::Unique, &"taken".to_owned()).is_none());
    }
}
