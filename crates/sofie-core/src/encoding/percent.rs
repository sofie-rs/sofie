//! Percent-encoding (RFC 3986, section 2.1), with uppercase digits as it recommends.

use super::hex;

const DIGITS: &[u8; 16] = b"0123456789ABCDEF";

/// RFC 3986, section 2.3: the bytes that never need escaping.
#[must_use]
pub fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

#[must_use]
pub fn encode(text: &str, keep: impl Fn(u8) -> bool) -> String {
    let mut out = String::with_capacity(text.len());
    push_encoded(&mut out, text, keep);
    out
}

/// Every byte `keep` refuses as `%XX`; bytes outside ASCII always are.
pub fn push_encoded(out: &mut String, text: &str, keep: impl Fn(u8) -> bool) {
    push(out, text, keep, Space::Escaped);
}

/// `application/x-www-form-urlencoded` (WHATWG URL, section 5.2): `name=value` pairs joined by `&`.
#[must_use]
pub fn form<'a>(fields: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    let mut out = String::new();
    for (name, value) in fields {
        if !out.is_empty() {
            out.push('&');
        }
        push_form_encoded(&mut out, name);
        out.push('=');
        push_form_encoded(&mut out, value);
    }
    out
}

/// One form name or value: a space as `+`, and only `*-._` kept beside letters and digits.
pub fn push_form_encoded(out: &mut String, text: &str) {
    push(out, text, is_form_safe, Space::Plus);
}

/// `+` stays `+`, as in a path. `None` when a `%` is not followed by two hex digits.
#[must_use]
pub fn decode(encoded: &[u8]) -> Option<Vec<u8>> {
    unescape(encoded, Space::Escaped)
}

/// `+` is a space, as in a form. `None` when a `%` is not followed by two hex digits.
#[must_use]
pub fn decode_form(encoded: &[u8]) -> Option<Vec<u8>> {
    unescape(encoded, Space::Plus)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Space {
    Escaped,
    Plus,
}

fn is_form_safe(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'*' | b'-' | b'.' | b'_')
}

fn push(out: &mut String, text: &str, keep: impl Fn(u8) -> bool, space: Space) {
    let mut copied = 0;
    for (index, byte) in text.bytes().enumerate() {
        if byte.is_ascii() && keep(byte) {
            continue;
        }
        // A kept run is ASCII, so it starts and ends on a character boundary; between two
        // escaped bytes of one character, `copied` does not, and slicing there panics.
        if copied < index {
            out.push_str(&text[copied..index]);
        }
        if space == Space::Plus && byte == b' ' {
            out.push('+');
        } else {
            out.push('%');
            out.push(char::from(DIGITS[usize::from(byte >> 4)]));
            out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
        }
        copied = index + 1;
    }
    out.push_str(&text[copied..]);
}

fn unescape(encoded: &[u8], space: Space) -> Option<Vec<u8>> {
    let mut bytes = Vec::with_capacity(encoded.len());
    let mut rest = encoded;
    while let Some((&byte, tail)) = rest.split_first() {
        rest = tail;
        match byte {
            b'+' if space == Space::Plus => bytes.push(b' '),
            b'%' => {
                let ([high, low], tail) = rest.split_first_chunk()?;
                bytes.push(hex::byte_of(*high, *low)?);
                rest = tail;
            }
            _ => bytes.push(byte),
        }
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encodes_all_but_unreserved_characters() {
        let mut out = String::new();
        push_encoded(&mut out, "a-Z_9.~ /?#%é", is_unreserved);
        assert_eq!(out, "a-Z_9.~%20%2F%3F%23%25%C3%A9");
    }

    #[test]
    fn keep_decides_which_ascii_bytes_stay_but_never_keeps_other_bytes() {
        assert_eq!(encode("", is_unreserved), "");
        assert_eq!(encode("a/b c", |byte| byte == b'/'), "%61/%62%20%63");
        assert_eq!(encode("é/", |_| true), "%C3%A9/");
        assert_eq!(encode("ab", |_| false), "%61%62");
    }

    #[test]
    fn a_form_field_is_percent_encoded_with_spaces_as_plus() {
        let mut out = String::new();
        push_form_encoded(&mut out, "line_items[0][price]=a b&ç");
        assert_eq!(out, "line_items%5B0%5D%5Bprice%5D%3Da+b%26%C3%A7");
        assert_eq!(encode("*~+", is_form_safe), "*%7E%2B");
    }

    #[test]
    fn a_form_joins_its_fields() {
        assert_eq!(form([]), "");
        assert_eq!(form([("a", "1 2"), ("b&", "")]), "a=1+2&b%26=");
    }

    #[test]
    fn decodes_either_case_and_plus_only_in_a_form() {
        assert_eq!(decode(b"caf%C3%a9+bar").unwrap(), "café+bar".as_bytes());
        assert_eq!(
            decode_form(b"caf%C3%a9+bar").unwrap(),
            "café bar".as_bytes()
        );
        assert_eq!(decode(b"").unwrap(), b"");
        assert_eq!(decode(b"%2B").unwrap(), b"+");
    }

    #[test]
    fn refuses_a_percent_without_two_hex_digits() {
        for encoded in [&b"50%"[..], b"%2", b"%zz", b"%+f", b"a%-1b"] {
            assert_eq!(decode(encoded), None, "{encoded:?}");
            assert_eq!(decode_form(encoded), None, "{encoded:?}");
        }
    }

    #[test]
    fn decoding_undoes_encoding() {
        let text = "/orders?page=2 & Jürgen+1@x.io";
        assert_eq!(
            decode(encode(text, is_unreserved).as_bytes()).unwrap(),
            text.as_bytes()
        );
        assert_eq!(
            decode_form(form([(text, text)]).as_bytes()).unwrap(),
            format!("{text}={text}").as_bytes()
        );
    }
}
