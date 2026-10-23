use std::fmt;
use std::num::NonZeroU32;
use std::sync::{Arc, LazyLock};

use argon2::password_hash::phc::{self, Output, ParamsString, Salt};
use argon2::{
    ARGON2ID_IDENT, Algorithm, Argon2, Params, PasswordHasher, PasswordVerifier, Version,
};
use ring::pbkdf2::{self, PBKDF2_HMAC_SHA256};
use sofie::core::encoding::base64::{self, STANDARD};
use sofie::core::{Cast, ColumnReader, ColumnSpec, Error, Measured, Result, SqlValue};
use sofie::serde::{Deserialize, Deserializer};
use sofie::values::{Control, Input, Present};
use tokio::sync::Semaphore;
use unicode_normalization::UnicodeNormalization;

/// OWASP's first choice for Argon2id: 19 MiB, two passes, one lane.
const PARAMS: Params = Params::DEFAULT;
const VERSION: Version = Version::V0x13;
const SALT_LENGTH: usize = 16;
const HASH_LENGTH: usize = 32;
const PBKDF2_SCHEME: &str = "pbkdf2-sha256";
const MINIMUM_CHARACTERS: usize = 12;
const MAXIMUM_CHARACTERS: usize = 128;

/// Each Argon2id hash holds 19 MiB while it runs, so at most one per core runs at once.
static HASHING: LazyLock<Arc<Semaphore>> = LazyLock::new(|| {
    Arc::new(Semaphore::new(
        std::thread::available_parallelism().map_or(1, std::num::NonZero::get),
    ))
});

async fn limited<T: Send + 'static>(
    limit: Arc<Semaphore>,
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    let permit = limit
        .acquire_owned()
        .await
        .map_err(|error| Error::internal("waiting to hash a password", error))?;
    // A dropped request cannot stop a blocking task, so the permit lives as long as the hash.
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
    .await
    .map_err(|error| Error::internal("hashing a password", error))?
}

fn argon2id() -> Argon2<'static> {
    Argon2::new(Algorithm::Argon2id, VERSION, PARAMS)
}

/// Never printed or serialized, only hashed or checked; kept in NFC, as NIST SP 800-63B asks.
pub struct Password {
    text: String,
    /// As typed when not NFC, which a hash stored before normalization was of, to be replaced.
    typed: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasswordCheck {
    Wrong,
    Right,
    RightAsTyped,
}

impl PasswordCheck {
    #[must_use]
    pub fn is_right(self) -> bool {
        self != Self::Wrong
    }
}

impl Password {
    pub fn parse(typed: String) -> Result<Self> {
        let password = Self::entered(typed);
        let characters = password.text.chars().count();
        if characters < MINIMUM_CHARACTERS {
            return Err(Error::invalid("password", "must be at least 12 characters"));
        }
        if characters > MAXIMUM_CHARACTERS {
            return Err(Error::invalid("password", "must be at most 128 characters"));
        }
        Ok(password)
    }

    /// Without the length policy, for checking one already chosen: one chosen under an older
    /// policy, or counted before NFC, must still match.
    #[must_use]
    pub fn entered(typed: String) -> Self {
        let text: String = typed.nfc().collect();
        let typed = (typed != text).then_some(typed);
        Self { text, typed }
    }

    /// Without a hash it checks a fixed one, and a non-NFC password both ways always, so an
    /// unknown account takes as long.
    pub async fn check(&self, hash: Option<&PasswordHash>) -> Result<PasswordCheck> {
        let hash = match hash {
            Some(hash) => hash.clone(),
            None => PasswordHash::unmatchable()?,
        };
        let text = self.text.clone();
        let typed = self.typed.clone();
        limited(Arc::clone(&HASHING), move || {
            let right = hash.verify(&text)?;
            let right_as_typed = match &typed {
                Some(typed) => hash.verify(typed)?,
                None => false,
            };
            Ok(match (right, right_as_typed) {
                (true, _) => PasswordCheck::Right,
                (false, true) => PasswordCheck::RightAsTyped,
                (false, false) => PasswordCheck::Wrong,
            })
        })
        .await
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Password(…)")
    }
}

impl Present for Password {
    const INPUT: Input = Input::new(Control::Password);
    const SEARCH: sofie::values::Search = sofie::values::Search::Skipped;

    fn csv(&self) -> String {
        "***".to_owned()
    }
}

/// So an input may tighten the length a password needs with `#[min]` and `#[max]`.
impl Measured for Password {
    const TEXT: bool = true;

    fn size(&self) -> Option<i64> {
        self.text.size()
    }

    fn is_blank(&self) -> bool {
        self.text.is_blank()
    }
}

impl<'de> Deserialize<'de> for Password {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(text).map_err(Error::into_deserialize_error)
    }
}

/// Written as Argon2id in the PHC string format, `$argon2id$v=19$m=19456,t=2,p=1$<salt>$<hash>`.
/// `pbkdf2-sha256$<iterations>$<salt>$<hash>`, written before, is still read and checked.
#[derive(Clone)]
pub struct PasswordHash {
    scheme: Scheme,
}

#[derive(Clone)]
enum Scheme {
    /// Boxed, as it is ten times the size of the other.
    Argon2id(Box<phc::PasswordHash>),
    Pbkdf2 {
        iterations: NonZeroU32,
        salt: [u8; SALT_LENGTH],
        hash: [u8; HASH_LENGTH],
    },
}

impl PasswordHash {
    pub async fn new(password: &Password) -> Result<Self> {
        let mut salt = [0; SALT_LENGTH];
        sofie::core::random::fill(&mut salt)
            .map_err(|error| Error::internal("drawing a password's salt", error))?;
        let text = password.text.clone();
        limited(Arc::clone(&HASHING), move || {
            let hash = argon2id()
                .hash_password_with_salt(text.as_bytes(), &salt)
                .map_err(|error| Error::internal("hashing a password with Argon2id", error))?;
            Ok(Self {
                scheme: Scheme::Argon2id(Box::new(hash)),
            })
        })
        .await
    }

    /// Never matches: no password derives to all zero bytes. Checking it costs what checking
    /// a new hash does.
    pub(crate) fn unmatchable() -> Result<Self> {
        let malformed = |error| Error::internal("building the unmatchable password hash", error);
        let hash = phc::PasswordHash {
            algorithm: ARGON2ID_IDENT,
            version: Some(VERSION.into()),
            params: ParamsString::try_from(&PARAMS).map_err(malformed)?,
            salt: Some(Salt::new(&[0; SALT_LENGTH]).map_err(|error| malformed(error.into()))?),
            hash: Some(Output::new(&[0; HASH_LENGTH]).map_err(|error| malformed(error.into()))?),
        };
        Ok(Self {
            scheme: Scheme::Argon2id(Box::new(hash)),
        })
    }

    /// Whether it was written by an older scheme or weaker parameters, so a sign-in that proved
    /// its password should store a new one.
    #[must_use]
    pub fn needs_rehash(&self) -> bool {
        match &self.scheme {
            Scheme::Argon2id(hash) => {
                Params::try_from(&hash.params).map_or(true, |params| !is_current(&params))
            }
            Scheme::Pbkdf2 { .. } => true,
        }
    }

    /// Compared in constant time.
    fn verify(&self, password: &str) -> Result<bool> {
        match &self.scheme {
            Scheme::Argon2id(hash) => {
                match argon2id().verify_password(password.as_bytes(), hash.as_ref()) {
                    Ok(()) => Ok(true),
                    Err(argon2::password_hash::Error::PasswordInvalid) => Ok(false),
                    Err(error) => Err(Error::internal("checking a password with Argon2id", error)),
                }
            }
            Scheme::Pbkdf2 {
                iterations,
                salt,
                hash,
            } => Ok(pbkdf2::verify(
                PBKDF2_HMAC_SHA256,
                *iterations,
                salt,
                password.as_bytes(),
                hash,
            )
            .is_ok()),
        }
    }

    fn to_text(&self) -> String {
        match &self.scheme {
            Scheme::Argon2id(hash) => hash.to_string(),
            Scheme::Pbkdf2 {
                iterations,
                salt,
                hash,
            } => format!(
                "{PBKDF2_SCHEME}${iterations}${}${}",
                base64::encode(&STANDARD, salt),
                base64::encode(&STANDARD, hash)
            ),
        }
    }
}

fn is_current(params: &Params) -> bool {
    params.m_cost() >= PARAMS.m_cost()
        && params.t_cost() >= PARAMS.t_cost()
        && params.p_cost() >= PARAMS.p_cost()
}

/// Checked whole here, so checking a password later fails only on a wrong password.
fn argon2id_from_text(text: &str) -> Option<Scheme> {
    let hash = phc::PasswordHash::new(text).ok()?;
    if hash.algorithm != ARGON2ID_IDENT || hash.version != Some(VERSION.into()) {
        return None;
    }
    Params::try_from(&hash).ok()?;
    hash.salt?;
    hash.hash?;
    Some(Scheme::Argon2id(Box::new(hash)))
}

fn pbkdf2_from_text(text: &str) -> Option<Scheme> {
    let parts: Vec<&str> = text.split('$').collect();
    let [PBKDF2_SCHEME, iterations, salt, hash] = parts.as_slice() else {
        return None;
    };
    Some(Scheme::Pbkdf2 {
        iterations: iterations.parse().ok()?,
        salt: base64::decode(&STANDARD, salt)?.try_into().ok()?,
        hash: base64::decode(&STANDARD, hash)?.try_into().ok()?,
    })
}

impl fmt::Debug for PasswordHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PasswordHash(…)")
    }
}

impl Cast for PasswordHash {
    const COLUMNS: &'static [ColumnSpec] = String::COLUMNS;
    const SENSITIVE: bool = true;

    fn encode(&self, out: &mut Vec<SqlValue>) {
        out.push(SqlValue::Text(self.to_text()));
    }

    fn decode(row: &mut dyn ColumnReader) -> Result<Self> {
        let text = row.text()?;
        let scheme = if text.starts_with('$') {
            argon2id_from_text(text)
        } else {
            pbkdf2_from_text(text)
        };
        scheme.map(|scheme| Self { scheme }).ok_or_else(|| {
            Error::internal_message(
                "a stored password hash is neither Argon2id in the PHC string format nor pbkdf2-sha256",
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use sofie::core::RowReader;

    use super::*;

    const ARGON2ID_PREFIX: &str = "$argon2id$v=19$m=19456,t=2,p=1$";

    #[tokio::test]
    async fn a_dropped_request_keeps_its_hashing_permit_until_the_hash_ends() {
        let limit = Arc::new(Semaphore::new(1));
        let (started, has_started) = std::sync::mpsc::channel();
        let (finish, may_finish) = std::sync::mpsc::channel::<()>();
        let request = tokio::spawn(limited(Arc::clone(&limit), move || {
            started.send(()).unwrap();
            may_finish.recv().unwrap();
            Ok(())
        }));
        tokio::task::spawn_blocking(move || has_started.recv().unwrap())
            .await
            .unwrap();

        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        assert_eq!(limit.available_permits(), 0);

        finish.send(()).unwrap();
        drop(limit.acquire().await.unwrap());
    }

    fn password(text: &str) -> Password {
        Password::parse(text.to_owned()).unwrap()
    }

    fn stored(text: &str) -> Result<PasswordHash> {
        let values = [SqlValue::Text(text.to_owned())];
        PasswordHash::decode(&mut RowReader::new(&values))
    }

    fn column(hash: &PasswordHash) -> String {
        let mut column = Vec::new();
        hash.encode(&mut column);
        let [SqlValue::Text(text)] = column.as_slice() else {
            panic!("one text column");
        };
        text.clone()
    }

    fn pbkdf2_text(password: &str) -> String {
        let salt = [7; SALT_LENGTH];
        let mut hash = [0; HASH_LENGTH];
        let iterations = NonZeroU32::new(600_000).unwrap();
        pbkdf2::derive(
            PBKDF2_HMAC_SHA256,
            iterations,
            &salt,
            password.as_bytes(),
            &mut hash,
        );
        format!(
            "pbkdf2-sha256${iterations}${}${}",
            base64::encode(&STANDARD, &salt),
            base64::encode(&STANDARD, &hash)
        )
    }

    #[test]
    fn a_password_is_measured_in_characters_like_text() {
        let measured = password("çorrect horse battery");

        assert_eq!(measured.size(), Some(21));
        assert!(!measured.is_blank());
        assert!(password(&" ".repeat(12)).is_blank());
    }

    #[test]
    fn new_hashes_take_owasp_s_argon2id_parameters() {
        assert_eq!(
            (PARAMS.m_cost(), PARAMS.t_cost(), PARAMS.p_cost()),
            (19 * 1024, 2, 1)
        );
    }

    #[tokio::test]
    async fn a_new_hash_is_argon2id_and_matches_its_password_and_no_other() {
        let hash = PasswordHash::new(&password("correct horse battery"))
            .await
            .unwrap();

        assert!(
            column(&hash).starts_with(ARGON2ID_PREFIX),
            "{}",
            column(&hash)
        );
        assert!(!hash.needs_rehash());
        assert!(
            password("correct horse battery")
                .check(Some(&hash))
                .await
                .unwrap()
                .is_right()
        );
        assert!(
            !password("correct horse batterY")
                .check(Some(&hash))
                .await
                .unwrap()
                .is_right()
        );
    }

    #[tokio::test]
    async fn an_old_pbkdf2_hash_still_matches_only_its_password_and_asks_for_a_rehash() {
        let text = pbkdf2_text("correct horse battery");
        let old = stored(&text).unwrap();

        assert!(old.needs_rehash());
        assert_eq!(column(&old), text, "read back as it was written");
        assert!(
            password("correct horse battery")
                .check(Some(&old))
                .await
                .unwrap()
                .is_right()
        );
        assert!(
            !password("correct horse batterY")
                .check(Some(&old))
                .await
                .unwrap()
                .is_right()
        );
    }

    #[test]
    fn an_argon2id_hash_weaker_than_today_s_asks_for_a_rehash() {
        let weaker = stored(&format!(
            "$argon2id$v=19$m=8192,t=2,p=1${}${}",
            "AAAAAAAAAAAAAAAAAAAAAA",
            "A".repeat(43)
        ))
        .unwrap();

        assert!(weaker.needs_rehash());
    }

    #[tokio::test]
    async fn without_a_hash_nothing_matches_at_the_cost_of_a_new_hash() {
        let unmatchable = PasswordHash::unmatchable().unwrap();

        assert!(column(&unmatchable).starts_with(ARGON2ID_PREFIX));
        assert!(!unmatchable.needs_rehash());
        assert!(
            !password("correct horse battery")
                .check(None)
                .await
                .unwrap()
                .is_right()
        );
    }

    #[tokio::test]
    async fn a_hash_survives_its_column_and_never_prints() {
        let hash = PasswordHash::new(&password("correct horse battery"))
            .await
            .unwrap();

        let decoded = stored(&column(&hash)).unwrap();

        assert!(
            password("correct horse battery")
                .check(Some(&decoded))
                .await
                .unwrap()
                .is_right()
        );
        assert_eq!(format!("{decoded:?}"), "PasswordHash(…)");
    }

    #[test]
    fn a_malformed_stored_hash_is_refused_without_quoting_it() {
        for text in [
            "pbkdf2-sha256$600000$c2FsdA",
            "md5$1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "pbkdf2-sha256$0$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "$argon2i$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "$argon2id$v=16$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA",
            "$argon2id$v=19$m=1,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        ] {
            let error = stored(text).unwrap_err();
            assert!(!error.to_string().contains(text), "{error}");
        }
    }

    #[test]
    fn a_password_has_twelve_to_128_characters_counted_not_bytes() {
        assert!(Password::parse("ü".repeat(11)).is_err());
        assert!(Password::parse("ü".repeat(12)).is_ok());
        assert!(Password::parse("ü".repeat(128)).is_ok());
        assert!(Password::parse("a".repeat(129)).is_err());
    }

    #[test]
    fn a_password_is_read_from_input_and_never_printed() {
        let read: Password = sofie::serde_json::from_str("\"correct horse battery\"").unwrap();
        assert_eq!(format!("{read:?}"), "Password(…)");

        let error = sofie::serde_json::from_str::<Password>("\"too short\"").unwrap_err();
        assert!(!error.to_string().contains("too short"), "{error}");
        assert!(error.to_string().contains("at least 12"), "{error}");
    }

    #[tokio::test]
    async fn a_password_typed_decomposed_matches_its_composed_hash() {
        let hash = PasswordHash::new(&password("çorrect horse battery"))
            .await
            .unwrap();

        let check = password("c\u{327}orrect horse battery")
            .check(Some(&hash))
            .await
            .unwrap();

        assert_eq!(check, PasswordCheck::Right);
    }

    #[tokio::test]
    async fn a_hash_of_a_decomposed_password_still_matches_and_asks_to_be_replaced() {
        let decomposed = "c\u{327}orrect horse battery";
        let before_normalization = stored(&pbkdf2_text(decomposed)).unwrap();

        let check = password(decomposed)
            .check(Some(&before_normalization))
            .await
            .unwrap();
        let other = password("c\u{327}orrect horse batterY")
            .check(Some(&before_normalization))
            .await
            .unwrap();

        assert_eq!(check, PasswordCheck::RightAsTyped);
        assert_eq!(other, PasswordCheck::Wrong);
    }

    #[test]
    fn a_password_is_counted_after_nfc() {
        assert!(Password::parse("u\u{308}".repeat(12)).is_ok());
        assert!(Password::parse("u\u{308}".repeat(11)).is_err());
        assert!(Password::parse("u\u{308}".repeat(128)).is_ok());
    }
}
