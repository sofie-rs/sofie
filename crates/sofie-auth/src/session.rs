use std::fmt;
use std::time::Duration;

use ring::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use sofie_core::{Error, Result, Timestamp, Uuid};

use crate::signing::PurposeKeys;
use sofie_core::encoding::base64::{self, STANDARD, URL_SAFE};

pub const SIGNING_KEYS: &str = "SOFIE_SIGNING_KEYS";
/// Read until apps move to `SOFIE_SIGNING_KEYS`; `sofie::run` warns while it is in use.
pub const OLD_SIGNING_KEYS: &str = "SOFIE_SESSION_KEYS";
pub(crate) const KEY_LENGTH: usize = 32;
const ABSOLUTE_LIFETIME_SECONDS: i64 = 30 * 24 * 60 * 60;
const IDLE_TIMEOUT_SECONDS: i64 = 12 * 60 * 60;
const REFRESH_AFTER_SECONDS: i64 = 5 * 60;
/// Bound to every seal, so a value sealed for anything else never opens.
const PURPOSE: &[u8] = b"sofie-session-v2";
/// Prefixes a sealed value's own purpose, so no value purpose can spell the session's.
const VALUE_PURPOSE: &[u8] = b"sofie-value-v1:";
const FIXED_LENGTH: usize = 16 + 16 + 8 + 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionClaims {
    /// Chosen at sign-in and kept through refreshes, so a store can list and end the session.
    session: Uuid,
    principal: Uuid,
    area: String,
    issued_at: i64,
    last_seen: i64,
}

impl SessionClaims {
    pub const ABSOLUTE_LIFETIME: Duration =
        Duration::from_secs(ABSOLUTE_LIFETIME_SECONDS.unsigned_abs());

    #[must_use]
    pub fn new(session: Uuid, area: &str, principal: Uuid, now: Timestamp) -> Self {
        Self {
            session,
            principal,
            area: area.to_owned(),
            issued_at: now.as_second(),
            last_seen: now.as_second(),
        }
    }

    /// For a session signed back in, which keeps its first sign-in's absolute lifetime.
    #[must_use]
    pub fn issued_at(self, at: Timestamp) -> Self {
        Self {
            issued_at: at.as_second(),
            ..self
        }
    }

    #[must_use]
    pub fn session(&self) -> Uuid {
        self.session
    }

    #[must_use]
    pub fn principal(&self) -> Uuid {
        self.principal
    }

    #[must_use]
    pub fn area(&self) -> &str {
        &self.area
    }

    #[must_use]
    pub fn remaining_seconds(&self, now: Timestamp) -> i64 {
        (self.issued_at + ABSOLUTE_LIFETIME_SECONDS - now.as_second()).max(0)
    }

    #[must_use]
    pub fn needs_refresh(&self, now: Timestamp) -> bool {
        now.as_second() - self.last_seen >= REFRESH_AFTER_SECONDS
    }

    #[must_use]
    pub fn seen_at(self, now: Timestamp) -> Self {
        Self {
            last_seen: now.as_second(),
            ..self
        }
    }

    pub fn seal(&self, keys: &SigningKeys) -> Result<String> {
        keys.sealed(PURPOSE, self.to_bytes())
    }

    pub fn open(
        keys: &SigningKeys,
        sealed: &str,
        now: Timestamp,
    ) -> std::result::Result<Self, OpenError> {
        let plain = keys.opened(PURPOSE, sealed)?;
        let claims = Self::from_bytes(&plain).ok_or(OpenError::Corrupt)?;
        if !claims.is_live(now) {
            return Err(OpenError::Expired);
        }
        Ok(claims)
    }

    fn is_live(&self, now: Timestamp) -> bool {
        let now = now.as_second();
        now < self.issued_at + ABSOLUTE_LIFETIME_SECONDS
            && now < self.last_seen + IDLE_TIMEOUT_SECONDS
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(FIXED_LENGTH + self.area.len());
        bytes.extend_from_slice(self.session.as_bytes());
        bytes.extend_from_slice(self.principal.as_bytes());
        bytes.extend_from_slice(&self.issued_at.to_be_bytes());
        bytes.extend_from_slice(&self.last_seen.to_be_bytes());
        bytes.extend_from_slice(self.area.as_bytes());
        bytes
    }

    fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let (session, rest) = bytes.split_first_chunk::<16>()?;
        let (principal, rest) = rest.split_first_chunk::<16>()?;
        let (issued_at, rest) = rest.split_first_chunk::<8>()?;
        let (last_seen, area) = rest.split_first_chunk::<8>()?;
        Some(Self {
            session: Uuid::from_bytes(*session),
            principal: Uuid::from_bytes(*principal),
            area: String::from_utf8(area.to_vec()).ok()?,
            issued_at: i64::from_be_bytes(*issued_at),
            last_seen: i64::from_be_bytes(*last_seen),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenError {
    Malformed,
    Unauthenticated,
    Expired,
    /// Opened but holds no claims: sealed by a framework bug, not by a client.
    Corrupt,
}

impl fmt::Display for OpenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Malformed => "the sealed session is malformed",
            Self::Unauthenticated => "no key held opens the sealed session",
            Self::Expired => "the session has expired",
            Self::Corrupt => "the sealed session opened but holds no claims",
        })
    }
}

impl std::error::Error for OpenError {}

/// The first key seals and signs, every key opens: rotate by putting the successor first.
pub struct SigningKeys {
    first: Key,
    rest: Vec<Key>,
}

pub(crate) struct Key {
    sealing: LessSafeKey,
    pub(crate) signing: PurposeKeys,
}

impl Key {
    fn new(key: &[u8; KEY_LENGTH]) -> Self {
        let sealing = UnboundKey::new(&AES_256_GCM, key)
            .expect("AES-256-GCM takes any key of KEY_LENGTH bytes");
        Self {
            sealing: LessSafeKey::new(sealing),
            signing: crate::signing::purpose_keys(key),
        }
    }
}

impl SigningKeys {
    pub fn from_environment() -> Result<Self> {
        Self::read(sofie_core::environment_variable)
    }

    pub fn read(lookup: impl Fn(&str) -> Result<Option<String>>) -> Result<Self> {
        let text = keys_text(lookup)?.ok_or_else(|| {
            keys_error("is not set; set it to one or more comma-separated base64 keys of 32 bytes")
        })?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self> {
        let keys = text
            .split(',')
            .enumerate()
            .map(|(index, key)| decode_key(index, key.trim()))
            .collect::<Result<Vec<_>>>()?;
        Self::new(&keys)
    }

    pub fn new(keys: &[[u8; KEY_LENGTH]]) -> Result<Self> {
        let (first, rest) = keys
            .split_first()
            .ok_or_else(|| keys_error("holds no key"))?;
        Ok(Self {
            first: Key::new(first),
            rest: rest.iter().map(Key::new).collect(),
        })
    }

    pub(crate) fn first(&self) -> &Key {
        &self.first
    }

    pub(crate) fn all(&self) -> impl Iterator<Item = &Key> {
        std::iter::once(&self.first).chain(&self.rest)
    }

    pub fn generate() -> Result<Self> {
        Self::new(&[crate::random_bytes("a key")?])
    }

    pub fn seal_value(&self, purpose: &str, plain: &[u8]) -> Result<String> {
        self.sealed(&value_purpose(purpose), plain.to_vec())
    }

    #[must_use]
    pub fn open_value(&self, purpose: &str, sealed: &str) -> Option<Vec<u8>> {
        self.opened(&value_purpose(purpose), sealed).ok()
    }

    fn sealed(&self, purpose: &[u8], mut plain: Vec<u8>) -> Result<String> {
        let nonce: [u8; NONCE_LEN] = crate::random_bytes("a nonce")?;
        self.first
            .sealing
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(purpose),
                &mut plain,
            )
            .map_err(|_| Error::internal_message("sealing a value failed"))?;
        let mut bytes = nonce.to_vec();
        bytes.append(&mut plain);
        Ok(base64::encode(&URL_SAFE, &bytes))
    }

    fn opened(&self, purpose: &[u8], sealed: &str) -> std::result::Result<Vec<u8>, OpenError> {
        let bytes = base64::decode(&URL_SAFE, sealed).ok_or(OpenError::Malformed)?;
        let (nonce, ciphertext) = bytes
            .split_first_chunk::<NONCE_LEN>()
            .ok_or(OpenError::Malformed)?;
        self.all()
            .find_map(|key| open_with(&key.sealing, *nonce, purpose, ciphertext))
            .ok_or(OpenError::Unauthenticated)
    }
}

/// The keys under their name, else under the old one; both set apart is refused,
/// as which one signs would otherwise depend on the reader.
fn keys_text(lookup: impl Fn(&str) -> Result<Option<String>>) -> Result<Option<String>> {
    match (lookup(SIGNING_KEYS)?, lookup(OLD_SIGNING_KEYS)?) {
        (Some(keys), Some(old)) if keys != old => Err(keys_error(&format!(
            "and {OLD_SIGNING_KEYS} are both set and differ; keep only {SIGNING_KEYS}"
        ))),
        (Some(keys), _) | (None, Some(keys)) => Ok(Some(keys)),
        (None, None) => Ok(None),
    }
}

fn value_purpose(purpose: &str) -> Vec<u8> {
    [VALUE_PURPOSE, purpose.as_bytes()].concat()
}

impl fmt::Debug for SigningKeys {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SigningKeys(…)")
    }
}

/// Opens a copy, since a failed open garbles its buffer and the next key needs the original.
fn open_with(
    key: &LessSafeKey,
    nonce: [u8; NONCE_LEN],
    purpose: &[u8],
    ciphertext: &[u8],
) -> Option<Vec<u8>> {
    let mut plain = ciphertext.to_vec();
    let length = key
        .open_in_place(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(purpose),
            &mut plain,
        )
        .ok()?
        .len();
    plain.truncate(length);
    Some(plain)
}

/// Names the key by its position only; the key itself is a secret.
fn decode_key(index: usize, text: &str) -> Result<[u8; KEY_LENGTH]> {
    base64::decode(&STANDARD, text)
        .and_then(|bytes| <[u8; KEY_LENGTH]>::try_from(bytes).ok())
        .ok_or_else(|| {
            keys_error(&format!(
                "key {} is not the base64 of 32 bytes; generate one with `openssl rand -base64 32`",
                index + 1
            ))
        })
}

fn keys_error(problem: &str) -> Error {
    Error::internal_message(format!("{SIGNING_KEYS} {problem}"))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const HOUR: i64 = 60 * 60;

    fn at(second: i64) -> Timestamp {
        Timestamp::from_second(1_700_000_000 + second).unwrap()
    }

    pub(crate) fn keys(byte: u8) -> SigningKeys {
        SigningKeys::new(&[[byte; KEY_LENGTH]]).unwrap()
    }

    pub(crate) fn claims() -> SessionClaims {
        SessionClaims::new(
            Uuid::new_v4().unwrap(),
            "member",
            Uuid::now_v7().unwrap(),
            at(0),
        )
    }

    #[test]
    fn a_sealed_session_opens_with_its_claims_and_hides_them() {
        let keys = keys(1);
        let claims = claims();

        let sealed = claims.seal(&keys).unwrap();

        assert_eq!(
            SessionClaims::open(&keys, &sealed, at(60)),
            Ok(claims.clone())
        );
        assert!(!sealed.contains("member"));
        assert_ne!(
            claims.seal(&keys).unwrap(),
            sealed,
            "every seal has its own nonce"
        );
    }

    #[test]
    fn a_sealed_value_opens_only_for_its_own_purpose() {
        let sealing = keys(1);
        let sealed = sealing.seal_value("totp", b"secret").unwrap();

        assert_eq!(
            sealing.open_value("totp", &sealed),
            Some(b"secret".to_vec())
        );
        assert_eq!(sealing.open_value("pending", &sealed), None);
        assert_eq!(
            SessionClaims::open(&sealing, &sealed, at(0)),
            Err(OpenError::Unauthenticated)
        );
        assert_eq!(keys(2).open_value("totp", &sealed), None);
        let session = claims().seal(&sealing).unwrap();
        assert_eq!(sealing.open_value("", &session), None);
    }

    #[test]
    fn a_tampered_session_is_refused() {
        let keys = keys(1);
        let sealed = claims().seal(&keys).unwrap();
        let mut bytes = base64::decode(&URL_SAFE, &sealed).unwrap();

        for index in [0, NONCE_LEN, bytes.len() - 1] {
            bytes[index] ^= 1;
            assert_eq!(
                SessionClaims::open(&keys, &base64::encode(&URL_SAFE, &bytes), at(0)),
                Err(OpenError::Unauthenticated)
            );
            bytes[index] ^= 1;
        }
        assert_eq!(
            SessionClaims::open(&keys, "not a session", at(0)),
            Err(OpenError::Malformed)
        );
        assert_eq!(
            SessionClaims::open(&keys, "", at(0)),
            Err(OpenError::Malformed)
        );
    }

    #[test]
    fn a_session_sealed_with_another_key_is_refused() {
        let sealed = claims().seal(&keys(1)).unwrap();

        assert_eq!(
            SessionClaims::open(&keys(2), &sealed, at(0)),
            Err(OpenError::Unauthenticated)
        );
    }

    #[test]
    fn a_session_that_opens_without_claims_is_corrupt() {
        let keys = keys(1);
        let sealed = keys.sealed(PURPOSE, b"too short".to_vec()).unwrap();

        assert_eq!(
            SessionClaims::open(&keys, &sealed, at(0)),
            Err(OpenError::Corrupt)
        );
    }

    #[test]
    fn keys_rotate_by_putting_the_successor_first() {
        let old = claims().seal(&keys(1)).unwrap();
        let rotated = SigningKeys::new(&[[2; KEY_LENGTH], [1; KEY_LENGTH]]).unwrap();

        let new = claims().seal(&rotated).unwrap();

        assert!(SessionClaims::open(&rotated, &old, at(0)).is_ok());
        assert!(SessionClaims::open(&keys(2), &new, at(0)).is_ok());
        assert_eq!(
            SessionClaims::open(&keys(1), &new, at(0)),
            Err(OpenError::Unauthenticated)
        );
    }

    #[test]
    fn a_session_ends_thirty_days_after_sign_in_however_active() {
        let keys = keys(1);
        let day = 24 * HOUR;
        let active = claims().seen_at(at(30 * day - 60));
        let sealed = active.seal(&keys).unwrap();

        assert!(SessionClaims::open(&keys, &sealed, at(30 * day - 1)).is_ok());
        assert_eq!(
            SessionClaims::open(&keys, &sealed, at(30 * day)),
            Err(OpenError::Expired)
        );
        assert_eq!(active.remaining_seconds(at(29 * day)), day);
    }

    #[test]
    fn a_session_signed_back_in_keeps_its_first_sign_ins_lifetime() {
        let keys = keys(1);
        let day = 24 * HOUR;
        let resumed = SessionClaims::new(
            Uuid::new_v4().unwrap(),
            "member",
            Uuid::now_v7().unwrap(),
            at(29 * day),
        )
        .issued_at(at(0));
        let sealed = resumed.seal(&keys).unwrap();

        assert_eq!(resumed.remaining_seconds(at(29 * day)), day);
        assert_eq!(
            SessionClaims::open(&keys, &sealed, at(30 * day)),
            Err(OpenError::Expired)
        );
    }

    #[test]
    fn a_session_ends_after_twelve_idle_hours_and_a_refresh_extends_it() {
        let keys = keys(1);
        let claims = claims();
        let sealed = claims.seal(&keys).unwrap();
        let refreshed = claims.clone().seen_at(at(11 * HOUR)).seal(&keys).unwrap();

        assert!(SessionClaims::open(&keys, &sealed, at(12 * HOUR - 1)).is_ok());
        assert_eq!(
            SessionClaims::open(&keys, &sealed, at(12 * HOUR)),
            Err(OpenError::Expired)
        );
        assert!(SessionClaims::open(&keys, &refreshed, at(22 * HOUR)).is_ok());
    }

    #[test]
    fn a_refresh_is_due_after_five_minutes() {
        let claims = claims();

        assert!(!claims.needs_refresh(at(299)));
        assert!(claims.needs_refresh(at(300)));
    }

    #[test]
    fn keys_are_parsed_from_base64_and_never_quoted() {
        let key = base64::encode(&STANDARD, &[7; KEY_LENGTH]);
        assert!(SigningKeys::parse(&format!("{key}=, {key}")).is_ok());

        let short = base64::encode(&STANDARD, &[7; 16]);
        let error = SigningKeys::parse(&format!("{key},{short}")).unwrap_err();
        let message = error.to_string();
        assert!(
            message.starts_with("SOFIE_SIGNING_KEYS key 2 is not"),
            "{message}"
        );
        assert!(!message.contains(&short) && !message.contains(&key));
        assert!(SigningKeys::parse("").is_err());
        assert_eq!(format!("{:?}", keys(1)), "SigningKeys(…)");
    }

    fn read(variables: &[(&str, &str)]) -> Result<SigningKeys> {
        SigningKeys::read(|name| {
            Ok(variables
                .iter()
                .find(|(variable, _)| *variable == name)
                .map(|(_, value)| (*value).to_owned()))
        })
    }

    #[test]
    fn keys_are_read_under_the_old_name_too_unless_both_differ() {
        let key = base64::encode(&STANDARD, &[7; KEY_LENGTH]);
        let other = base64::encode(&STANDARD, &[8; KEY_LENGTH]);
        assert!(read(&[(SIGNING_KEYS, &key)]).is_ok());
        assert!(read(&[(OLD_SIGNING_KEYS, &key)]).is_ok());
        assert!(read(&[(SIGNING_KEYS, &key), (OLD_SIGNING_KEYS, &key)]).is_ok());

        let differing = read(&[(SIGNING_KEYS, &key), (OLD_SIGNING_KEYS, &other)]);
        let message = differing.unwrap_err().to_string();
        assert_eq!(
            message,
            "SOFIE_SIGNING_KEYS and SOFIE_SESSION_KEYS are both set and differ; \
             keep only SOFIE_SIGNING_KEYS"
        );
        let missing = read(&[]).unwrap_err().to_string();
        assert!(
            missing.starts_with("SOFIE_SIGNING_KEYS is not set"),
            "{missing}"
        );
    }
}
