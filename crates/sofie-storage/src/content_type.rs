use std::fmt;

use sofie_core::{Error, Result};

const MAXIMUM_LENGTH: usize = 127;
const OCTET_STREAM: &str = "application/octet-stream";

/// The types told apart by their first bytes; a file claiming one must start with its signature.
const SIGNATURES: &[(&str, &[u8])] = &[
    ("image/png", b"\x89PNG\r\n\x1a\n"),
    ("image/jpeg", b"\xff\xd8\xff"),
    ("image/gif", b"GIF87a"),
    ("image/gif", b"GIF89a"),
    ("application/pdf", b"%PDF-"),
];
const WEBP: &str = "image/webp";
/// Shown in the page when downloaded; any other type is sent as an attachment, so an uploaded
/// HTML or SVG file never runs as the app's own page.
const INLINE: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/gif",
    WEBP,
    "application/pdf",
];

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ContentType {
    essence: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    Inline,
    Attachment,
}

impl ContentType {
    /// The first bytes `sniff` and `matches` need.
    pub const SNIFFED_LENGTH: usize = 12;

    pub fn parse(text: &str) -> Result<Self> {
        let essence = text
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let essence = match essence.as_str() {
            "image/jpg" | "image/pjpeg" => "image/jpeg".to_owned(),
            _ => essence,
        };
        let is_media_type = essence.len() <= MAXIMUM_LENGTH
            && essence
                .split_once('/')
                .is_some_and(|(kind, subtype)| is_token(kind) && is_token(subtype));
        if !is_media_type {
            return Err(Error::invalid(
                "content_type",
                "is not a media type such as image/png",
            ));
        }
        Ok(Self { essence })
    }

    #[must_use]
    pub fn octet_stream() -> Self {
        Self {
            essence: OCTET_STREAM.to_owned(),
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.essence
    }

    /// `None` when the bytes start like no type this checks, which a text file does.
    #[must_use]
    pub fn sniff(start: &[u8]) -> Option<Self> {
        let essence = SIGNATURES
            .iter()
            .find(|(_, signature)| start.starts_with(signature))
            .map(|(essence, _)| *essence)
            .or_else(|| is_webp(start).then_some(WEBP))?;
        Some(Self {
            essence: essence.to_owned(),
        })
    }

    #[must_use]
    pub fn matches(&self, start: &[u8]) -> bool {
        // Claims nothing, and is served as an attachment whatever it holds.
        if self.essence == OCTET_STREAM {
            return true;
        }
        match Self::sniff(start) {
            Some(sniffed) => sniffed == *self,
            None => !self.is_sniffed(),
        }
    }

    /// How a download is sent: its type, and inline only when that cannot run as a page.
    #[must_use]
    pub fn served(&self) -> (&str, Disposition) {
        if INLINE.contains(&self.essence.as_str()) {
            return (&self.essence, Disposition::Inline);
        }
        (OCTET_STREAM, Disposition::Attachment)
    }

    fn is_sniffed(&self) -> bool {
        self.essence == WEBP
            || SIGNATURES
                .iter()
                .any(|(essence, _)| *essence == self.essence)
    }
}

impl Disposition {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inline => "inline",
            Self::Attachment => "attachment",
        }
    }
}

fn is_webp(start: &[u8]) -> bool {
    start.len() >= 12 && start.starts_with(b"RIFF") && &start[8..12] == b"WEBP"
}

/// RFC 9110's token characters, less those a URL path reads otherwise (`#`, `?`, `&`, …): a
/// type is part of a stored file's link. No registered media type uses them.
fn is_token(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.+".contains(&byte))
}

impl fmt::Display for ContentType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.essence)
    }
}

impl fmt::Debug for ContentType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ContentType({})", self.essence)
    }
}

sofie_core::text_value!(ContentType, essence);

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(text: &str) -> ContentType {
        ContentType::parse(text).unwrap()
    }

    #[test]
    fn keeps_the_lowercase_essence_and_refuses_what_is_no_media_type() {
        assert_eq!(parsed("Text/Plain; charset=UTF-8").as_str(), "text/plain");
        assert_eq!(parsed("image/jpg").as_str(), "image/jpeg");
        for text in [
            "",
            "text",
            "text/",
            "/plain",
            "text/pla in",
            "a/b/c",
            "text/\"x\"",
        ] {
            assert!(ContentType::parse(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn sniffs_common_types_by_their_first_bytes() {
        let samples: [(&[u8], &str); 5] = [
            (b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR", "image/png"),
            (b"\xff\xd8\xff\xe0\0\x10JFIF", "image/jpeg"),
            (b"GIF89a\x01\0", "image/gif"),
            (b"RIFF\x24\0\0\0WEBPVP8 ", "image/webp"),
            (b"%PDF-1.7\n", "application/pdf"),
        ];
        for (start, essence) in samples {
            assert_eq!(
                ContentType::sniff(start),
                Some(parsed(essence)),
                "{essence}"
            );
        }
        assert_eq!(ContentType::sniff(b"name,total\n"), None);
    }

    #[test]
    fn a_claimed_type_must_agree_with_the_bytes() {
        assert!(parsed("image/png").matches(b"\x89PNG\r\n\x1a\n...."));
        assert!(!parsed("image/png").matches(b"<script>alert(1)</script>"));
        assert!(!parsed("image/jpeg").matches(b"\x89PNG\r\n\x1a\n...."));
        assert!(!parsed("text/plain").matches(b"%PDF-1.7\n"));
        assert!(parsed("text/csv").matches(b"name,total\n"));
        assert!(ContentType::octet_stream().matches(b"\x89PNG\r\n\x1a\n...."));
        assert!(ContentType::parse("text/x#y").is_err());
    }

    #[test]
    fn only_types_that_cannot_run_as_a_page_are_shown_inline() {
        assert_eq!(
            parsed("image/png").served(),
            ("image/png", Disposition::Inline)
        );
        for essence in [
            "text/html",
            "image/svg+xml",
            "text/plain",
            "application/xml",
        ] {
            assert_eq!(
                parsed(essence).served(),
                (OCTET_STREAM, Disposition::Attachment),
                "{essence}"
            );
        }
    }
}
