//! RFC 8292: the app proves to a push service that it holds the key the browser subscribed with.

use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair};
use serde_json::json;
use sofie_core::encoding::base64::{URL_SAFE, decode, encode};
use sofie_core::{Error, Result, Timestamp};

/// The longest a push service accepts is a day; half leaves room for clock skew.
const VALID_SECONDS: i64 = 12 * 60 * 60;

pub struct VapidKeys {
    pair: EcdsaKeyPair,
    public: String,
    subject: String,
}

impl VapidKeys {
    /// Base64url, as key generators print them: the 65-byte public key and the 32-byte private one.
    pub fn new(public: &str, private: &str, subject: &str) -> Result<Self> {
        let malformed = |what: &str| {
            Error::internal_message(format!(
                "the VAPID {what} key is not base64url of a P-256 key"
            ))
        };
        let public_bytes = decode(&URL_SAFE, public).ok_or_else(|| malformed("public"))?;
        let private_bytes = decode(&URL_SAFE, private).ok_or_else(|| malformed("private"))?;
        let pair = EcdsaKeyPair::from_private_key_and_public_key(
            &ECDSA_P256_SHA256_FIXED_SIGNING,
            &private_bytes,
            &public_bytes,
            &SystemRandom::new(),
        )
        .map_err(|_| malformed("private or public"))?;
        if !(subject.starts_with("mailto:") || subject.starts_with("https://")) {
            return Err(Error::internal_message(
                "the VAPID subject is a mailto: or https: address",
            ));
        }
        Ok(Self {
            pair,
            public: encode(&URL_SAFE, &public_bytes),
            subject: subject.to_owned(),
        })
    }

    /// What a browser subscribes with, as `applicationServerKey`.
    #[must_use]
    pub fn public_key(&self) -> &str {
        &self.public
    }

    pub(crate) fn authorization(&self, endpoint: &str, now: Timestamp) -> Result<String> {
        let header = encode(&URL_SAFE, br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = json!({
            "aud": audience(endpoint)?,
            "exp": now.as_second() + VALID_SECONDS,
            "sub": self.subject,
        });
        let claims = encode(&URL_SAFE, claims.to_string().as_bytes());
        let signed = format!("{header}.{claims}");
        let signature = self
            .pair
            .sign(&SystemRandom::new(), signed.as_bytes())
            .map_err(|_| Error::internal_message("signing a VAPID token failed"))?;
        let signature = encode(&URL_SAFE, signature.as_ref());
        Ok(format!("vapid t={signed}.{signature}, k={}", self.public))
    }
}

pub(super) fn audience(endpoint: &str) -> Result<&str> {
    let start = endpoint
        .find("://")
        .ok_or_else(|| Error::internal_message("a push endpoint is not a URL"))?
        + 3;
    let end = endpoint[start..]
        .find('/')
        .map_or(endpoint.len(), |slash| start + slash);
    Ok(&endpoint[..end])
}

#[cfg(test)]
pub(crate) mod tests {
    use ring::signature::{ECDSA_P256_SHA256_FIXED, KeyPair, UnparsedPublicKey};

    use super::*;

    pub(crate) fn generated() -> (String, String) {
        let random = SystemRandom::new();
        let pkcs8 =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &random).unwrap();
        let pair =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &random)
                .unwrap();
        // A P-256 PKCS#8 document ends with the public key, the private scalar sits at 36..68.
        let private = &pkcs8.as_ref()[36..68];
        (
            encode(&URL_SAFE, pair.public_key().as_ref()),
            encode(&URL_SAFE, private),
        )
    }

    #[test]
    fn a_token_names_the_push_services_origin_and_verifies_with_the_public_key() {
        let (public, private) = generated();
        let keys = VapidKeys::new(&public, &private, "mailto:ops@example.test").unwrap();
        let now = Timestamp::from_second(1_760_000_000).unwrap();

        let authorization = keys
            .authorization("https://fcm.googleapis.com/fcm/send/abc", now)
            .unwrap();

        let token = authorization
            .strip_prefix("vapid t=")
            .unwrap()
            .split(", k=")
            .next()
            .unwrap();
        let (signed, signature) = token.rsplit_once('.').unwrap();
        let claims = decode(&URL_SAFE, signed.split('.').nth(1).unwrap()).unwrap();
        let claims: serde_json::Value = serde_json::from_slice(&claims).unwrap();
        assert_eq!(claims["aud"], "https://fcm.googleapis.com");
        assert_eq!(claims["exp"], 1_760_043_200);
        let public_bytes = decode(&URL_SAFE, &public).unwrap();
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, public_bytes)
            .verify(signed.as_bytes(), &decode(&URL_SAFE, signature).unwrap())
            .unwrap();
        assert!(authorization.ends_with(&format!(", k={public}")));
    }

    #[test]
    fn keys_that_are_not_a_pair_are_refused() {
        let (public, _) = generated();
        let (_, other_private) = generated();

        assert!(VapidKeys::new(&public, &other_private, "mailto:ops@example.test").is_err());
        assert!(VapidKeys::new("not base64!", "x", "mailto:a@b.test").is_err());
    }
}
