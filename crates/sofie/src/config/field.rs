use std::net::SocketAddr;

use sofie_values::{Currency, Email, Secret};

use super::environment::{ConfigError, Entry};

/// What `#[derive(sofie::Config)]` implements: a struct read from the keys under one prefix.
pub trait Group: Sized {
    const FIELDS: &'static [FieldSpec];
    /// The group of the app's own keys read for each key of this group left unset.
    const FALLBACK: Option<&'static str> = None;

    fn build(entry: &Entry<'_>) -> Result<Self, ConfigError>;
}

#[derive(Clone, Copy, Debug)]
pub struct FieldSpec {
    pub name: &'static str,
    pub key: &'static str,
    pub optional: bool,
    pub kind: Kind,
    pub fallback: Option<&'static str>,
}

#[derive(Clone, Copy, Debug)]
pub enum Kind {
    Switch,
    Number,
    Text,
    Address,
    Value,
    Group(&'static [FieldSpec]),
}

pub trait Field: Sized {
    const KIND: Kind;
    const FALLBACK: Option<&'static str> = None;

    fn read(entry: &Entry<'_>) -> Result<Self, ConfigError>;
}

impl<G: Group> Field for G {
    const KIND: Kind = Kind::Group(G::FIELDS);
    const FALLBACK: Option<&'static str> = G::FALLBACK;

    fn read(entry: &Entry<'_>) -> Result<Self, ConfigError> {
        G::build(entry)
    }
}

impl<T: Field> Field for Option<T> {
    const KIND: Kind = T::KIND;

    fn read(entry: &Entry<'_>) -> Result<Self, ConfigError> {
        entry.is_set(T::KIND).then(|| T::read(entry)).transpose()
    }
}

impl<T: Field> Field for Secret<T> {
    const KIND: Kind = T::KIND;

    fn read(entry: &Entry<'_>) -> Result<Self, ConfigError> {
        T::read(entry).map(Secret::new)
    }
}

macro_rules! parsed {
    ($($kind:ident, $expected:literal, $parse:expr => $($parsed:ty),+;)*) => {$($(
        impl Field for $parsed {
            const KIND: Kind = Kind::$kind;

            fn read(entry: &Entry<'_>) -> Result<Self, ConfigError> {
                entry.parse($expected, $parse)
            }
        }
    )+)*};
}

parsed! {
    Switch, "`on` or `off`", crate::settings::switched => bool;
    Number, "a whole number", str::parse::<Self> => i8, i16, i32, i64, isize;
    Number, "a whole number of 0 or more", str::parse::<Self> => u8, u16, u32, u64, usize;
    Number, "a number", str::parse::<Self> => f32, f64;
    Text, "text", str::parse::<Self> => String;
    Address, "an address such as 127.0.0.1:8080", str::parse::<Self> => SocketAddr;
    Value, "an email address", Self::parse => Email;
    Value, "a currency code", Self::parse => Currency;
}
