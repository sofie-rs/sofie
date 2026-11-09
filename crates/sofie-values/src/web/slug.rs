use sofie_core::{Error, Result};

use crate::slug_character::{draws_nothing, without_marks_that_draw_nothing};

const MAXIMUM_LENGTH: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Slug {
    text: String,
}

impl Slug {
    pub fn parse(text: &str) -> Result<Self> {
        Self::checked(text.to_owned())
    }

    /// Also reads a slug stored before marks that draw nothing were refused.
    fn from_stored(text: String) -> Result<Self> {
        if text.chars().any(draws_nothing) {
            Self::checked(without_marks_that_draw_nothing(&text))?;
            return Ok(Self { text });
        }
        Self::checked(text)
    }

    fn checked(text: String) -> Result<Self> {
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

/// `is_slug_character` for the browser, which reads a pattern with the `v` flag: letters that
/// lowercasing keeps, digits and marks, less the marks and fillers that draw nothing, enclosing
/// marks and decomposed Latin accents.
macro_rules! slug_character_class {
    () => {
        concat!(
            r"[[\p{Ll}\p{Lm}\p{Lo}\p{Mn}\p{Mc}\p{N}]--[\u{300}-\u{36f}\u{34f}\u{17b4}\u{17b5}",
            r"\u{180b}-\u{180d}\u{180f}\u{fe00}-\u{fe0f}\u{e0100}-\u{e01ef}\u{115f}\u{1160}",
            r"\u{3164}\u{ffa0}\u{2160}-\u{216f}]]",
        )
    };
}

crate::present::present_text!(
    Slug,
    crate::Input::typed(&[
        ("type", "text"),
        (
            "pattern",
            concat!(
                slug_character_class!(),
                "+(-",
                slug_character_class!(),
                "+)*"
            )
        )
    ]),
    Exact
);

#[cfg(test)]
mod tests {
    use sofie_core::{Cast, RowReader, SqlValue};

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

    #[test]
    fn reads_a_slug_stored_before_marks_that_draw_nothing_were_refused() {
        let values = [SqlValue::Text("blue\u{FE0F}-scarf".into())];
        let slug = Slug::decode(&mut RowReader::new(&values)).unwrap();

        assert_eq!(slug.as_str(), "blue\u{FE0F}-scarf");
        assert!(
            serde_json::to_value(&slug)
                .and_then(serde_json::from_value::<Slug>)
                .is_err()
        );
        let refused = [SqlValue::Text("Blue\u{FE0F}".into())];
        assert!(Slug::decode(&mut RowReader::new(&refused)).is_err());
    }
}
