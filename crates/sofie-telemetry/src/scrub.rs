//! What has no type to mask by: fields named like credentials, and values carried in free text.

mod personal;

use std::ops::{Deref, Range};
use std::sync::OnceLock;

use sofie_core::encoding::base64;

use crate::fields::{FieldValue, Fields};
use crate::line::PendingLine;

pub(crate) const MASK: &str = "***";

/// Matched against the end of a key's last dotted part, ignoring case, `-` and `_`.
const SENSITIVE_KEYS: [&str; 25] = [
    "password",
    "passwords",
    "passwd",
    "passphrase",
    "secret",
    "secrets",
    "secret_key",
    "access_key",
    "signing_key",
    "encryption_key",
    "client_key",
    "session_keys",
    "signing_keys",
    "token",
    "tokens",
    "authorization",
    "cookie",
    "api_key",
    "private_key",
    "credentials",
    "signature",
    "session",
    "card_number",
    "cvc",
    "cvv",
];

/// Shorter runs after the word are prose ("refused a bearer token"), not credentials.
const SHORTEST_TOKEN: usize = 16;

type Rule = fn(&str) -> Vec<Range<usize>>;

const RULES: [Rule; 7] = [
    url_passwords,
    authorization_tokens,
    sensitive_pairs,
    query_values,
    row_values,
    personal::email_addresses,
    personal::ip_addresses,
];

/// A span's fields, scrubbed once on the writer's thread however many lines it is in.
#[derive(Clone, Debug, Default)]
pub(crate) struct SpanFields {
    raw: Fields,
    scrubbed: OnceLock<Option<Fields>>,
}

impl SpanFields {
    pub(crate) fn new(raw: Fields) -> Self {
        Self {
            raw,
            scrubbed: OnceLock::new(),
        }
    }

    pub(crate) fn merge(&mut self, recorded: Fields) {
        self.raw.merge(recorded);
        self.scrubbed = OnceLock::new();
    }

    pub(crate) fn scrubbed(&self) -> &Fields {
        self.scrubbed
            .get_or_init(|| scrubbed_copy(&self.raw))
            .as_ref()
            .unwrap_or(&self.raw)
    }
}

impl Deref for SpanFields {
    type Target = Fields;

    fn deref(&self) -> &Fields {
        &self.raw
    }
}

/// Spans are scrubbed as they render, through [`SpanFields::scrubbed`].
pub(crate) fn scrub(line: &mut PendingLine) {
    for (name, value) in line.fields.iter_mut() {
        if let Some(clean) = scrubbed(name, value) {
            *value = clean;
        }
    }
}

fn scrubbed_copy(fields: &Fields) -> Option<Fields> {
    let mut copy: Option<Fields> = None;
    for (name, value) in fields.iter() {
        if let Some(clean) = scrubbed(name, value) {
            copy.get_or_insert_with(|| fields.clone()).set(name, clean);
        }
    }
    copy
}

fn scrubbed(name: &str, value: &FieldValue) -> Option<FieldValue> {
    if is_sensitive_key(name) {
        let mask = FieldValue::Text(MASK.to_owned());
        return (*value != mask).then_some(mask);
    }
    match value {
        FieldValue::Text(text) => scrubbed_text(text).map(FieldValue::Text),
        FieldValue::Formatted(text) => scrubbed_text(text).map(FieldValue::Formatted),
        FieldValue::Composed(text) => scrubbed_text(text).map(FieldValue::Composed),
        _ => None,
    }
}

pub(crate) fn is_sensitive_key(key: &str) -> bool {
    let last = key.rsplit('.').next().unwrap_or(key);
    SENSITIVE_KEYS
        .iter()
        .any(|word| ends_with_word(last.as_bytes(), word.as_bytes()))
}

fn ends_with_word(key: &[u8], word: &[u8]) -> bool {
    let is_letter = |byte: &&u8| !matches!(byte, b'-' | b'_');
    let mut key = key.iter().rev().filter(is_letter);
    word.iter().rev().filter(is_letter).all(|expected| {
        key.next()
            .is_some_and(|letter| letter.eq_ignore_ascii_case(expected))
    })
}

pub(crate) fn scrubbed_text(text: &str) -> Option<String> {
    let mut scrubbed: Option<String> = None;
    for rule in RULES {
        let current = scrubbed.as_deref().unwrap_or(text);
        let ranges = rule(current);
        if !ranges.is_empty() {
            scrubbed = Some(masked(current, &ranges));
        }
    }
    scrubbed
}

/// Every rule's ranges start and end on char boundaries.
fn masked(text: &str, ranges: &[Range<usize>]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    for range in ranges {
        if range.end <= copied {
            continue;
        }
        out.push_str(&text[copied..range.start.max(copied)]);
        out.push_str(MASK);
        copied = range.end;
    }
    out.push_str(&text[copied..]);
    out
}

/// `postgres://user:password@host`, and all of `https://token@host`, which holds no name.
fn url_passwords(text: &str) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    let mut from = 0;
    while let Some(found) = text[from..].find("://") {
        let start = from + found + 3;
        let authority_end = run_until(bytes, start, |byte| {
            matches!(byte, b'/' | b'?' | b'#') || byte.is_ascii_whitespace()
        });
        // The last `@` ends the credentials: a password may hold one unescaped.
        if let Some(at) = text[start..authority_end].rfind('@').filter(|at| *at > 0) {
            let end = start + at;
            match text[start..end].find(':') {
                Some(colon) if start + colon + 1 < end => ranges.push(start + colon + 1..end),
                Some(_) => {}
                None => ranges.push(start..end),
            }
        }
        from = authority_end;
    }
    ranges
}

/// `Bearer eyJhbGciOi…`, `Basic dXNlcjpwYXNz…`; a short Basic run only if it decodes to `a:b`.
fn authorization_tokens(text: &str) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    for scheme in [&b"bearer "[..], b"basic "] {
        for start in 0..bytes.len().saturating_sub(scheme.len()) {
            let starts_word = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
            if !starts_word || !bytes[start..start + scheme.len()].eq_ignore_ascii_case(scheme) {
                continue;
            }
            let token = start + scheme.len();
            let end = run_until(bytes, token, |byte| {
                !(byte.is_ascii_alphanumeric() || b"-._~+/=".contains(&byte))
            });
            let basic = scheme == b"basic " && is_basic_credential(&text[token..end]);
            if end - token >= SHORTEST_TOKEN || basic {
                ranges.push(token..end);
            }
        }
    }
    ranges.sort_by_key(|range| range.start);
    ranges
}

fn is_basic_credential(token: &str) -> bool {
    base64::decode(&base64::STANDARD, token).is_some_and(|decoded| decoded.contains(&b':'))
}

/// `password=…`, `"api_key": "…"`, `token: …`; an unquoted cookie header to its last `name=value`.
fn sensitive_pairs(text: &str) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let separator = at;
        at += 1;
        if !matches!(bytes[separator], b'=' | b':') || bytes[separator..].starts_with(b"://") {
            continue;
        }
        let Some(key) = key_before(bytes, separator) else {
            continue;
        };
        if !is_sensitive_key(&text[key.clone()]) {
            continue;
        }
        let key = &text[key];
        let mut value = if is_authorization_key(key) {
            authorization_value(bytes, separator + 1)
        } else {
            value_after(bytes, separator + 1)
        };
        if is_cookie_key(key) {
            value.end = cookies_end(bytes, value.end);
        }
        if !value.is_empty() {
            at = value.end;
            ranges.push(value);
        }
    }
    ranges
}

fn is_authorization_key(key: &str) -> bool {
    let last = key.rsplit('.').next().unwrap_or(key);
    ends_with_word(last.as_bytes(), b"authorization")
}

/// A scheme such as `Token` or `Digest` may come before the credential or several parts follow
/// it, so an unquoted value runs to the line's end.
fn authorization_value(bytes: &[u8], from: usize) -> Range<usize> {
    let start = run_until(bytes, from, |byte| byte != b' ');
    if matches!(bytes.get(start), Some(b'"' | b'\'' | b'\\' | b'{' | b'[')) {
        return value_after(bytes, from);
    }
    start..run_until(bytes, start, |byte| matches!(byte, b'\n' | b'\r'))
}

fn is_cookie_key(key: &str) -> bool {
    let key = key.trim_end_matches(['"', '\'', '\\']);
    key.len() >= "cookie".len() && key[key.len() - "cookie".len()..].eq_ignore_ascii_case("cookie")
}

/// `a=1; sid=…; b=2`: each pair after a `;` belongs to the same header.
fn cookies_end(bytes: &[u8], mut end: usize) -> usize {
    while bytes.get(end) == Some(&b';') {
        let next = run_until(bytes, end + 1, |byte| byte != b' ');
        let pair_end = run_until(bytes, next, |byte| {
            byte.is_ascii_whitespace() || b"&,;})]\"'\\".contains(&byte)
        });
        if pair_end == next || !bytes[next..pair_end].contains(&b'=') {
            return end;
        }
        end = pair_end;
    }
    end
}

fn key_before(bytes: &[u8], separator: usize) -> Option<Range<usize>> {
    let mut end = separator;
    while end > 0 && bytes[end - 1] == b' ' {
        end -= 1;
    }
    if end > 0 && matches!(bytes[end - 1], b'"' | b'\'') {
        end -= 1;
    }
    if end > 0 && bytes[end - 1] == b'\\' {
        end -= 1;
    }
    let start = bytes[..end]
        .iter()
        .rposition(|byte| !(byte.is_ascii_alphanumeric() || b"_-.".contains(byte)))
        .map_or(0, |before| before + 1);
    (start < end).then_some(start..end)
}

/// A quoted value runs to its closing quote; JSON inside a `Debug` string quotes as `\"`.
fn value_after(bytes: &[u8], from: usize) -> Range<usize> {
    let start = run_until(bytes, from, |byte| byte != b' ');
    if matches!(bytes.get(start), Some(b'{' | b'[')) {
        return start..bracketed_end(bytes, start);
    }
    let escaped = bytes.get(start) == Some(&b'\\');
    let quote_at = start + usize::from(escaped);
    if let Some(&quote @ (b'"' | b'\'')) = bytes.get(quote_at) {
        let inside = quote_at + 1;
        return inside..closing_quote(bytes, inside, quote, escaped);
    }
    start..run_until(bytes, start, |byte| {
        byte.is_ascii_whitespace() || b"&,;})]\"'\\".contains(&byte)
    })
}

fn bracketed_end(bytes: &[u8], from: usize) -> usize {
    let mut depth = 0_usize;
    for (offset, byte) in bytes[from..].iter().enumerate() {
        match byte {
            b'{' | b'[' => depth += 1,
            b'}' | b']' if depth == 1 => return from + offset + 1,
            b'}' | b']' => depth -= 1,
            _ => {}
        }
    }
    bytes.len()
}

fn closing_quote(bytes: &[u8], from: usize, quote: u8, escaped: bool) -> usize {
    let mut at = from;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' if escaped && bytes.get(at + 1) == Some(&quote) => return at,
            b'\\' => at += 2,
            byte if byte == quote && !escaped => return at,
            _ => at += 1,
        }
    }
    bytes.len()
}

/// `/orders?email=…&page=…`: every value, since a URL's query carries what was typed.
fn query_values(text: &str) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    let mut from = 0;
    while let Some(found) = text[from..].find('?') {
        let query = from + found + 1;
        let end = run_until(bytes, query, |byte| {
            byte.is_ascii_whitespace() || b"#\"'<>`".contains(&byte)
        });
        let mut pair = query;
        while pair < end {
            let pair_end = run_until(&bytes[..end], pair, |byte| byte == b'&');
            if let Some(equals) = text[pair..pair_end].find('=')
                && pair + equals + 1 < pair_end
            {
                ranges.push(pair + equals + 1..pair_end);
            }
            pair = pair_end + 1;
        }
        from = end.max(query);
    }
    ranges
}

/// Postgres details: `Key (email)=(…) already exists`, `Failing row contains (…)`.
fn row_values(text: &str) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    for marker in [")=(", "row contains ("] {
        let mut from = 0;
        while let Some(found) = text[from..].find(marker) {
            let start = from + found + marker.len();
            let end = balanced_end(bytes, start);
            if start < end {
                ranges.push(start..end);
            }
            from = end;
        }
    }
    ranges.sort_by_key(|range| range.start);
    ranges
}

fn balanced_end(bytes: &[u8], from: usize) -> usize {
    let mut depth = 1_usize;
    for (offset, byte) in bytes[from..].iter().enumerate() {
        match byte {
            b'(' => depth += 1,
            b')' if depth == 1 => return from + offset,
            b')' => depth -= 1,
            _ => {}
        }
    }
    bytes.len()
}

fn run_until(bytes: &[u8], from: usize, stop: impl Fn(u8) -> bool) -> usize {
    bytes
        .get(from..)
        .and_then(|rest| rest.iter().position(|byte| stop(*byte)))
        .map_or(bytes.len(), |offset| from + offset)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::SystemTime;

    use tracing::Level;

    use super::{SpanFields, is_sensitive_key, scrub, scrubbed_text};
    use crate::fields::{FieldValue, Fields};
    use crate::line::PendingLine;

    fn clean(text: &str) -> String {
        scrubbed_text(text).unwrap_or_else(|| text.to_owned())
    }

    #[test]
    fn keys_named_like_credentials_are_sensitive() {
        for key in [
            "password",
            "newPassword",
            "client_secret",
            "access_token",
            "http.request.header.authorization",
            "set-cookie",
            "X-Api-Key",
            "apiKey",
            "privateKey",
            "cardNumber",
            "db_passwd",
        ] {
            assert!(is_sensitive_key(key), "{key}");
        }
        for key in [
            "token.id",
            "cache.key",
            "feature_flag.key",
            "message",
            "email",
        ] {
            assert!(!is_sensitive_key(key), "{key}");
        }
    }

    #[test]
    fn keys_ending_in_a_credential_s_kind_of_key_or_a_plural_are_sensitive() {
        for key in [
            "aws_secret_access_key",
            "secret_key",
            "signing_key",
            "encryption_key",
            "client_key",
            "SOFIE_SIGNING_KEYS",
            "SOFIE_SESSION_KEYS",
            "refresh_tokens",
            "passwords",
            "secrets",
        ] {
            assert!(is_sensitive_key(key), "{key}");
        }
    }

    #[test]
    fn a_structured_value_after_a_sensitive_key_is_masked_whole() {
        assert_eq!(
            clean(r#"{"secret":{"value":"sk_live_x"},"id":7}"#),
            r#"{"secret":***,"id":7}"#
        );
        assert_eq!(
            clean(r#"{"api_key": ["k1", "k2"], "id":7}"#),
            r#"{"api_key": ***, "id":7}"#
        );
    }

    #[test]
    fn an_unquoted_authorization_value_is_masked_to_the_line_s_end() {
        let cases = [
            (
                "Authorization: Token ghp_abc123 extra\nretrying",
                "Authorization: ***\nretrying",
            ),
            (
                "proxy-authorization=Digest username=\"a\", response=\"b\"",
                "proxy-authorization=***",
            ),
            (
                r#"{"authorization": "Token ghp_abc123", "id": 7}"#,
                r#"{"authorization": "***", "id": 7}"#,
            ),
        ];
        for (text, expected) in cases {
            assert_eq!(clean(text), expected);
        }
    }

    #[test]
    fn a_url_password_holding_an_at_sign_is_masked_whole() {
        assert_eq!(
            clean("connecting to redis://:p@ss@cache:6379/0"),
            "connecting to redis://:***@cache:6379/0"
        );
    }

    #[test]
    fn free_text_loses_the_values_it_carries() {
        let cases = [
            (
                "connecting to postgres://app:hunter2@db:5432/shop failed",
                "connecting to postgres://app:***@db:5432/shop failed",
            ),
            (
                "sent Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.e30.sig to the vendor",
                "sent Authorization: ***",
            ),
            (
                "the vendor answered {\"id\":7,\"client_secret\":\"pi_3_secret_x\",\"api_key\": \"k\"}",
                "the vendor answered {\"id\":7,\"client_secret\":\"***\",\"api_key\": \"***\"}",
            ),
            (
                r#"decoding Some("{\"password\":\"a b\"}") failed"#,
                r#"decoding Some("{\"password\":\"***\"}") failed"#,
            ),
            (
                "login password=hunter2 user=jane",
                "login password=*** user=jane",
            ),
            (
                "GET /orders?email=jane@example.com&page=2 answered 500",
                "GET /orders?email=***&page=*** answered 500",
            ),
            (
                "duplicate key value violates unique constraint \"accounts_email\": \
                 Key (email)=(jane@example.com) already exists.",
                "duplicate key value violates unique constraint \"accounts_email\": \
                 Key (email)=(***) already exists.",
            ),
            (
                "Failing row contains (7, jane (admin), null).",
                "Failing row contains (***).",
            ),
            (
                "Failing row contains (1, (a)=(b), null).",
                "Failing row contains (***).",
            ),
            (
                "Key (name)=(row contains (x)) already exists.",
                "Key (name)=(***) already exists.",
            ),
            (
                r#"the vendor answered {"apiKey":"sk_live_1"}"#,
                r#"the vendor answered {"apiKey":"***"}"#,
            ),
            (
                "retrying with Basic YTpi now",
                "retrying with Basic *** now",
            ),
            (
                "sent cookie: theme=dark; sid=abc123;  cart=7 upstream",
                "sent cookie: *** upstream",
            ),
            (
                "cloning https://ghp_abcdef123@github.com/org/repo failed",
                "cloning https://***@github.com/org/repo failed",
            ),
        ];
        for (text, expected) in cases {
            assert_eq!(clean(text), expected);
        }
    }

    #[test]
    fn plain_text_is_left_alone() {
        for text in [
            "refused an unknown, expired or revoked bearer token",
            "request finished",
            "is it ready? not yet",
            "https://example.com/a",
            "basic auth failed",
            "a basic plan",
        ] {
            assert_eq!(scrubbed_text(text), None, "{text}");
        }
    }

    #[test]
    fn a_line_masks_sensitive_fields_and_scrubs_text_in_its_spans() {
        let mut event = Fields::default();
        event.set("message", FieldValue::Formatted("password=x".to_owned()));
        event.set("api_token", FieldValue::Unsigned(7));
        let mut span = Fields::default();
        span.set("url.query", FieldValue::Text("?q=jane".to_owned()));
        let shared = Arc::new(SpanFields::new(span));
        let mut line = PendingLine {
            level: Level::INFO,
            target: "app",
            fields: event,
            scope: vec![("request", Arc::clone(&shared))],
            trace: None,
            time: SystemTime::UNIX_EPOCH,
        };

        scrub(&mut line);

        let values: Vec<String> = line
            .fields
            .iter()
            .chain(line.scope[0].1.scrubbed().iter())
            .map(|(_, value)| value.to_string())
            .collect();
        assert_eq!(values, ["password=***", "***", "?q=***"]);
        assert_eq!(shared.iter().next().unwrap().1.to_string(), "?q=jane");
    }

    #[test]
    fn a_span_recorded_after_it_was_scrubbed_is_scrubbed_again() {
        let mut span = SpanFields::new(Fields::default());
        assert!(span.scrubbed().is_empty());

        let mut recorded = Fields::default();
        recorded.set("url.query", FieldValue::Text("?q=jane".to_owned()));
        span.merge(recorded);

        assert_eq!(
            span.scrubbed().iter().next().unwrap().1.to_string(),
            "?q=***"
        );
    }
}
