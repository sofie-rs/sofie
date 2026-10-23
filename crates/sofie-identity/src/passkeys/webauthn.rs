//! The relying party's checks of a `WebAuthn` ceremony (Web Authentication level 3, §7),
//! without attestation: a passkey is trusted for the account that registered it.

use ring::digest::{SHA256, digest};
use ring::signature::{
    ECDSA_P256_SHA256_ASN1, ED25519, RSA_PKCS1_2048_8192_SHA256, RsaPublicKeyComponents,
    UnparsedPublicKey,
};
use sofie::core::encoding::base64::{self, URL_SAFE};

use super::cbor::{self, Item};

pub(super) const CREATE: &str = "webauthn.create";
pub(super) const GET: &str = "webauthn.get";
/// ES256, `EdDSA` and RS256, the algorithms every platform authenticator offers one of.
pub(super) const ALGORITHMS: [i32; 3] = [-7, -8, -257];

const USER_PRESENT: u8 = 0x01;
const USER_VERIFIED: u8 = 0x04;
const ATTESTED: u8 = 0x40;

/// Why a ceremony was refused; shown to nobody but the log.
pub(super) type Refused = &'static str;

pub(super) fn challenge(
    client_data_json: &[u8],
    kind: &str,
    origin: &str,
) -> Result<Vec<u8>, Refused> {
    let data: sofie::serde_json::Value = sofie::serde_json::from_slice(client_data_json)
        .map_err(|_| "the client data is not JSON")?;
    if data["type"] != kind {
        return Err("the client data is for another ceremony");
    }
    if data["origin"] != origin || data["crossOrigin"] == true {
        return Err("the client data names another origin");
    }
    data["challenge"]
        .as_str()
        .and_then(|challenge| base64::decode(&URL_SAFE, challenge))
        .ok_or("the challenge is not base64url")
}

pub(super) struct AuthenticatorData<'a> {
    rp_id_hash: &'a [u8],
    flags: u8,
    pub(super) sign_count: u32,
    /// The registered credential's id and COSE key; only in a registration.
    pub(super) credential: Option<(&'a [u8], &'a [u8])>,
}

impl AuthenticatorData<'_> {
    pub(super) fn is_user_verified(&self) -> bool {
        self.flags & USER_VERIFIED != 0
    }

    pub(super) fn check(&self, rp_id: &str) -> Result<(), Refused> {
        if self.rp_id_hash != digest(&SHA256, rp_id.as_bytes()).as_ref() {
            return Err("the authenticator signed for another relying party");
        }
        if self.flags & USER_PRESENT == 0 {
            return Err("no user was present");
        }
        Ok(())
    }
}

pub(super) fn authenticator_data(bytes: &[u8]) -> Result<AuthenticatorData<'_>, Refused> {
    const TRUNCATED: Refused = "the authenticator data is truncated";
    let (rp_id_hash, rest) = bytes.split_at_checked(32).ok_or(TRUNCATED)?;
    let (&flags, rest) = rest.split_first().ok_or(TRUNCATED)?;
    let (count, rest) = rest.split_first_chunk::<4>().ok_or(TRUNCATED)?;
    let credential = if flags & ATTESTED == 0 {
        None
    } else {
        let (_aaguid, rest) = rest.split_at_checked(16).ok_or(TRUNCATED)?;
        let (length, rest) = rest.split_first_chunk::<2>().ok_or(TRUNCATED)?;
        let (id, rest) = rest
            .split_at_checked(usize::from(u16::from_be_bytes(*length)))
            .ok_or(TRUNCATED)?;
        let (_, after) = cbor::read(rest).ok_or("the credential's public key is not CBOR")?;
        Some((id, &rest[..rest.len() - after.len()]))
    };
    Ok(AuthenticatorData {
        rp_id_hash,
        flags,
        sign_count: u32::from_be_bytes(*count),
        credential,
    })
}

pub(super) fn attested(attestation_object: &[u8]) -> Result<&[u8], Refused> {
    let (object, _) = cbor::read(attestation_object).ok_or("the attestation object is not CBOR")?;
    object
        .by_text("authData")
        .and_then(Item::bytes)
        .ok_or("the attestation object holds no authenticator data")
}

enum Key {
    Es256(Vec<u8>),
    Ed25519(Vec<u8>),
    Rs256 { n: Vec<u8>, e: Vec<u8> },
}

pub(super) fn algorithm(cose_key: &[u8]) -> Result<i32, Refused> {
    key(cose_key).map(|(_, algorithm)| algorithm)
}

fn key(cose_key: &[u8]) -> Result<(Key, i32), Refused> {
    const UNSUPPORTED: Refused = "the public key is of a kind not supported";
    let (key, _) = cbor::read(cose_key).ok_or("the public key is not CBOR")?;
    let number = |label| key.by_number(label).and_then(Item::integer);
    let bytes = |label| {
        key.by_number(label)
            .and_then(Item::bytes)
            .map(<[u8]>::to_vec)
    };
    let algorithm = number(3)
        .and_then(|value| i32::try_from(value).ok())
        .ok_or(UNSUPPORTED)?;
    let parsed = match (number(1), algorithm, number(-1)) {
        (Some(2), -7, Some(1)) => {
            let (x, y) = bytes(-2).zip(bytes(-3)).ok_or(UNSUPPORTED)?;
            if x.len() != 32 || y.len() != 32 {
                return Err(UNSUPPORTED);
            }
            Key::Es256([&[0x04][..], &x, &y].concat())
        }
        (Some(1), -8, Some(6)) => {
            Key::Ed25519(bytes(-2).filter(|x| x.len() == 32).ok_or(UNSUPPORTED)?)
        }
        (Some(3), -257, _) => {
            let (n, e) = bytes(-1).zip(bytes(-2)).ok_or(UNSUPPORTED)?;
            Key::Rs256 { n, e }
        }
        _ => return Err(UNSUPPORTED),
    };
    Ok((parsed, algorithm))
}

/// Over the authenticator data followed by the digest of the client data.
pub(super) fn verifies(
    cose_key: &[u8],
    authenticator_data: &[u8],
    client_data_json: &[u8],
    signature: &[u8],
) -> bool {
    let Ok((key, _)) = key(cose_key) else {
        return false;
    };
    let signed = [
        authenticator_data,
        digest(&SHA256, client_data_json).as_ref(),
    ]
    .concat();
    match key {
        Key::Es256(point) => UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, point)
            .verify(&signed, signature)
            .is_ok(),
        Key::Ed25519(point) => UnparsedPublicKey::new(&ED25519, point)
            .verify(&signed, signature)
            .is_ok(),
        Key::Rs256 { n, e } => RsaPublicKeyComponents { n, e }
            .verify(&RSA_PKCS1_2048_8192_SHA256, &signed, signature)
            .is_ok(),
    }
}

#[cfg(test)]
mod tests {
    use ring::rand::SystemRandom;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    use super::*;

    fn cose_ed25519(public: &[u8]) -> Vec<u8> {
        [
            &[0xa4, 0x01, 0x01, 0x03, 0x27, 0x20, 0x06, 0x21, 0x58, 0x20][..],
            public,
        ]
        .concat()
    }

    fn signed_in_data(rp_id: &str, flags: u8, count: u32) -> Vec<u8> {
        [
            digest(&SHA256, rp_id.as_bytes()).as_ref(),
            &[flags],
            &count.to_be_bytes(),
        ]
        .concat()
    }

    #[test]
    fn an_assertion_verifies_with_its_key_and_no_other_data() {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let cose = cose_ed25519(pair.public_key().as_ref());
        let data = signed_in_data("example.com", USER_PRESENT | USER_VERIFIED, 7);
        let client =
            br#"{"type":"webauthn.get","challenge":"AQID","origin":"https://example.com"}"#;
        let signed = [&data[..], digest(&SHA256, client).as_ref()].concat();
        let signature = pair.sign(&signed);

        assert_eq!(algorithm(&cose), Ok(-8));
        assert!(verifies(&cose, &data, client, signature.as_ref()));
        assert!(!verifies(
            &cose,
            &data,
            br#"{"type":"webauthn.get"}"#,
            signature.as_ref()
        ));
        let parsed = authenticator_data(&data).unwrap();
        assert_eq!(parsed.sign_count, 7);
        assert!(parsed.is_user_verified());
        assert!(parsed.check("example.com").is_ok());
        assert!(parsed.check("evil.example").is_err());
        assert_eq!(
            challenge(client, GET, "https://example.com"),
            Ok(vec![1, 2, 3])
        );
        assert!(challenge(client, CREATE, "https://example.com").is_err());
        assert!(challenge(client, GET, "https://evil.example").is_err());
    }

    #[test]
    fn a_registration_names_its_credential_and_key() {
        let cose = cose_ed25519(&[9; 32]);
        let data = [
            &signed_in_data("example.com", USER_PRESENT | ATTESTED, 0)[..],
            &[0; 16],
            &[0x00, 0x02, 0xab, 0xcd],
            &cose,
        ]
        .concat();
        let mut object = vec![0xa3, 0x63, b'f', b'm', b't', 0x64, b'n', b'o', b'n', b'e'];
        object.extend([0x67, b'a', b't', b't', b'S', b't', b'm', b't', 0xa0]);
        object.extend([0x68, b'a', b'u', b't', b'h', b'D', b'a', b't', b'a', 0x58]);
        object.push(u8::try_from(data.len()).unwrap());
        object.extend(&data);

        let parsed = authenticator_data(attested(&object).unwrap()).unwrap();

        assert_eq!(parsed.credential, Some((&[0xab, 0xcd][..], &cose[..])));
        assert!(authenticator_data(&data[..40]).is_err());
    }
}
