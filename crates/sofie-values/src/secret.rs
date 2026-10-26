use std::fmt;

use serde::{Deserialize, Deserializer};
use sofie_core::{Cast, ColumnReader, ColumnSpec, Result, SqlValue};

/// No `Display` and no `Serialize`, so no reply, log field or event payload can carry it.
#[derive(Clone)]
pub struct Secret<T> {
    value: T,
}

impl<T> Secret<T> {
    pub fn new(value: T) -> Self {
        Self { value }
    }

    pub fn expose(&self) -> &T {
        &self.value
    }
}

/// Constant time for equal lengths, so timing does not tell how much of a guess was right.
impl<T: AsRef<[u8]>> PartialEq for Secret<T> {
    fn eq(&self, other: &Self) -> bool {
        let (own, other) = (self.value.as_ref(), other.value.as_ref());
        let difference = own
            .iter()
            .zip(other)
            .fold(0, |difference, (own, other)| difference | (own ^ other));
        own.len() == other.len() && std::hint::black_box(difference) == 0
    }
}

impl<T: AsRef<[u8]>> Eq for Secret<T> {}

impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        sofie_core::value::write_masked(formatter, "Secret")
    }
}

impl<T: Cast> Cast for Secret<T> {
    const COLUMNS: &'static [ColumnSpec] = T::COLUMNS;
    const NULLABLE: bool = T::NULLABLE;
    const SENSITIVE: bool = true;

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.value.encode(out);
    }

    fn decode(row: &mut dyn ColumnReader) -> Result<Self> {
        T::decode(row).map(Self::new)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Secret<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        T::deserialize(deserializer).map(Self::new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::round_trip;

    #[test]
    fn never_prints_its_value() {
        let key = Secret::new("sk_live_123".to_owned());

        assert_eq!(format!("{key:?}"), "Secret(***)");
        assert_eq!(format!("{:?}", Some(&key)), "Some(Secret(***))");
        assert_eq!(key.expose(), "sk_live_123");
    }

    #[test]
    fn equals_only_the_same_bytes() {
        let key = Secret::new("sk_live_123".to_owned());

        assert_eq!(key, Secret::new("sk_live_123".to_owned()));
        assert_ne!(key, Secret::new("sk_live_124".to_owned()));
        assert_ne!(key, Secret::new("sk_live_12".to_owned()));
    }

    #[test]
    fn is_read_from_input_and_stored_like_its_value() {
        let key: Secret<String> = serde_json::from_value(serde_json::json!("sk_123")).unwrap();
        assert_eq!(round_trip(&key), vec![SqlValue::Text("sk_123".into())]);
    }
}
