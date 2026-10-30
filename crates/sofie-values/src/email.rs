use std::fmt;

use sofie_core::{Error, Result};
use unicode_normalization::{UnicodeNormalization, is_nfc};

use crate::slug_character::is_word_mark;

const MAXIMUM_LENGTH: usize = 254;
const MAXIMUM_LOCAL_LENGTH: usize = 64;
const MAXIMUM_LABEL_LENGTH: usize = 63;

/// `Debug` masks it so it stays out of logs.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Email {
    address: String,
}

impl Email {
    pub fn parse(input: &str) -> Result<Self> {
        let trimmed = input.trim();
        // RFC 5321 counts octets; refused before the lowercase copy.
        if trimmed.len() > MAXIMUM_LENGTH {
            return Err(invalid_email("must be at most 254 bytes"));
        }
        // NFC, so `josé` typed composed or decomposed is one address and one account; `İ` folds
        // to `i` rather than `i` plus a dot above, so `İlker` and `ilker` are one account too.
        let lowercase = trimmed.replace('\u{130}', "i").to_lowercase();
        Self::from_normalized(lowercase.nfc().collect())
    }

    fn from_stored(text: String) -> Result<Self> {
        let normalized = text.trim().len() == text.len()
            && text.chars().flat_map(char::to_lowercase).eq(text.chars())
            && is_nfc(&text);
        if normalized {
            return Self::from_normalized(text);
        }
        Self::parse(&text)
    }

    fn from_normalized(address: String) -> Result<Self> {
        if address.len() > MAXIMUM_LENGTH {
            return Err(invalid_email("must be at most 254 bytes"));
        }
        if address.chars().any(char::is_whitespace) {
            return Err(invalid_email("must not contain whitespace"));
        }
        if address.chars().any(is_invisible) {
            return Err(invalid_email(
                "must not contain control or invisible formatting characters",
            ));
        }
        let (local, domain) = address
            .split_once('@')
            .ok_or_else(|| invalid_email("must contain an @"))?;
        validate_parts(local, domain)?;
        Ok(Self { address })
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.address
    }
}

/// ZWJ and ZWNJ, kept in names, are refused too: an address is matched byte for byte, so one
/// that looks the same would be another account.
fn is_invisible(character: char) -> bool {
    sofie_core::misleads_display(character) || matches!(character, '\u{200C}' | '\u{200D}')
}

fn validate_parts(local: &str, domain: &str) -> Result<()> {
    if domain.contains('@') {
        return Err(invalid_email("must contain a single @"));
    }
    if local.is_empty() {
        return Err(invalid_email("must have a part before the @"));
    }
    if local.len() > MAXIMUM_LOCAL_LENGTH {
        return Err(invalid_email("must have at most 64 bytes before the @"));
    }
    if !local.chars().all(is_local_character) {
        return Err(invalid_email(
            "must use only letters, digits and !#$%&'*+-/=?^_`{|}~ before the @",
        ));
    }
    if !marks_follow_letters(local) {
        return Err(invalid_email(
            "must put combining marks only after a letter before the @",
        ));
    }
    if local.split('.').any(str::is_empty) {
        return Err(invalid_email(
            "must not start or end the part before the @ with a dot, or repeat a dot in it",
        ));
    }
    if !domain.contains('.') {
        return Err(invalid_email("must have a domain with a dot after the @"));
    }
    if domain.split('.').any(str::is_empty) {
        return Err(invalid_email(
            "must not start or end the domain with a dot, or repeat a dot in it",
        ));
    }
    if domain
        .split('.')
        .any(|label| label.len() > MAXIMUM_LABEL_LENGTH)
    {
        return Err(invalid_email(
            "must have at most 63 bytes between the dots of the domain",
        ));
    }
    if !marks_follow_letters(domain) {
        return Err(invalid_email(
            "must put combining marks only after a letter in the domain",
        ));
    }
    if !domain.split('.').all(is_domain_label) {
        return Err(invalid_email(
            "must use only letters, digits and inner hyphens between the dots of the domain",
        ));
    }
    Ok(())
}

/// RFC 5322 `atext` and dots; non-ASCII letters as RFC 6531 allows. Quoted local parts are
/// refused: no mail provider hands them out.
fn is_local_character(character: char) -> bool {
    is_word_character(character) || "!#$%&'*+-/=?^_`{|}~.".contains(character)
}

/// Marks such as the dot lowercasing leaves on `i`, a Hindi virama or a Thai tone mark.
fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || is_word_mark(character)
}

fn marks_follow_letters(part: &str) -> bool {
    let mut follows_letter = false;
    part.chars().all(|character| {
        let allowed = follows_letter || !is_word_mark(character);
        follows_letter = is_word_character(character);
        allowed
    })
}

/// A host name label; non-ASCII letters are an internationalized domain before punycode.
fn is_domain_label(label: &str) -> bool {
    !label.starts_with('-')
        && !label.ends_with('-')
        && label
            .chars()
            .all(|character| is_word_character(character) || character == '-')
}

fn invalid_email(message: &str) -> Error {
    Error::invalid("email", message)
}

impl fmt::Display for Email {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.address)
    }
}

impl fmt::Debug for Email {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (local, domain) = self
            .address
            .split_once('@')
            .expect("a parsed email contains an @");
        let first = local.chars().next().unwrap_or_default();
        write!(formatter, "{first}***@{domain}")
    }
}

sofie_core::text_value!(Email, address, Email::from_stored);

#[cfg(test)]
mod tests {
    use sofie_core::{Cast, RowReader, SqlValue};

    use super::*;
    use crate::testing::{assert_refused, round_trip};

    #[test]
    fn accepts_and_normalizes_valid_addresses() {
        let samples = [
            ("jane@example.com", "jane@example.com"),
            ("  Jane.Doe@Example.COM \n", "jane.doe@example.com"),
            ("a+tag@mail.example.org", "a+tag@mail.example.org"),
            (
                "o'brien_{x}@sub-domain.example.com",
                "o'brien_{x}@sub-domain.example.com",
            ),
            ("Jürgen@München.de", "jürgen@münchen.de"),
            ("jose\u{301}@example.com", "jos\u{E9}@example.com"),
            ("İlker@gmail.com", "ilker@gmail.com"),
            ("i\u{307}lker@gmail.com", "i\u{307}lker@gmail.com"),
            ("नमस्ते@example.com", "नमस्ते@example.com"),
            ("ผู้ใช้@example.com", "ผู้ใช้@example.com"),
            ("jane@हिन्दी.com", "jane@हिन्दी.com"),
        ];
        for (input, normalized) in samples {
            assert_eq!(Email::parse(input).unwrap().as_str(), normalized);
        }
    }

    #[test]
    fn stored_text_is_checked_and_normalized_like_input() {
        use sofie_core::value::Value;
        let stored = |text: &str| Email::try_from_inner(text.to_owned()).map(|email| email.address);

        assert_eq!(
            stored("jane@example.com").ok().as_deref(),
            Some("jane@example.com")
        );
        assert_eq!(
            stored(" Jane@Example.COM").ok().as_deref(),
            Some("jane@example.com")
        );
        assert_eq!(
            stored("jose\u{301}@example.com").ok().as_deref(),
            Some("jos\u{E9}@example.com")
        );
        assert_eq!(
            stored("İlker@gmail.com").ok().as_deref(),
            Some("ilker@gmail.com")
        );
        assert!(stored("jane").is_err());
    }

    #[test]
    fn refuses_a_combining_mark_that_follows_no_letter() {
        let samples = [
            "\u{307}jane@example.com",
            "jane.\u{301}doe@example.com",
            "jane@\u{94D}example.com",
            "jane@exa-\u{94D}mple.com",
        ];
        assert_refused(Email::parse, &samples);
    }

    #[test]
    fn rejects_invalid_addresses() {
        let too_long = format!("jane@{}com", "a.".repeat(125));
        let too_many_bytes = format!("jane@{}com", "é.".repeat(83));
        let long_local = format!("{}@example.com", "a".repeat(65));
        let samples = [
            "",
            "jane.example.com",
            "jane@doe@example.com",
            "@example.com",
            "jane@",
            "jane@localhost",
            "jane doe@example.com",
            "jane@exa\tmple.com",
            too_long.as_str(),
            too_many_bytes.as_str(),
            "jane\u{202E}moc.elpmaxe@example.com",
            "jane\u{0}@example.com",
            "ja\u{00AD}ne@example.com",
            "ja\u{200D}ne@example.com",
            "ja\u{200C}ne@example.com",
            "jane@exam\u{E0067}ple.com",
            long_local.as_str(),
            ".jane@example.com",
            "jane.@example.com",
            "ja..ne@example.com",
            "jane@.example.com",
            "jane@example..com",
            "jane@example.com.",
            "jane@exa<mple>.com",
            "a@b.com,c.d",
            "a@b.c;d",
            "x@-evil-.com",
            "x@evil-.com",
            "jane@exa_mple.com",
            "ja<ne>@example.com",
            "\"jane\"@example.com",
            "ja,ne@example.com",
            "ja\\ne@example.com",
        ];
        assert_refused(Email::parse, &samples);
        let longest_local = format!("{}@example.com", "a".repeat(64));
        assert!(Email::parse(&longest_local).is_ok());
    }

    #[test]
    fn refuses_a_domain_label_over_63_bytes() {
        let longest = format!("jane@{}.com", "a".repeat(63));
        let too_long = format!("jane@{}.com", "a".repeat(64));

        assert!(Email::parse(&longest).is_ok());
        assert!(matches!(Email::parse(&too_long), Err(Error::Invalid(_))));
    }

    #[test]
    fn debug_masks_the_local_part() {
        let email = Email::parse("jane@example.com").unwrap();

        assert_eq!(format!("{email:?}"), "j***@example.com");
        assert_eq!(email.to_string(), "jane@example.com");
    }

    #[test]
    fn round_trips_through_one_text_column() {
        let email = Email::parse("jane@example.com").unwrap();
        assert_eq!(
            round_trip(&email),
            vec![SqlValue::Text("jane@example.com".into())]
        );
    }

    #[test]
    fn an_invalid_stored_address_is_an_internal_error() {
        let values = [SqlValue::Text("jane".into())];
        let mut row = RowReader::new(&values);

        assert!(matches!(
            Email::decode(&mut row),
            Err(Error::Internal { .. })
        ));
    }

    #[test]
    fn round_trips_through_json_as_a_string() {
        let email = Email::parse("jane@example.com").unwrap();
        let json = serde_json::to_string(&email).unwrap();

        assert_eq!(json, "\"jane@example.com\"");
        assert_eq!(serde_json::from_str::<Email>(&json).unwrap(), email);
    }

    #[test]
    fn json_deserialization_validates_and_normalizes() {
        let email: Email = serde_json::from_str("\"Jane@Example.com\"").unwrap();

        assert_eq!(email.as_str(), "jane@example.com");
        assert!(serde_json::from_str::<Email>("\"jane\"").is_err());
    }
}
