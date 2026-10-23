use std::fmt;

use serde::de::{self, Visitor};

/// Through the parser, so a request cannot build the value without the check; the text is read in
/// place, borrowed or not, so no string is allocated.
pub(crate) fn deserialize_parsed<'de, V, D: serde::Deserializer<'de>>(
    deserializer: D,
    expected: &'static str,
    parse: fn(&str) -> sofie_core::Result<V>,
) -> Result<V, D::Error> {
    deserializer.deserialize_str(Parsed { expected, parse })
}

struct Parsed<V> {
    expected: &'static str,
    parse: fn(&str) -> sofie_core::Result<V>,
}

impl<V> Visitor<'_> for Parsed<V> {
    type Value = V;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.expected)
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<V, E> {
        (self.parse)(text).map_err(sofie_core::Error::into_deserialize_error)
    }
}
