use sofie_core::{Error, Result};

/// Each part is checked for its shape, not against a registry.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Locale {
    tag: String,
}

impl Locale {
    pub fn parse(text: &str) -> Result<Self> {
        let mut parts = text.split(['-', '_']);
        let language = parts
            .next()
            .filter(|language| is_letters(language, 2..=3))
            .ok_or_else(|| invalid_locale("must start with a language such as en"))?;
        let mut tag = language.to_ascii_lowercase();
        let mut rest = parts.peekable();
        if let Some(script) = rest.next_if(|script| is_letters(script, 4..=4)) {
            tag.push('-');
            tag.push_str(&script[..1].to_ascii_uppercase());
            tag.push_str(&script[1..].to_ascii_lowercase());
        }
        if let Some(region) = rest.next() {
            if !is_letters(region, 2..=2) && !is_digits(region, 3) {
                return Err(invalid_locale("must have a region such as US or 419"));
            }
            tag.push('-');
            tag.push_str(&region.to_ascii_uppercase());
        }
        if rest.next().is_some() {
            return Err(invalid_locale(
                "must be a language, script and region at most",
            ));
        }
        Ok(Self { tag })
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.tag
    }

    #[must_use]
    pub fn language(&self) -> &str {
        self.tag.split('-').next().unwrap_or_default()
    }
}

fn is_letters(part: &str, lengths: std::ops::RangeInclusive<usize>) -> bool {
    lengths.contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_alphabetic())
}

fn is_digits(part: &str, length: usize) -> bool {
    part.len() == length && part.bytes().all(|byte| byte.is_ascii_digit())
}

fn invalid_locale(message: &str) -> Error {
    Error::invalid("locale", message)
}

sofie_core::text_value!(Locale, tag);

crate::present::present_text!(
    Locale,
    crate::Input::typed(&[("type", "text"), ("maxlength", "35")]),
    Exact
);

#[cfg(test)]
mod tests {
    use sofie_core::SqlValue;

    use super::*;
    use crate::testing::{assert_refused, round_trip};

    #[test]
    fn writes_each_part_in_its_usual_case() {
        for (text, tag) in [
            ("tr", "tr"),
            ("en-us", "en-US"),
            ("EN_GB", "en-GB"),
            ("zh-hant-tw", "zh-Hant-TW"),
            ("sr-Latn", "sr-Latn"),
            ("es-419", "es-419"),
            ("fil-PH", "fil-PH"),
        ] {
            assert_eq!(Locale::parse(text).unwrap().as_str(), tag, "{text}");
        }
        assert_eq!(Locale::parse("zh-Hant-TW").unwrap().language(), "zh");
    }

    #[test]
    fn refuses_what_is_not_a_tag() {
        assert_refused(
            Locale::parse,
            &[
                "",
                "e",
                "engl",
                "en-",
                "en-USA",
                "en-U1",
                "en-US-x",
                "en-Latn-US-1",
                "1en",
                "en US",
            ],
        );
    }

    #[test]
    fn round_trips_through_a_text_column_and_json() {
        let locale = Locale::parse("en-US").unwrap();
        assert_eq!(round_trip(&locale), vec![SqlValue::Text("en-US".into())]);
        let read: Locale = serde_json::from_value(serde_json::json!("en_us")).unwrap();
        assert_eq!(read, locale);
        assert_eq!(
            serde_json::to_value(&read).unwrap(),
            serde_json::json!("en-US")
        );
    }
}
