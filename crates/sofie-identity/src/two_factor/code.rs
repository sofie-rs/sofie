//! Time-based one-time passwords (RFC 6238 over RFC 4226) and recovery codes.

use ring::hmac;
use sofie::core::{Error, Result};

const STEP_SECONDS: i64 = 30;
const DIGITS: u32 = 6;
/// 160 bits, as RFC 4226 recommends for HMAC-SHA1.
const SECRET_BYTES: usize = 20;
/// One step either side, for a clock a little off.
const WINDOW: i64 = 1;
const RECOVERY_CODES: usize = 10;
/// 80 bits, so a leaked digest gives no code back by brute force.
const RECOVERY_BYTES: usize = 10;
const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

pub(crate) fn new_secret() -> Result<Vec<u8>> {
    random(SECRET_BYTES)
}

fn random(length: usize) -> Result<Vec<u8>> {
    let mut bytes = vec![0; length];
    sofie::core::random::fill(&mut bytes)
        .map_err(|error| Error::internal("drawing a second factor's secrets", error))?;
    Ok(bytes)
}

/// The step `code` belongs to, if it is one of the steps around `now` later than `after`.
pub(crate) fn matching_step(
    secret: &[u8],
    code: &str,
    now_seconds: i64,
    after: i64,
) -> Option<i64> {
    let code: String = code
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    if code.len() != DIGITS as usize || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let current = now_seconds.div_euclid(STEP_SECONDS);
    (current - WINDOW..=current + WINDOW)
        .filter(|step| *step > after)
        .find(|step| {
            format!(
                "{:0width$}",
                code_at(secret, *step),
                width = DIGITS as usize
            ) == code
        })
}

fn code_at(secret: &[u8], step: i64) -> u32 {
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, secret);
    let tag = hmac::sign(&key, &step.to_be_bytes());
    let digest = tag.as_ref();
    let offset = usize::from(digest[digest.len() - 1] & 0x0f);
    let truncated = u32::from_be_bytes([
        digest[offset] & 0x7f,
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]);
    truncated % 10u32.pow(DIGITS)
}

/// RFC 4648 base32 without padding, as authenticator apps take a key.
pub(crate) fn base32(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let (mut buffer, mut bits) = (0u32, 0);
    for byte in bytes {
        buffer = (buffer << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            text.push(char::from(BASE32[((buffer >> bits) & 0x1f) as usize]));
        }
    }
    if bits > 0 {
        text.push(char::from(BASE32[((buffer << (5 - bits)) & 0x1f) as usize]));
    }
    text
}

/// The issuer both prefixes the label and stands alone, as authenticator apps read either.
pub(crate) fn otpauth_uri(issuer: &str, account: &str, secret: &[u8]) -> String {
    let issuer = sofie::http::path_segment(&issuer);
    let account = sofie::http::path_segment(&account);
    format!(
        "otpauth://totp/{issuer}:{account}?secret={}&issuer={issuer}",
        base32(secret)
    )
}

pub(crate) fn recovery_codes() -> Result<Vec<String>> {
    (0..RECOVERY_CODES)
        .map(|_| {
            let text = base32(&random(RECOVERY_BYTES)?).to_ascii_lowercase();
            Ok(format!(
                "{}-{}-{}-{}",
                &text[..4],
                &text[4..8],
                &text[8..12],
                &text[12..16]
            ))
        })
        .collect()
}

/// Case, spaces and dashes do not count, so a code read aloud or retyped still matches.
pub(crate) fn recovery_digest(code: &str) -> String {
    let normalized: String = code
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|character| character.to_ascii_lowercase())
        .collect();
    sofie::auth::digest(&[normalized.as_bytes()])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6238 appendix B, SHA-1, cut to six digits.
    #[test]
    fn codes_match_the_rfc_test_vectors() {
        let secret = b"12345678901234567890";
        for (time, code) in [
            (59, "287082"),
            (1_111_111_109, "081804"),
            (1_234_567_890, "005924"),
            (2_000_000_000, "279037"),
        ] {
            let step = time / STEP_SECONDS;
            assert_eq!(
                matching_step(secret, code, time, i64::MIN),
                Some(step),
                "{time}"
            );
        }
    }

    #[test]
    fn a_code_works_one_step_either_side_and_never_twice() {
        let secret = b"12345678901234567890";
        assert_eq!(matching_step(secret, "287 082", 59 + 30, i64::MIN), Some(1));
        assert_eq!(matching_step(secret, "287082", 59 + 60, i64::MIN), None);
        assert_eq!(matching_step(secret, "287082", 59, 1), None);
        assert_eq!(matching_step(secret, "28708", 59, i64::MIN), None);
    }

    #[test]
    fn base32_spells_the_rfc_examples() {
        assert_eq!(base32(b"foobar"), "MZXW6YTBOI");
        assert_eq!(base32(b"f"), "MY");
    }

    #[test]
    fn a_recovery_code_matches_however_it_is_typed() {
        let codes = recovery_codes().unwrap();
        assert_eq!(codes.len(), 10);
        let code = &codes[0];
        assert_eq!(code.len(), 19);
        assert_eq!(
            recovery_digest(code),
            recovery_digest(&code.to_uppercase().replace('-', " "))
        );
    }
}
