use sofie_auth::{WEBHOOK_ID, WEBHOOK_SIGNATURE, WEBHOOK_TIMESTAMP, WebhookHeaders, WebhookSecret};
use sofie_core::{Error, Result};

use super::{Delivery, Verify};

/// Deliveries from any sender of Standard Webhooks, sofie-webhooks among them; the `webhook-id`
/// is the delivery id, so a retried event runs once.
#[derive(Debug)]
pub struct StandardWebhooks(WebhookSecret);

impl StandardWebhooks {
    /// The secret the sender showed, `whsec_…`.
    pub fn new(secret: &str) -> Result<Self> {
        WebhookSecret::parse(secret).map(Self)
    }
}

impl Verify for StandardWebhooks {
    fn verify(&self, delivery: &Delivery<'_>) -> Result<String> {
        let header = |name| {
            delivery
                .header(name)
                .ok_or_else(|| Error::BadRequest(format!("no `{name}` header")))
        };
        let headers = WebhookHeaders {
            id: header(WEBHOOK_ID)?,
            timestamp: header(WEBHOOK_TIMESTAMP)?,
            signatures: header(WEBHOOK_SIGNATURE)?,
        };
        self.0.verify(&headers, delivery.body(), delivery.now())?;
        Ok(headers.id.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use http::{HeaderMap, HeaderValue};
    use sofie_core::Timestamp;

    use super::*;

    const SECRET: &str = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw";
    const SENT: i64 = 1_614_265_330;
    const BODY: &[u8] = br#"{"test": 2432232314}"#;

    fn verified(headers: &[(&'static str, &str)], now: i64) -> Result<String> {
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            map.insert(*name, HeaderValue::from_str(value).unwrap());
        }
        let delivery = Delivery {
            headers: &map,
            body: BODY,
            now: Timestamp::from_second(now).unwrap(),
        };
        StandardWebhooks::new(SECRET).unwrap().verify(&delivery)
    }

    #[test]
    fn a_signed_delivery_is_known_by_its_webhook_id() {
        let headers = [
            (WEBHOOK_ID, "msg_p5jXN8AQM9LWM0D4loKWxJek"),
            (WEBHOOK_TIMESTAMP, "1614265330"),
            (
                WEBHOOK_SIGNATURE,
                "v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE=",
            ),
        ];

        assert_eq!(
            verified(&headers, SENT).unwrap(),
            "msg_p5jXN8AQM9LWM0D4loKWxJek"
        );
        assert!(verified(&headers, SENT + 600).is_err());
    }

    #[test]
    fn a_missing_header_or_a_forged_signature_is_refused() {
        let unsigned = [
            (WEBHOOK_ID, "msg_p5jXN8AQM9LWM0D4loKWxJek"),
            (WEBHOOK_TIMESTAMP, "1614265330"),
        ];
        let forged = [
            (WEBHOOK_ID, "msg_other"),
            (WEBHOOK_TIMESTAMP, "1614265330"),
            (
                WEBHOOK_SIGNATURE,
                "v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE=",
            ),
        ];

        assert!(verified(&unsigned, SENT).is_err());
        assert!(verified(&forged, SENT).is_err());
        assert!(StandardWebhooks::new("not a secret!").is_err());
    }
}
