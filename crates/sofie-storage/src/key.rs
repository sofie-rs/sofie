use std::fmt;

use sofie_core::{Error, Result};

const RANDOM_BYTES: usize = 16;
const LENGTH: usize = RANDOM_BYTES * 2;

/// 128 random bits as lowercase hex: never a name a caller chose, so never a path that leaves its directory.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct StorageKey {
    hex: String,
}

impl StorageKey {
    pub fn generate() -> Result<Self> {
        let mut random = [0; RANDOM_BYTES];
        sofie_core::random::fill(&mut random)
            .map_err(|error| Error::internal("generating a storage key", error))?;
        Ok(Self {
            hex: sofie_core::encoding::hex::encode(&random),
        })
    }

    pub fn parse(text: &str) -> Result<Self> {
        let is_key = text.len() == LENGTH
            && text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if !is_key {
            return Err(Error::invalid(
                "storage_key",
                "is not a storage key: 32 lowercase hexadecimal digits",
            ));
        }
        Ok(Self {
            hex: text.to_owned(),
        })
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.hex
    }

    #[cfg(feature = "variants")]
    pub(crate) fn from_hex(hex: String) -> Self {
        Self { hex }
    }
}

impl fmt::Display for StorageKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.hex)
    }
}

impl fmt::Debug for StorageKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "StorageKey({})", self.hex)
    }
}

sofie_core::text_value!(StorageKey, hex);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generated_key_parses_back_and_differs_from_the_next() {
        let key = StorageKey::generate().unwrap();

        assert_eq!(StorageKey::parse(key.as_str()).unwrap(), key);
        assert_ne!(StorageKey::generate().unwrap(), key);
    }

    #[test]
    fn only_32_lowercase_hex_digits_are_a_key() {
        for text in [
            "",
            "../../etc/passwd",
            "0123456789abcdef0123456789ABCDEF",
            "0123456789abcdef0123456789abcde",
            "0123456789abcdef0123456789abcdef0",
            "0123456789abcdef/123456789abcdef",
        ] {
            assert!(StorageKey::parse(text).is_err(), "{text:?}");
        }
        assert!(StorageKey::parse("0123456789abcdef0123456789abcdef").is_ok());
    }
}
