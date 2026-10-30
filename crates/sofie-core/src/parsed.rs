use std::fmt;

use serde::de::{self, Visitor};

/// Through the parser, so a request cannot build the value without the check; a refusal is
/// written in its alternate form, as the field being read already names the value.
pub fn deserialize_parsed<'de, V, M: fmt::Display, D: serde::Deserializer<'de>>(
    deserializer: D,
    expected: &'static str,
    parse: fn(&str) -> Result<V, M>,
) -> Result<V, D::Error> {
    deserializer.deserialize_str(Parsed { expected, parse })
}

struct Parsed<V, M> {
    expected: &'static str,
    parse: fn(&str) -> Result<V, M>,
}

impl<V, M: fmt::Display> Visitor<'_> for Parsed<V, M> {
    type Value = V;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.expected)
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<V, E> {
        (self.parse)(text).map_err(|refusal| E::custom(format_args!("{refusal:#}")))
    }
}
