use std::fmt;

use sofie_core::Result;

use sofie_core::encoding::base64::{self, URL_SAFE};

const SECRET_BYTES: usize = 32;
const TEXT_LENGTH: usize = (SECRET_BYTES * 4).div_ceil(3);

/// Shown once when issued; only its digest is stored.
pub struct BearerToken(String);

impl BearerToken {
    pub fn generate() -> Result<Self> {
        let secret: [u8; SECRET_BYTES] = crate::random_bytes("a token")?;
        Ok(Self(base64::encode(&URL_SAFE, &secret)))
    }

    /// `None` for text no issued token has, which is then never looked up.
    #[must_use]
    pub fn presented(text: &str) -> Option<Self> {
        let well_formed = text.len() == TEXT_LENGTH
            && text
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
        well_formed.then(|| Self(text.to_owned()))
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Looked up by this, not compared: timing could only tell digests apart, never a token.
    #[must_use]
    pub fn digest(&self) -> String {
        crate::digest(&[self.0.as_bytes()])
    }
}

impl fmt::Debug for BearerToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BearerToken(…)")
    }
}

#[cfg(test)]
mod tests {
    use super::BearerToken;

    #[test]
    fn a_token_is_random_presentable_and_never_printed() {
        let token = BearerToken::generate().unwrap();
        let again = BearerToken::presented(token.expose()).unwrap();

        assert_eq!(token.digest(), again.digest());
        assert_ne!(token.expose(), BearerToken::generate().unwrap().expose());
        assert_eq!(format!("{token:?}"), "BearerToken(…)");
        assert!(BearerToken::presented("short").is_none());
        assert!(BearerToken::presented(&format!("{}=", &token.expose()[1..])).is_none());
    }
}
