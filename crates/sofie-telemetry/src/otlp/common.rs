//! What every OTLP request shares: attribute values, the resource and the scope.
//!
//! Field numbers here and in the other `otlp` encoders are opentelemetry-proto v1.10.0's, as
//! generated in the `opentelemetry-proto` 0.33.1 crate.

use std::borrow::Cow;
use std::time::{SystemTime, UNIX_EPOCH};

use super::protobuf::Encoder;
use crate::fields::FieldValue;

mod any_value {
    pub(super) const STRING: u32 = 1;
    pub(super) const BOOL: u32 = 2;
    pub(super) const INT: u32 = 3;
    pub(super) const DOUBLE: u32 = 4;
}

mod key_value {
    pub(super) const KEY: u32 = 1;
    pub(super) const VALUE: u32 = 2;
}

mod scope {
    pub(super) const NAME: u32 = 1;
}

mod resource {
    pub(super) const ATTRIBUTES: u32 = 1;
}

pub(crate) const RESOURCE: u32 = 1;
pub(crate) const SCOPE: u32 = 1;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Value {
    Text(Cow<'static, str>),
    Int(i64),
    Double(f64),
    Bool(bool),
}

/// OpenTelemetry has no unsigned integers; one too large for `i64` goes as text.
impl From<FieldValue> for Value {
    fn from(value: FieldValue) -> Self {
        match value {
            FieldValue::Text(text) | FieldValue::Formatted(text) | FieldValue::Composed(text) => {
                Self::Text(Cow::Owned(text))
            }
            FieldValue::Signed(number) => Self::Int(number),
            FieldValue::Unsigned(number) => i64::try_from(number)
                .map_or_else(|_| Self::Text(Cow::Owned(number.to_string())), Self::Int),
            FieldValue::Float(number) => Self::Double(number),
            FieldValue::Bool(value) => Self::Bool(value),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Attribute {
    pub(crate) key: &'static str,
    pub(crate) value: Value,
}

impl Attribute {
    pub(crate) fn new(key: &'static str, value: impl Into<Value>) -> Self {
        Self {
            key,
            value: value.into(),
        }
    }
}

impl From<&'static str> for Value {
    fn from(text: &'static str) -> Self {
        Self::Text(Cow::Borrowed(text))
    }
}

impl From<i64> for Value {
    fn from(number: i64) -> Self {
        Self::Int(number)
    }
}

pub(crate) fn any_value(encoder: &mut Encoder, field: u32, value: &Value) {
    encoder.message(field, |encoder| match value {
        Value::Text(text) => encoder.string(any_value::STRING, text),
        Value::Bool(value) => encoder.bool(any_value::BOOL, *value),
        Value::Int(number) => encoder.int64(any_value::INT, *number),
        Value::Double(number) => encoder.double(any_value::DOUBLE, *number),
    });
}

pub(crate) fn attributes(encoder: &mut Encoder, field: u32, attributes: &[Attribute]) {
    for attribute in attributes {
        key_value(encoder, field, attribute.key, &attribute.value);
    }
}

pub(crate) fn key_value(encoder: &mut Encoder, field: u32, key: &str, value: &Value) {
    encoder.message(field, |encoder| {
        encoder.string(key_value::KEY, key);
        any_value(encoder, key_value::VALUE, value);
    });
}

pub(crate) fn scope(encoder: &mut Encoder, name: &str) {
    encoder.message(SCOPE, |encoder| encoder.string(scope::NAME, name));
}

/// A `Resource` message's body, encoded once and copied into every request.
pub(crate) fn resource_body(attributes: &[(String, String)]) -> Vec<u8> {
    let mut encoder = Encoder::default();
    for (key, value) in attributes {
        let value = Value::Text(Cow::Owned(value.clone()));
        key_value(&mut encoder, resource::ATTRIBUTES, key, &value);
    }
    encoder.into_bytes()
}

pub(crate) fn resource(encoder: &mut Encoder, body: &[u8]) {
    encoder.bytes(RESOURCE, body);
}

/// A clock set before 1970 reads as the epoch.
pub(crate) fn unix_nanos(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |since| {
        u64::try_from(since.as_nanos()).unwrap_or(u64::MAX)
    })
}
