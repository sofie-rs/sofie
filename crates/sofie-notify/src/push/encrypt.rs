//! RFC 8291: a push message encrypted to the browser's key with `aes128gcm` (RFC 8188).

use ring::rand::{SecureRandom, SystemRandom};
use ring::{aead, agreement, hkdf};
use sofie_core::{Error, Result};

/// One record holds the whole message; the push service takes up to 4096 bytes.
const RECORD_SIZE: u32 = 4096;
const TAG: usize = 16;
/// The record's size, less its tag and the delimiter octet after the plaintext.
pub(crate) const MAXIMUM_PLAINTEXT: usize = RECORD_SIZE as usize - TAG - 1;

pub(crate) fn encrypt(browser_public: &[u8], auth: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    if plaintext.len() > MAXIMUM_PLAINTEXT {
        return Err(Error::internal_message(
            "a push message is longer than one record",
        ));
    }
    let random = SystemRandom::new();
    let mut salt = [0; 16];
    random
        .fill(&mut salt)
        .map_err(|_| failed("drawing a salt"))?;
    let private = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &random)
        .map_err(|_| failed("generating a key"))?;
    let server_public = private
        .compute_public_key()
        .map_err(|_| failed("computing the public key"))?;
    let browser = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, browser_public);
    agreement::agree_ephemeral(private, &browser, |secret| {
        seal(
            &Keys {
                secret,
                auth,
                browser_public,
                server_public: server_public.as_ref(),
                salt: &salt,
            },
            plaintext,
        )
    })
    .map_err(|_| failed("agreeing on a secret with the browser's key"))?
}

struct Keys<'a> {
    secret: &'a [u8],
    auth: &'a [u8],
    browser_public: &'a [u8],
    server_public: &'a [u8],
    salt: &'a [u8],
}

fn seal(keys: &Keys<'_>, plaintext: &[u8]) -> Result<Vec<u8>> {
    let mut key_info = b"WebPush: info\0".to_vec();
    key_info.extend_from_slice(keys.browser_public);
    key_info.extend_from_slice(keys.server_public);
    let ikm = expand(keys.auth, keys.secret, &key_info, 32)?;
    let cek = expand(keys.salt, &ikm, b"Content-Encoding: aes128gcm\0", 16)?;
    let nonce = expand(keys.salt, &ikm, b"Content-Encoding: nonce\0", 12)?;
    let key = aead::UnboundKey::new(&aead::AES_128_GCM, &cek)
        .map_err(|_| failed("building the content key"))?;
    let nonce =
        aead::Nonce::try_assume_unique_for_key(&nonce).map_err(|_| failed("building the nonce"))?;
    let mut record = plaintext.to_vec();
    record.push(2);
    aead::LessSafeKey::new(key)
        .seal_in_place_append_tag(nonce, aead::Aad::empty(), &mut record)
        .map_err(|_| failed("sealing the record"))?;
    let server_public_length =
        u8::try_from(keys.server_public.len()).map_err(|_| failed("sizing the key id"))?;
    let mut body = Vec::with_capacity(21 + keys.server_public.len() + record.len());
    body.extend_from_slice(keys.salt);
    body.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    body.push(server_public_length);
    body.extend_from_slice(keys.server_public);
    body.extend_from_slice(&record);
    Ok(body)
}

struct Length(usize);

impl hkdf::KeyType for Length {
    fn len(&self) -> usize {
        self.0
    }
}

fn expand(salt: &[u8], input: &[u8], info: &[u8], length: usize) -> Result<Vec<u8>> {
    let pseudorandom = hkdf::Salt::new(hkdf::HKDF_SHA256, salt).extract(input);
    let info = [info];
    let okm = pseudorandom
        .expand(&info, Length(length))
        .map_err(|_| failed("deriving a key"))?;
    let mut out = vec![0; length];
    okm.fill(&mut out).map_err(|_| failed("deriving a key"))?;
    Ok(out)
}

fn failed(step: &str) -> Error {
    Error::internal_message(format!("encrypting a push message failed while {step}"))
}

#[cfg(test)]
mod tests {
    use sofie_core::encoding::base64::{URL_SAFE, decode, encode};

    use super::*;

    fn bytes(text: &str) -> Vec<u8> {
        decode(&URL_SAFE, &text.replace(' ', "")).unwrap()
    }

    /// RFC 8291 appendix A.
    #[test]
    fn the_rfc_example_encrypts_to_the_rfc_message() {
        let keys = Keys {
            secret: &bytes("kyrL1jIIOHEzg3sM2ZWRHDRB62YACZhhSlknJ672kSs"),
            auth: &bytes("BTBZMqHH6r4Tts7J_aSIgg"),
            browser_public: &bytes(
                "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4",
            ),
            server_public: &bytes(
                "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8",
            ),
            salt: &bytes("DGv6ra1nlYgDCS1FRnbzlw"),
        };

        let body = seal(&keys, b"When I grow up, I want to be a watermelon").unwrap();

        assert_eq!(
            encode(&URL_SAFE, &body),
            "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27ml\
             mlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPT\
             pK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN"
        );
    }

    #[test]
    fn the_browser_opens_what_was_encrypted_to_it() {
        let random = SystemRandom::new();
        let browser =
            agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &random).unwrap();
        let browser_public = browser.compute_public_key().unwrap();
        let auth = [7; 16];

        let body = encrypt(browser_public.as_ref(), &auth, b"{\"title\":\"Shipped\"}").unwrap();

        let (salt, rest) = body.split_at(16);
        let server_public = &rest[5..70];
        let opened = agreement::agree_ephemeral(
            browser,
            &agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, server_public),
            |secret| {
                let mut key_info = b"WebPush: info\0".to_vec();
                key_info.extend_from_slice(browser_public.as_ref());
                key_info.extend_from_slice(server_public);
                let ikm = expand(&auth, secret, &key_info, 32).unwrap();
                let cek = expand(salt, &ikm, b"Content-Encoding: aes128gcm\0", 16).unwrap();
                let nonce = expand(salt, &ikm, b"Content-Encoding: nonce\0", 12).unwrap();
                let key = aead::LessSafeKey::new(
                    aead::UnboundKey::new(&aead::AES_128_GCM, &cek).unwrap(),
                );
                let mut record = rest[70..].to_vec();
                let plain = key
                    .open_in_place(
                        aead::Nonce::try_assume_unique_for_key(&nonce).unwrap(),
                        aead::Aad::empty(),
                        &mut record,
                    )
                    .unwrap();
                plain.to_vec()
            },
        )
        .unwrap();

        assert_eq!(opened, b"{\"title\":\"Shipped\"}\x02");
    }
}
