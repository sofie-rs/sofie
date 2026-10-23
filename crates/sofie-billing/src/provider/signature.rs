//! Both providers sign `<timestamp><separator><body>` with HMAC-SHA256 under the webhook secret.

use ring::hmac;
use serde_json::Value;
use sofie_core::encoding::hex;
use sofie_core::{Error, Result, Timestamp};
use sofie_http::Delivery;

/// Older or newer deliveries are refused, so a captured one cannot be replayed later.
const TOLERANCE_SECONDS: u64 = 5 * 60;

pub(crate) struct Signed<'a> {
    pub(crate) timestamp: &'a str,
    pub(crate) separator: u8,
    /// Hex, as sent; more than one while a provider rolls its secret.
    pub(crate) signatures: Vec<&'a str>,
}

/// How a provider lays out its signature header.
pub(crate) struct Scheme {
    pub(crate) header: &'static str,
    pub(crate) parts: char,
    pub(crate) timestamp: &'static str,
    pub(crate) signature: &'static str,
    pub(crate) separator: u8,
}

impl Scheme {
    pub(crate) fn verify(&self, delivery: &Delivery<'_>, secret: &str) -> Result<Value> {
        let header = delivery
            .header(self.header)
            .ok_or_else(|| Error::BadRequest(format!("no `{}` header", self.header)))?;
        let mut timestamp = None;
        let mut signatures = Vec::new();
        for part in header.split(self.parts) {
            match part.trim().split_once('=') {
                Some((key, value)) if key == self.timestamp => timestamp = Some(value),
                Some((key, value)) if key == self.signature => signatures.push(value),
                _ => {}
            }
        }
        let signed = Signed {
            timestamp: timestamp.ok_or_else(|| refused("it names no timestamp"))?,
            separator: self.separator,
            signatures,
        };
        check(&signed, delivery.body(), secret, delivery.now())?;
        serde_json::from_slice(delivery.body())
            .map_err(|_| Error::BadRequest("the delivery is not JSON".to_owned()))
    }
}

pub(crate) fn check(signed: &Signed<'_>, body: &[u8], secret: &str, now: Timestamp) -> Result<()> {
    let sent: i64 = signed
        .timestamp
        .parse()
        .map_err(|_| refused("its timestamp is not a number"))?;
    if now.as_second().abs_diff(sent) > TOLERANCE_SECONDS {
        return Err(refused("it was signed too long ago"));
    }
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    let mut message = Vec::with_capacity(signed.timestamp.len() + 1 + body.len());
    message.extend_from_slice(signed.timestamp.as_bytes());
    message.push(signed.separator);
    message.extend_from_slice(body);
    let matches = signed.signatures.iter().any(|signature| {
        hex::decode(signature).is_some_and(|tag| hmac::verify(&key, &message, &tag).is_ok())
    });
    if matches {
        return Ok(());
    }
    Err(refused("no signature matches"))
}

fn refused(why: &str) -> Error {
    Error::BadRequest(format!("the delivery's signature is refused: {why}"))
}

#[cfg(test)]
pub(crate) fn sign(secret: &str, timestamp: &str, separator: u8, body: &[u8]) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    let mut message = timestamp.as_bytes().to_vec();
    message.push(separator);
    message.extend_from_slice(body);
    hex::encode(hmac::sign(&key, &message).as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_760_000_000;

    fn now() -> Timestamp {
        Timestamp::from_second(NOW).unwrap()
    }

    #[test]
    fn a_signature_over_timestamp_and_body_holds_and_anything_else_is_refused() {
        let body = br#"{"id":"evt_1"}"#;
        let timestamp = NOW.to_string();
        let good = sign("whsec_test", &timestamp, b'.', body);
        let signed = |signatures| Signed {
            timestamp: &timestamp,
            separator: b'.',
            signatures,
        };

        assert!(check(&signed(vec!["00", &good]), body, "whsec_test", now()).is_ok());
        assert!(check(&signed(vec![&good]), b"{}", "whsec_test", now()).is_err());
        assert!(check(&signed(vec![&good]), body, "whsec_other", now()).is_err());
        assert!(check(&signed(vec!["zz"]), body, "whsec_test", now()).is_err());
        let later = Timestamp::from_second(NOW + 301).unwrap();
        assert!(check(&signed(vec![&good]), body, "whsec_test", later).is_err());
    }

    #[test]
    fn a_timestamp_far_from_now_is_refused_without_overflowing() {
        for timestamp in [i64::MIN.to_string(), i64::MAX.to_string()] {
            let signed = Signed {
                timestamp: &timestamp,
                separator: b'.',
                signatures: vec!["00"],
            };
            assert!(check(&signed, b"{}", "whsec_test", now()).is_err());
        }
    }
}
