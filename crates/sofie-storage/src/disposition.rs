use sofie_core::encoding::percent;

use crate::Disposition;

/// RFC 6266: an ASCII `filename` for old clients and the UTF-8 `filename*` (RFC 5987) beside it.
/// Quotes, backslashes and characters that mislead a reader are dropped from both.
#[must_use]
pub fn content_disposition(disposition: Disposition, filename: Option<&str>) -> String {
    let mut value = disposition.as_str().to_owned();
    let Some(filename) = filename.map(shown).filter(|name| !name.is_empty()) else {
        return value;
    };
    value.push_str("; filename=\"");
    value.extend(
        filename
            .chars()
            .map(|character| if character.is_ascii() { character } else { '_' }),
    );
    value.push_str("\"; filename*=UTF-8''");
    percent::push_encoded(&mut value, &filename, is_attribute_character);
    value
}

/// RFC 5987's `attr-char`: what `filename*` may carry unescaped.
fn is_attribute_character(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&byte)
}

fn shown(filename: &str) -> String {
    filename
        .chars()
        .filter(|character| {
            !sofie_core::misleads_display(*character) && !matches!(character, '"' | '\\')
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_given_as_ascii_and_as_utf_8() {
        assert_eq!(
            content_disposition(Disposition::Attachment, None),
            "attachment"
        );
        assert_eq!(
            content_disposition(Disposition::Attachment, Some("Fatura Şubat.pdf")),
            "attachment; filename=\"Fatura _ubat.pdf\"; filename*=UTF-8''Fatura%20%C5%9Eubat.pdf"
        );
        assert_eq!(
            content_disposition(Disposition::Inline, Some("a\"b\r\n;c%.png")),
            "inline; filename=\"ab;c%.png\"; filename*=UTF-8''ab%3Bc%25.png"
        );
    }
}
