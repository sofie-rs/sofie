//! Each value is parsed once at the boundary: deserializing input and decoding a row share it.

mod currency;
mod decimal;
mod email;
mod money;
mod parsed;
mod present;
mod secret;
mod slug_character;
#[cfg(test)]
mod testing;

#[cfg(feature = "geo")]
mod coordinate;
#[cfg(feature = "filter")]
mod filter;
#[cfg(feature = "fixtures")]
pub mod fixture;
#[cfg(feature = "log-fields")]
mod log_fields;
#[cfg(feature = "measure")]
mod measurement;
#[cfg(feature = "percentage")]
mod percentage;
#[cfg(feature = "person")]
mod person_name;
#[cfg(feature = "phone")]
mod phone_number;
#[cfg(feature = "time")]
mod time;
#[cfg(feature = "web")]
mod web;

pub use currency::Currency;
pub use email::Email;
pub use money::Money;
#[cfg(feature = "web")]
pub use present::Conventions;
pub use present::{
    Control, FormField, FormFields, Input, Part, Present, Search, SurfacesFromInner, percent_text,
};
pub use secret::Secret;
pub use slug_character::{LATIN_COMBINING_ACCENTS, is_slug_character};
pub use sofie_core::{Constraint, Instant};

#[cfg(feature = "geo")]
pub use coordinate::Coordinate;
#[cfg(feature = "filter")]
pub use filter::{EXACT, Filter, ORDERED, Operator, TEXT};
#[cfg(feature = "fixtures")]
pub use fixture::{Fixture, InputFixture, Samples, Seed};
#[cfg(feature = "log-fields")]
pub use log_fields::{LogFields, LogValue, SensitiveFields};
#[cfg(feature = "measure")]
pub use measurement::{
    Energy, EnergyUnit, Length, LengthUnit, Measurement, Unit, Weight, WeightUnit,
};
#[cfg(feature = "percentage")]
pub use percentage::Percentage;
#[cfg(feature = "person")]
pub use person_name::PersonName;
#[cfg(feature = "phone")]
pub use phone_number::PhoneNumber;
#[cfg(feature = "time")]
pub use time::{Date, LocalDateTime, TimeOfDay, TimeZone, Weekday};
#[cfg(feature = "web")]
pub use web::{Locale, Slug, Url};
