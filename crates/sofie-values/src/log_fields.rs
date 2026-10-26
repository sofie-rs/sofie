//! A sensitive value hands over only its mask, so it cannot reach a log unmasked.

use std::borrow::Cow;

use sofie_core::Instant;

use crate::{Currency, Email, Money, Secret};

#[derive(Clone, Debug, PartialEq)]
pub enum LogValue<'a> {
    Integer(i64),
    Float(f64),
    Bool(bool),
    Text(Cow<'a, str>),
}

pub trait LogFields {
    /// `None` as the member name logs the value whole.
    fn log_fields(&self, field: &mut dyn FnMut(Option<&'static str>, LogValue<'_>));
}

/// Written by `#[derive(Aggregate)]` for `#[sensitive]` fields, which log only `***`.
pub trait SensitiveFields {
    const SENSITIVE_FIELDS: &'static [&'static str];
}

impl<T: SensitiveFields + ?Sized> SensitiveFields for &T {
    const SENSITIVE_FIELDS: &'static [&'static str] = T::SENSITIVE_FIELDS;
}

impl<T: SensitiveFields + ?Sized> SensitiveFields for &mut T {
    const SENSITIVE_FIELDS: &'static [&'static str] = T::SENSITIVE_FIELDS;
}

macro_rules! log_whole {
    ($($value:ty),+ => |$self_:ident| $logged:expr) => {$(
        impl LogFields for $value {
            fn log_fields(&self, field: &mut dyn FnMut(Option<&'static str>, LogValue<'_>)) {
                let $self_ = self;
                field(None, $logged);
            }
        }
    )+};
}

log_whole!(i64 => |number| LogValue::Integer(*number));
log_whole!(i16, i32, u16, u32 => |number| LogValue::Integer(i64::from(*number)));
log_whole!(bool => |flag| LogValue::Bool(*flag));
log_whole!(String => |text| LogValue::Text(Cow::Borrowed(text)));
log_whole!(Currency => |code| LogValue::Text(Cow::Borrowed(code.as_str())));
log_whole!(Instant => |instant| LogValue::Text(Cow::Owned(instant.to_string())));
log_whole!(Email => |email| masked(email));

fn masked(value: &impl std::fmt::Debug) -> LogValue<'static> {
    LogValue::Text(Cow::Owned(format!("{value:?}")))
}

impl<T> LogFields for Secret<T> {
    fn log_fields(&self, field: &mut dyn FnMut(Option<&'static str>, LogValue<'_>)) {
        field(None, masked(self));
    }
}

impl<V: crate::SurfacesFromInner<Inner: LogFields> + std::fmt::Debug> LogFields for V {
    fn log_fields(&self, field: &mut dyn FnMut(Option<&'static str>, LogValue<'_>)) {
        if <V as crate::SurfacesFromInner>::SENSITIVE {
            field(None, masked(self));
        } else {
            self.inner().log_fields(field);
        }
    }
}

impl<T: LogFields> LogFields for Option<T> {
    fn log_fields(&self, field: &mut dyn FnMut(Option<&'static str>, LogValue<'_>)) {
        if let Some(value) = self {
            value.log_fields(field);
        }
    }
}

impl LogFields for Money {
    fn log_fields(&self, field: &mut dyn FnMut(Option<&'static str>, LogValue<'_>)) {
        field(Some("minor"), LogValue::Integer(self.minor()));
        field(
            Some("currency"),
            LogValue::Text(Cow::Borrowed(self.currency().as_str())),
        );
    }
}

#[cfg(feature = "time")]
log_whole!(crate::Date => |day| LogValue::Text(Cow::Owned(day.to_string())));
#[cfg(feature = "time")]
log_whole!(crate::TimeZone => |zone| LogValue::Text(Cow::Borrowed(zone.name())));
#[cfg(feature = "time")]
log_whole!(crate::TimeOfDay => |time| LogValue::Text(Cow::Owned(time.to_string())));
#[cfg(feature = "time")]
log_whole!(crate::Weekday => |weekday| LogValue::Text(Cow::Borrowed(sofie_core::Variants::name(*weekday))));
#[cfg(feature = "phone")]
log_whole!(crate::PhoneNumber => |phone| masked(phone));
#[cfg(feature = "person")]
log_whole!(crate::PersonName => |name| masked(name));
#[cfg(feature = "percentage")]
log_whole!(crate::Percentage => |share| LogValue::Integer(share.basis_points()));
#[cfg(feature = "web")]
log_whole!(crate::Slug, crate::Locale, crate::Url => |text| LogValue::Text(Cow::Borrowed(text.as_str())));

#[cfg(feature = "time")]
impl LogFields for crate::LocalDateTime {
    fn log_fields(&self, field: &mut dyn FnMut(Option<&'static str>, LogValue<'_>)) {
        field(
            Some("local"),
            LogValue::Text(Cow::Owned(self.local().to_string())),
        );
        field(
            Some("zone"),
            LogValue::Text(Cow::Borrowed(self.zone().name())),
        );
    }
}

/// Two decimals, about a kilometre: a full-precision position is personal, an area is not.
#[cfg(feature = "geo")]
impl LogFields for crate::Coordinate {
    fn log_fields(&self, field: &mut dyn FnMut(Option<&'static str>, LogValue<'_>)) {
        let rounded = |degrees: f64| LogValue::Float((degrees * 100.0).round() / 100.0);
        field(Some("latitude"), rounded(self.latitude()));
        field(Some("longitude"), rounded(self.longitude()));
    }
}

#[cfg(feature = "measure")]
impl<U: crate::Unit> LogFields for crate::Measurement<U> {
    fn log_fields(&self, field: &mut dyn FnMut(Option<&'static str>, LogValue<'_>)) {
        field(
            Some("quantity"),
            LogValue::Text(Cow::Owned(self.quantity())),
        );
        field(
            Some("unit"),
            LogValue::Text(Cow::Borrowed(self.unit().name())),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn logged(value: &impl LogFields) -> Vec<(Option<&'static str>, String)> {
        let mut fields = Vec::new();
        value.log_fields(&mut |member, value| fields.push((member, format!("{value:?}"))));
        fields
    }

    #[test]
    fn money_logs_its_minor_units_and_currency() {
        let total = Money::new(14990, Currency::parse("EUR").unwrap());
        assert_eq!(
            logged(&total),
            [
                (Some("minor"), "Integer(14990)".to_owned()),
                (Some("currency"), "Text(\"EUR\")".to_owned())
            ]
        );
    }

    #[cfg(feature = "geo")]
    #[test]
    fn a_coordinate_logs_rounded_to_about_a_kilometre() {
        let home = crate::Coordinate::try_new(41.008_237, -28.978_359).unwrap();
        assert_eq!(
            logged(&home),
            [
                (Some("latitude"), "Float(41.01)".to_owned()),
                (Some("longitude"), "Float(-28.98)".to_owned())
            ]
        );
    }

    struct Pin(String);

    impl std::fmt::Debug for Pin {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            sofie_core::value::write_masked(formatter, "Pin")
        }
    }

    impl sofie_core::Value for Pin {
        type Inner = String;

        fn inner(&self) -> &String {
            &self.0
        }

        fn try_from_inner(inner: String) -> Result<Self, impl std::fmt::Display> {
            Ok::<_, std::convert::Infallible>(Self(inner))
        }
    }

    impl crate::SurfacesFromInner for Pin {
        const SENSITIVE: bool = true;
    }

    #[test]
    fn a_sensitive_derived_value_logs_its_mask_and_filters_whole() {
        assert_eq!(
            logged(&Pin("1234".to_owned())),
            [(None, "Text(\"Pin(***)\")".to_owned())]
        );
        #[cfg(feature = "filter")]
        assert_eq!(<Pin as crate::Filter>::OPERATORS, crate::EXACT);
    }

    #[test]
    fn sensitive_values_log_only_their_mask() {
        let email = Email::parse("jane@example.com").unwrap();
        assert_eq!(
            logged(&email),
            [(None, "Text(\"j***@example.com\")".to_owned())]
        );
        let key = Secret::new("sk_live_1".to_owned());
        assert_eq!(logged(&key), [(None, "Text(\"Secret(***)\")".to_owned())]);
        assert_eq!(logged(&None::<Email>), []);
    }
}
