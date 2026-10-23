//! A field's type alone decides its control, text and CSV cell, so no screen switches on types.

use sofie_core::{Constraint, Instant};

use crate::{Currency, Email, Money, Secret};

pub trait Present {
    const INPUT: Input;
    const SEARCH: Search;

    fn csv(&self) -> String;

    #[cfg(feature = "web")]
    fn write_localized(&self, out: &mut String, _conventions: &dyn Conventions) {
        out.push_str(&self.csv());
    }
}

/// How full-text search treats a value: words of `Text` are matched, an `Exact`
/// value only whole, `Number` and `Time` by range, and a `Skipped` one never indexed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Search {
    Text,
    Exact,
    Number,
    Time,
    Skipped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Input,
    /// Its value is never sent back to the page.
    Password,
    Textarea,
    Checkbox,
    Select,
    Money,
    /// Basis points shown and typed as a percent with up to two decimals.
    Percentage,
    /// One control per part, posted as `<field>.<part>`, for a value read as an object.
    Parts(&'static [Part]),
    /// Its form is sent as `multipart/form-data`; nothing is ever shown in it.
    File,
}

/// Basis points as the percent a `Percentage` control holds, `12.50` for 1250.
#[must_use]
pub fn percent_text(basis_points: i64) -> String {
    crate::decimal::scaled_text(basis_points, 2)
}

/// A part with options is a select.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Part {
    pub name: &'static str,
    pub attributes: &'static [(&'static str, &'static str)],
    pub options: &'static [&'static str],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Input {
    pub control: Control,
    pub attributes: &'static [(&'static str, &'static str)],
    pub options: &'static [&'static str],
}

impl Input {
    #[must_use]
    pub const fn new(control: Control) -> Self {
        Self {
            control,
            attributes: &[],
            options: &[],
        }
    }

    #[must_use]
    pub const fn typed(attributes: &'static [(&'static str, &'static str)]) -> Self {
        Self {
            attributes,
            ..Self::new(Control::Input)
        }
    }

    #[must_use]
    pub const fn select(options: &'static [&'static str]) -> Self {
        Self {
            options,
            ..Self::new(Control::Select)
        }
    }
}

#[cfg(feature = "web")]
pub trait Conventions {
    fn write_number(&self, out: &mut String, plain: &str);
    fn write_money(&self, out: &mut String, amount: Money);
    fn write_date(&self, out: &mut String, iso: &str);
    /// The variant's label in the `enumeration` the translations register.
    fn write_variant(&self, out: &mut String, enumeration: &str, variant: &str);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormField {
    pub name: &'static str,
    pub input: Input,
    pub required: bool,
    pub constraints: &'static [Constraint],
}

pub trait FormFields {
    const FIELDS: &'static [FormField];
}

pub(crate) const TEXT: Input = Input::typed(&[("type", "text")]);
pub(crate) const INTEGER: Input = Input::typed(&[("type", "number"), ("step", "1")]);

impl Present for bool {
    const INPUT: Input = Input::new(Control::Checkbox);
    const SEARCH: Search = Search::Exact;

    fn csv(&self) -> String {
        self.to_string()
    }
}

macro_rules! present_integer {
    ($($integer:ty),+) => {$(
        impl Present for $integer {
            const INPUT: Input = INTEGER;
            const SEARCH: Search = Search::Number;

            fn csv(&self) -> String {
                self.to_string()
            }

            #[cfg(feature = "web")]
            fn write_localized(&self, out: &mut String, conventions: &dyn Conventions) {
                conventions.write_number(out, &self.to_string());
            }
        }
    )+};
}

present_integer!(i16, i32, i64, u16, u32);

impl<T: Present> Present for Option<T> {
    const INPUT: Input = T::INPUT;
    const SEARCH: Search = T::SEARCH;

    fn csv(&self) -> String {
        self.as_ref().map(Present::csv).unwrap_or_default()
    }

    #[cfg(feature = "web")]
    fn write_localized(&self, out: &mut String, conventions: &dyn Conventions) {
        if let Some(value) = self {
            value.write_localized(out, conventions);
        }
    }
}

impl Present for Money {
    const INPUT: Input = Input::new(Control::Money);
    const SEARCH: Search = Search::Number;

    fn csv(&self) -> String {
        format!("{} {}", self.format_major(), self.currency())
    }

    #[cfg(feature = "web")]
    fn write_localized(&self, out: &mut String, conventions: &dyn Conventions) {
        conventions.write_money(out, *self);
    }
}

impl<T> Present for Secret<T> {
    const INPUT: Input = Input::new(Control::Password);
    const SEARCH: Search = Search::Skipped;

    fn csv(&self) -> String {
        "***".to_owned()
    }
}

/// The control shows and reads the app's display zone (`SOFIE_TIME_ZONE`); a time a user types
/// on their own wall clock belongs in `LocalDateTime`.
impl Present for Instant {
    const INPUT: Input = Input::typed(&[("type", "datetime-local")]);
    const SEARCH: Search = Search::Time;

    fn csv(&self) -> String {
        self.to_string()
    }

    #[cfg(feature = "web")]
    fn write_localized(&self, out: &mut String, conventions: &dyn Conventions) {
        conventions.write_date(out, &self.to_string());
    }
}

/// A marker the derive implements; a value overriding a surface implements that trait instead.
pub trait SurfacesFromInner: sofie_core::Value<Inner: Present> {
    const SENSITIVE: bool = false;
}

impl<V: SurfacesFromInner> Present for V {
    const INPUT: Input = if <V as crate::SurfacesFromInner>::SENSITIVE {
        Input::new(Control::Password)
    } else {
        V::Inner::INPUT
    };
    const SEARCH: Search = if <V as crate::SurfacesFromInner>::SENSITIVE {
        Search::Skipped
    } else {
        V::Inner::SEARCH
    };

    fn csv(&self) -> String {
        self.inner().csv()
    }

    #[cfg(feature = "web")]
    fn write_localized(&self, out: &mut String, conventions: &dyn Conventions) {
        self.inner().write_localized(out, conventions);
    }
}

macro_rules! present_text {
    ($value:ty, $input:expr, $search:ident) => {
        $crate::present::present_text!($value, $input, $search, |value| value.as_str().to_owned());
    };
    ($value:ty, $input:expr, $search:ident, |$self_:ident| $text:expr) => {
        impl $crate::Present for $value {
            const INPUT: $crate::Input = $input;
            const SEARCH: $crate::Search = $crate::Search::$search;

            fn csv(&self) -> String {
                let $self_ = self;
                $text
            }
        }
    };
}

pub(crate) use present_text;

present_text!(
    Currency,
    Input::typed(&[("type", "text"), ("maxlength", "3")]),
    Exact
);
present_text!(
    Email,
    Input::typed(&[("type", "email"), ("autocomplete", "email")]),
    Exact
);
present_text!(String, TEXT, Text);
// Hidden: an id is carried by a form, never typed.
present_text!(
    sofie_core::Uuid,
    Input::typed(&[("type", "hidden")]),
    Exact,
    |id| id.to_string()
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::euros;

    #[cfg(feature = "web")]
    struct Marked;

    #[cfg(feature = "web")]
    impl Conventions for Marked {
        fn write_number(&self, out: &mut String, plain: &str) {
            out.push_str("number:");
            out.push_str(plain);
        }

        fn write_money(&self, out: &mut String, amount: Money) {
            out.push_str("money:");
            out.push_str(&amount.csv());
        }

        fn write_date(&self, out: &mut String, iso: &str) {
            out.push_str("date:");
            out.push_str(iso);
        }

        fn write_variant(&self, out: &mut String, enumeration: &str, variant: &str) {
            out.push_str(enumeration);
            out.push(':');
            out.push_str(variant);
        }
    }

    #[cfg(feature = "web")]
    fn localized(value: &impl Present) -> String {
        let mut out = String::new();
        value.write_localized(&mut out, &Marked);
        out
    }

    #[test]
    fn money_is_typed_shown_and_exported_as_an_amount() {
        assert_eq!(Money::INPUT.control, Control::Money);
        assert_eq!(euros(14990).csv(), "149.90 EUR");
        #[cfg(feature = "web")]
        assert_eq!(localized(&euros(14990)), "money:149.90 EUR");
    }

    #[test]
    fn an_option_is_presented_like_its_value() {
        assert_eq!(<Option<Money>>::INPUT, Money::INPUT);
        assert_eq!(None::<i64>.csv(), "");
        #[cfg(feature = "web")]
        assert_eq!(localized(&Some(7_i64)), "number:7");
    }

    #[test]
    fn a_secret_never_shows_its_value() {
        let secret = Secret::new("hunter22".to_owned());
        assert_eq!(<Secret<String>>::INPUT.control, Control::Password);
        assert_eq!(secret.csv(), "***");
        #[cfg(feature = "web")]
        assert_eq!(localized(&secret), "***");
    }

    #[test]
    fn text_values_are_typed_by_their_attributes() {
        assert_eq!(
            Email::INPUT.attributes,
            &[("type", "email"), ("autocomplete", "email")]
        );
        assert_eq!(bool::INPUT.control, Control::Checkbox);
        let email = Email::parse("Jane@Example.com").unwrap();
        assert_eq!(email.csv(), "jane@example.com");
    }

    #[derive(Debug)]
    struct Quantity(i64);

    impl sofie_core::Value for Quantity {
        type Inner = i64;

        fn inner(&self) -> &i64 {
            &self.0
        }

        fn try_from_inner(inner: i64) -> Result<Self, impl std::fmt::Display> {
            Ok::<_, std::convert::Infallible>(Self(inner))
        }
    }

    impl SurfacesFromInner for Quantity {}

    #[test]
    fn search_reads_words_of_text_and_never_a_secret() {
        assert_eq!(String::SEARCH, Search::Text);
        assert_eq!(Email::SEARCH, Search::Exact);
        assert_eq!(<Option<Money>>::SEARCH, Search::Number);
        assert_eq!(Instant::SEARCH, Search::Time);
        assert_eq!(<Secret<String>>::SEARCH, Search::Skipped);
        assert_eq!(Quantity::SEARCH, Search::Number);
    }

    #[test]
    fn a_derived_value_is_presented_as_the_value_it_wraps() {
        assert_eq!(Quantity::INPUT, i64::INPUT);
        #[cfg(feature = "web")]
        assert_eq!(localized(&Quantity(3)), "number:3");
    }

    #[cfg(all(feature = "web", feature = "time"))]
    #[test]
    fn a_time_formats_as_a_date_and_a_weekday_as_its_enum_s_label() {
        assert_eq!(
            localized(&crate::TimeOfDay::constant("09:30")),
            "date:09:30"
        );
        assert_eq!(localized(&crate::Weekday::Monday), "weekday:MONDAY");
    }

    #[cfg(feature = "web")]
    #[test]
    fn instants_format_as_dates() {
        let instant = Instant::parse("2026-10-11T09:00:00+03:00").unwrap();
        assert_eq!(localized(&instant), "date:2026-10-11T06:00:00Z");
    }
}
