use sofie_core::{Error, Result};

const MAXIMUM_LENGTH: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Slug {
    text: String,
}

impl Slug {
    pub fn parse(text: &str) -> Result<Self> {
        Self::from_stored(text.to_owned())
    }

    fn from_stored(text: String) -> Result<Self> {
        if text.is_empty() || text.len() > MAXIMUM_LENGTH {
            return Err(invalid_slug("must have 1 to 100 characters"));
        }
        let well_formed = text
            .split('-')
            .all(|group| !group.is_empty() && group.chars().all(crate::is_slug_character));
        if !well_formed {
            return Err(invalid_slug(
                "must be lowercase letters and digits joined by single hyphens",
            ));
        }
        Ok(Self { text })
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

fn invalid_slug(message: &str) -> Error {
    Error::invalid("slug", message)
}

sofie_core::text_value!(Slug, text, Slug::from_stored);

crate::present::present_text!(
    Slug,
    crate::Input::typed(&[
        ("type", "text"),
        (
            "pattern",
            r"[\p{Ll}\p{Lm}\p{Lo}\p{M}\p{N}]+(-[\p{Ll}\p{Lm}\p{Lo}\p{M}\p{N}]+)*"
        )
    ]),
    Exact
);

#[cfg(test)]
mod tests {
    use sofie_core::SqlValue;

    use super::*;
    use crate::testing::{assert_refused, round_trip};

    #[test]
    fn accepts_hyphenated_lowercase_words_and_digits() {
        for text in [
            "scarf",
            "blue-wool-scarf",
            "2026-sale",
            "ogle-yemegi",
            "关于我们",
            "привет-мир",
            "नमस्ते-दुनिया",
            "ไม่-ใช่",
            "a",
            &"a".repeat(100),
        ] {
            assert_eq!(Slug::parse(text).unwrap().as_str(), text);
        }
    }

    #[test]
    fn refuses_anything_else() {
        assert_refused(
            Slug::parse,
            &[
                "",
                "Blue-Scarf",
                "blue--scarf",
                "-scarf",
                "scarf-",
                "blue scarf",
                "blue_scarf",
                "Şal",
                "привет мир",
                "नमस्ते दुनिया",
                "ไม่!",
                "Cre\u{300}me",
                "🎉",
                "about\u{feff}",
                "a\u{200b}b",
                "a\u{e000}",
                "a\u{378}",
                "\u{3164}",
                "a\u{20dd}",
                &"a".repeat(101),
            ],
        );
    }

    #[test]
    fn round_trips_through_a_text_column_and_json() {
        let slug = Slug::parse("blue-wool-scarf").unwrap();
        assert_eq!(
            round_trip(&slug),
            vec![SqlValue::Text("blue-wool-scarf".into())]
        );
        let json = serde_json::to_value(&slug).unwrap();
        assert_eq!(serde_json::from_value::<Slug>(json).unwrap(), slug);
        assert!(serde_json::from_value::<Slug>(serde_json::json!("Blue")).is_err());
    }
}
