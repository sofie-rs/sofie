//! Webhook signatures as Standard Webhooks spells them
//! (<https://github.com/standard-webhooks/standard-webhooks/blob/main/spec/standard-webhooks.md>),
//! so a delivery signed here verifies with any of its libraries and theirs verify here.

use std::fmt;

use ring::hmac;
use sofie_core::encoding::base64::{self, STANDARD};
use sofie_core::{Error, Result, Timestamp};

pub const WEBHOOK_ID: &str = "webhook-id";
pub const WEBHOOK_TIMESTAMP: &str = "webhook-timestamp";
pub const WEBHOOK_SIGNATURE: &str = "webhook-signature";

const PREFIX: &str = "whsec_";
const SECRET_BYTES: usize = 24;
const VERSION: &str = "v1,";
/// Older or newer deliveries are refused, so a captured one cannot be replayed later.
const TOLERANCE_SECONDS: u64 = 5 * 60;

pub struct WebhookSecret(hmac::Key);

pub struct WebhookHeaders<'a> {
    pub id: &'a str,
    pub timestamp: &'a str,
    /// Space-separated `v1,<signature>`; more than one while the sender rotates its secret.
    pub signatures: &'a str,
}

impl WebhookSecret {
    pub fn generate() -> Result<String> {
        let bytes = crate::random_bytes::<SECRET_BYTES>("a webhook secret")?;
        Ok(format!(
            "{PREFIX}{}",
            base64::encode_padded(&STANDARD, &bytes)
        ))
    }

    pub fn parse(text: &str) -> Result<Self> {
        let encoded = text.strip_prefix(PREFIX).unwrap_or(text);
        let key = base64::decode(&STANDARD, encoded)
            .filter(|key| !key.is_empty())
            .ok_or_else(|| {
                Error::internal_message("a webhook secret is not `whsec_` and base64")
            })?;
        Ok(Self(hmac::Key::new(hmac::HMAC_SHA256, &key)))
    }

    #[must_use]
    pub fn sign(&self, id: &str, timestamp: i64, body: &[u8]) -> String {
        let signed = hmac::sign(&self.0, &message(id, &timestamp.to_string(), body));
        format!(
            "{VERSION}{}",
            base64::encode_padded(&STANDARD, signed.as_ref())
        )
    }

    pub fn verify(&self, headers: &WebhookHeaders<'_>, body: &[u8], now: Timestamp) -> Result<()> {
        let sent: i64 = headers
            .timestamp
            .parse()
            .map_err(|_| refused("its timestamp is not a number"))?;
        if now.as_second().abs_diff(sent) > TOLERANCE_SECONDS {
            return Err(refused("it was signed too long ago"));
        }
        let message = message(headers.id, headers.timestamp, body);
        let matches = headers
            .signatures
            .split(' ')
            .filter_map(|signature| signature.strip_prefix(VERSION))
            .filter_map(|encoded| base64::decode(&STANDARD, encoded))
            .any(|tag| hmac::verify(&self.0, &message, &tag).is_ok());
        if matches {
            return Ok(());
        }
        Err(refused("no signature matches"))
    }
}

impl fmt::Debug for WebhookSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("WebhookSecret(…)")
    }
}

fn message(id: &str, timestamp: &str, body: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(id.len() + timestamp.len() + 2 + body.len());
    message.extend_from_slice(id.as_bytes());
    message.push(b'.');
    message.extend_from_slice(timestamp.as_bytes());
    message.push(b'.');
    message.extend_from_slice(body);
    message
}

fn refused(why: &str) -> Error {
    Error::BadRequest(format!("the webhook's signature is refused: {why}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw";
    const ID: &str = "msg_p5jXN8AQM9LWM0D4loKWxJek";
    const SENT: i64 = 1_614_265_330;
    const BODY: &[u8] = br#"{"test": 2432232314}"#;
    const SIGNATURE: &str = "v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE=";

    fn at(second: i64) -> Timestamp {
        Timestamp::from_second(second).unwrap()
    }

    fn headers(signatures: &str) -> WebhookHeaders<'_> {
        WebhookHeaders {
            id: ID,
            timestamp: "1614265330",
            signatures,
        }
    }

    #[test]
    fn signs_and_verifies_the_specification_example() {
        let secret = WebhookSecret::parse(SECRET).unwrap();

        assert_eq!(secret.sign(ID, SENT, BODY), SIGNATURE);
        assert!(secret.verify(&headers(SIGNATURE), BODY, at(SENT)).is_ok());
    }

    #[test]
    fn any_listed_signature_may_match_and_other_versions_are_skipped() {
        let secret = WebhookSecret::parse(SECRET).unwrap();
        let listed = format!("v1,AAAA v1a,{} {SIGNATURE}", &SIGNATURE[3..]);

        assert!(secret.verify(&headers(&listed), BODY, at(SENT)).is_ok());
    }

    #[test]
    fn a_changed_body_another_secret_or_a_stale_delivery_is_refused() {
        let secret = WebhookSecret::parse(SECRET).unwrap();
        let other = WebhookSecret::parse(&WebhookSecret::generate().unwrap()).unwrap();

        assert!(secret.verify(&headers(SIGNATURE), b"{}", at(SENT)).is_err());
        assert!(other.verify(&headers(SIGNATURE), BODY, at(SENT)).is_err());
        assert!(
            secret
                .verify(&headers(SIGNATURE), BODY, at(SENT + 301))
                .is_err()
        );
        assert!(
            secret
                .verify(&headers(SIGNATURE), BODY, at(SENT - 301))
                .is_err()
        );
    }

    #[test]
    fn a_timestamp_far_from_now_is_refused_without_overflowing() {
        let secret = WebhookSecret::parse(SECRET).unwrap();
        for timestamp in [
            i64::MIN.to_string(),
            i64::MAX.to_string(),
            "soon".to_owned(),
        ] {
            let headers = WebhookHeaders {
                id: ID,
                timestamp: &timestamp,
                signatures: SIGNATURE,
            };
            assert!(secret.verify(&headers, BODY, at(SENT)).is_err());
        }
    }

    #[test]
    fn a_secret_is_read_with_or_without_its_prefix_and_never_shown() {
        let generated = WebhookSecret::generate().unwrap();
        let encoded = generated.strip_prefix(PREFIX).unwrap();

        assert_eq!(
            base64::decode(&STANDARD, encoded).unwrap().len(),
            SECRET_BYTES
        );
        assert_eq!(
            WebhookSecret::parse(&SECRET[PREFIX.len()..])
                .unwrap()
                .sign(ID, SENT, BODY),
            SIGNATURE
        );
        assert!(WebhookSecret::parse("whsec_").is_err());
        assert!(WebhookSecret::parse("whsec_not base64!").is_err());
        assert_eq!(
            format!("{:?}", WebhookSecret::parse(SECRET).unwrap()),
            "WebhookSecret(…)"
        );
    }
}
