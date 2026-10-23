use http::HeaderMap;
use http::header::CONTENT_TYPE;
use sofie_core::encoding::percent;
use sofie_core::{Error, Result};

const MEDIA_TYPE: &str = "application/x-www-form-urlencoded";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Encoding {
    Json,
    Form,
}

impl Encoding {
    pub(crate) fn of(headers: &HeaderMap) -> Self {
        let media_type = headers
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .map(str::trim);
        match media_type {
            Some(media_type) if media_type.eq_ignore_ascii_case(MEDIA_TYPE) => Self::Form,
            _ => Self::Json,
        }
    }
}

/// Unreserved bytes and `/` as they are, every other byte as `%XX`.
#[must_use]
pub fn percent_encode(text: &str) -> String {
    percent::encode(text, is_path_byte)
}

pub(crate) fn is_path_byte(byte: u8) -> bool {
    percent::is_unreserved(byte) || byte == b'/'
}

pub(crate) fn decode(raw: &[u8]) -> Result<String> {
    unescaped(percent::decode_form(raw)).map_err(bad_form)
}

/// A path keeps `+` as it is; `None` when the segment does not decode to UTF-8.
pub(crate) fn decode_path_segment(raw: &str) -> Option<String> {
    unescaped(percent::decode(raw.as_bytes())).ok()
}

fn unescaped(bytes: Option<Vec<u8>>) -> std::result::Result<String, &'static str> {
    let bytes = bytes.ok_or("a `%` is not followed by two hexadecimal digits")?;
    // Postgres refuses NUL in text, and no form field means one; tab and line breaks stay.
    let is_refused = |byte: &u8| byte.is_ascii_control() && !matches!(byte, b'\t' | b'\n' | b'\r');
    if bytes.iter().any(is_refused) {
        return Err("it holds a control character");
    }
    String::from_utf8(bytes).map_err(|_| "the form is not UTF-8")
}

pub(crate) fn bad_form(reason: &str) -> Error {
    Error::BadRequest(format!("the form cannot be read: {reason}"))
}

#[cfg(test)]
mod tests {
    use http::HeaderValue;

    use super::*;

    #[test]
    fn encodes_every_byte_but_unreserved_ones_and_decodes_back() {
        let text = "/orders?page=2 & Jürgen+1@x.io";
        let encoded = percent_encode(text);
        assert_eq!(encoded, "/orders%3Fpage%3D2%20%26%20J%C3%BCrgen%2B1%40x.io");
        assert_eq!(decode(encoded.as_bytes()).unwrap(), text);
    }

    #[test]
    fn a_path_segment_decodes_escapes_but_keeps_plus() {
        assert_eq!(
            decode_path_segment("caf%C3%A9+bar").as_deref(),
            Some("café+bar")
        );
        assert_eq!(decode_path_segment("50%"), None);
        assert_eq!(decode_path_segment("%FF"), None);
    }

    #[test]
    fn a_control_character_is_refused_but_tabs_and_line_breaks_are_kept() {
        assert!(decode(b"note=a%00b").is_err());
        assert!(decode(b"note=a%1Bb").is_err());
        assert!(decode(b"note=a%7Fb").is_err());
        assert_eq!(decode(b"a%09b%0D%0Ac").unwrap(), "a\tb\r\nc");
        assert_eq!(decode_path_segment("a%00b"), None);
    }

    #[test]
    fn only_a_form_content_type_reads_as_a_form() {
        let headers = |value: &'static str| {
            let mut headers = HeaderMap::new();
            headers.insert(CONTENT_TYPE, HeaderValue::from_static(value));
            headers
        };
        assert_eq!(
            Encoding::of(&headers("application/x-www-form-urlencoded")),
            Encoding::Form
        );
        assert_eq!(
            Encoding::of(&headers("Application/X-WWW-Form-Urlencoded; charset=UTF-8")),
            Encoding::Form
        );
        assert_eq!(Encoding::of(&headers("application/json")), Encoding::Json);
        assert_eq!(
            Encoding::of(&headers("multipart/form-data")),
            Encoding::Json
        );
        assert_eq!(Encoding::of(&HeaderMap::new()), Encoding::Json);
    }
}
