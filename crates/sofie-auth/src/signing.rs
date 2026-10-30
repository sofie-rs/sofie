use ring::{hkdf, hmac};
use sofie_core::encoding::base64::{self, URL_SAFE};

use crate::SigningKeys;
use crate::session::KEY_LENGTH;

const SIGNING_PURPOSE: &[u8] = b"sofie-signature-v1";

/// Each purpose signs with its own derived key, so one purpose's signature never passes for another's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signed {
    Link,
    StoredFile,
    PrincipalToken,
    UploadSeal,
    ShieldBlock,
}

const PURPOSES: [Signed; 5] = [
    Signed::Link,
    Signed::StoredFile,
    Signed::PrincipalToken,
    Signed::UploadSeal,
    Signed::ShieldBlock,
];

impl Signed {
    const fn info(self) -> &'static [u8] {
        match self {
            Self::Link => b"link",
            Self::StoredFile => b"stored-file",
            Self::PrincipalToken => b"principal-token",
            Self::UploadSeal => b"upload-seal",
            Self::ShieldBlock => b"shield-block",
        }
    }
}

pub(crate) struct PurposeKeys([hmac::Key; PURPOSES.len()]);

impl PurposeKeys {
    fn of(&self, purpose: Signed) -> &hmac::Key {
        &self.0[purpose as usize]
    }
}

impl SigningKeys {
    #[must_use]
    pub fn sign(&self, purpose: Signed, message: &[u8]) -> String {
        let tag = hmac::sign(self.first().signing.of(purpose), message);
        base64::encode(&URL_SAFE, tag.as_ref())
    }

    #[must_use]
    pub fn verifies(&self, purpose: Signed, message: &[u8], signature: &str) -> bool {
        base64::decode(&URL_SAFE, signature).is_some_and(|tag| {
            self.all()
                .any(|key| hmac::verify(key.signing.of(purpose), message, &tag).is_ok())
        })
    }
}

pub(crate) fn purpose_keys(key: &[u8; KEY_LENGTH]) -> PurposeKeys {
    let secret = hkdf::Salt::new(hkdf::HKDF_SHA256, &[]).extract(key);
    PurposeKeys(PURPOSES.map(|purpose| {
        secret
            .expand(&[SIGNING_PURPOSE, purpose.info()], hmac::HMAC_SHA256)
            .map(hmac::Key::from)
            .expect("HKDF-SHA256 expands to the 32 bytes of an HMAC-SHA256 key")
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::tests::{claims, keys};

    #[test]
    fn a_signature_verifies_only_its_message_under_a_key_still_held() {
        let message = b"/files/1?expires=1700000000";
        let signature = keys(1).sign(Signed::Link, message);
        let rotated = SigningKeys::new(&[[2; KEY_LENGTH], [1; KEY_LENGTH]]).unwrap();

        assert!(keys(1).verifies(Signed::Link, message, &signature));
        assert!(rotated.verifies(Signed::Link, message, &signature));
        assert!(!keys(1).verifies(Signed::Link, b"/files/2?expires=1700000000", &signature));
        assert!(!keys(1).verifies(Signed::StoredFile, message, &signature));
        assert!(!keys(2).verifies(Signed::Link, message, &signature));
        assert!(!keys(1).verifies(Signed::Link, message, "not base64!"));
        assert!(!keys(1).verifies(Signed::Link, message, &claims().seal(&keys(1)).unwrap()));
    }
}
