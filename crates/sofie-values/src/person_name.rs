use std::fmt;

use sofie_core::{Error, Result};

const MAXIMUM_LENGTH: usize = 200;

/// One field: names do not split into given and family the same way everywhere.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct PersonName {
    name: String,
}

impl PersonName {
    pub fn parse(text: &str) -> Result<Self> {
        let text = text.trim();
        // A char is at most 4 bytes; refused before any copy.
        if text.len() > MAXIMUM_LENGTH * 4 {
            return Err(invalid_name("must have at most 200 characters"));
        }
        let mut name = String::with_capacity(text.len());
        for word in text.split_whitespace() {
            if !name.is_empty() {
                name.push(' ');
            }
            name.push_str(word);
        }
        Self::from_collapsed(name)
    }

    fn from_stored(text: String) -> Result<Self> {
        if is_collapsed(&text) {
            return Self::from_collapsed(text);
        }
        Self::parse(&text)
    }

    fn from_collapsed(name: String) -> Result<Self> {
        if name.is_empty() {
            return Err(invalid_name("is required"));
        }
        if name.chars().count() > MAXIMUM_LENGTH {
            return Err(invalid_name("must have at most 200 characters"));
        }
        if name.chars().any(sofie_core::misleads_display) {
            return Err(invalid_name(
                "must not contain control or invisible formatting characters",
            ));
        }
        Ok(Self { name })
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.name
    }
}

/// Words joined by single spaces, as `parse` leaves them.
fn is_collapsed(text: &str) -> bool {
    let mut after_space = true;
    for character in text.chars() {
        if !character.is_whitespace() {
            after_space = false;
        } else if character != ' ' || after_space {
            return false;
        } else {
            after_space = true;
        }
    }
    !after_space || text.is_empty()
}

fn invalid_name(message: &str) -> Error {
    Error::invalid("name", message)
}

impl fmt::Debug for PersonName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let first = self.name.chars().next().unwrap_or_default();
        write!(formatter, "PersonName({first}***)")
    }
}

sofie_core::text_value!(PersonName, name, PersonName::from_stored);

crate::present::present_text!(
    PersonName,
    crate::Input::typed(&[("type", "text"), ("autocomplete", "name")]),
    Text
);

#[cfg(test)]
mod tests {
    use sofie_core::SqlValue;

    use super::*;
    use crate::testing::{assert_refused, round_trip};

    #[test]
    fn keeps_a_name_as_written_with_whitespace_collapsed() {
        for (text, name) in [
            ("Ada Lovelace", "Ada Lovelace"),
            ("  Gabriel  García\tMárquez ", "Gabriel García Márquez"),
            ("毛泽东", "毛泽东"),
            ("O'Brien-Smith", "O'Brien-Smith"),
            ("می\u{200C}خواهم", "می\u{200C}خواهم"),
        ] {
            assert_eq!(PersonName::parse(text).unwrap().as_str(), name);
        }
        assert!(PersonName::parse(&"é".repeat(200)).is_ok());
    }

    #[test]
    fn refuses_empty_overlong_control_and_invisible_formatting_characters() {
        assert_refused(
            PersonName::parse,
            &[
                "",
                "   ",
                "Ada\u{0}Lovelace",
                "Ada\u{7f}",
                "Ada\u{202E}ecalevoL",
                "Ada\u{200B}",
                &"é".repeat(201),
            ],
        );
    }

    #[test]
    fn stored_text_is_checked_and_collapsed_like_input() {
        use sofie_core::value::Value;
        let stored = |text: &str| PersonName::try_from_inner(text.to_owned()).map(|name| name.name);

        assert_eq!(stored("Ada Lovelace").ok().as_deref(), Some("Ada Lovelace"));
        assert_eq!(
            stored(" Ada  Lovelace\t").ok().as_deref(),
            Some("Ada Lovelace")
        );
        assert!(stored("").is_err());
        assert!(stored("Ada\u{202E}").is_err());
    }

    #[test]
    fn masks_itself_in_debug() {
        let name = PersonName::parse("Ada Lovelace").unwrap();

        assert_eq!(format!("{name:?}"), "PersonName(A***)");
    }

    #[test]
    fn round_trips_through_a_text_column_and_json() {
        let name = PersonName::parse("Ada Lovelace").unwrap();
        assert_eq!(
            round_trip(&name),
            vec![SqlValue::Text("Ada Lovelace".into())]
        );
        let json = serde_json::to_value(&name).unwrap();
        assert_eq!(serde_json::from_value::<PersonName>(json).unwrap(), name);
    }
}
