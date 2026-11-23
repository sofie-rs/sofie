//! Secrets and signatures as Standard Webhooks spells them
//! (<https://github.com/standard-webhooks/standard-webhooks/blob/main/spec/standard-webhooks.md>),
//! so receivers verify with any of its libraries.

use ring::hmac;
use sofie::core::encoding::base64::{self, STANDARD};
use sofie::core::{Error, Result};

const PREFIX: &str = "whsec_";
const SECRET_BYTES: usize = 24;
/// Sealing is bound to its purpose, so a sealed secret opens nowhere else.
pub(crate) const PURPOSE: &str = "sofie_webhooks.secret";

/// Shown once, when it is made; stored only sealed.
pub(crate) fn generate() -> Result<String> {
    let mut bytes = [0; SECRET_BYTES];
    sofie::core::random::fill(&mut bytes)
        .map_err(|error| Error::internal("generating a webhook secret", error))?;
    Ok(format!(
        "{PREFIX}{}",
        base64::encode_padded(&STANDARD, &bytes)
    ))
}

/// `v1,<signature>` over `id.timestamp.body`; a receiver accepts any one listed.
pub(crate) fn signature(secret: &str, id: &str, timestamp: i64, body: &str) -> Result<String> {
    let key = secret
        .strip_prefix(PREFIX)
        .and_then(|encoded| base64::decode(&STANDARD, encoded))
        .ok_or_else(|| Error::internal_message("a stored webhook secret is malformed"))?;
    let key = hmac::Key::new(hmac::HMAC_SHA256, &key);
    let signed = hmac::sign(&key, format!("{id}.{timestamp}.{body}").as_bytes());
    Ok(format!(
        "v1,{}",
        base64::encode_padded(&STANDARD, signed.as_ref())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The example the specification's reference libraries test with.
    #[test]
    fn signs_the_standard_webhooks_example() {
        let secret = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw";
        let body = r#"{"test": 2432232314}"#;

        let signed = signature(secret, "msg_p5jXN8AQM9LWM0D4loKWxJek", 1_614_265_330, body);

        assert_eq!(
            signed.unwrap(),
            "v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE="
        );
    }

    #[test]
    fn a_secret_is_prefixed_and_carries_its_bytes() {
        let secret = generate().unwrap();

        let encoded = secret.strip_prefix(PREFIX).unwrap();
        assert_eq!(
            base64::decode(&STANDARD, encoded).unwrap().len(),
            SECRET_BYTES
        );
        assert_ne!(secret, generate().unwrap());
    }
}
