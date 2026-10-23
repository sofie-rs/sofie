use sofie_core::Shown;

/// Split once when the file is read, so rendering only copies.
#[derive(Clone, Debug)]
pub(crate) struct Message {
    parts: Vec<Part>,
}

#[derive(Clone, Debug)]
pub(crate) enum Part {
    Text(String),
    Argument(String),
}

impl Message {
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let mut parts = Vec::new();
        let mut rest = text;
        while let Some(brace) = rest.find(['{', '}']) {
            let (before, from_brace) = rest.split_at(brace);
            if !before.is_empty() {
                parts.push(Part::Text(before.to_owned()));
            }
            let Some(inside) = from_brace.strip_prefix('{') else {
                return Err(
                    "a `}` closes no placeholder; placeholders are written `{name}`".to_owned(),
                );
            };
            let end = inside.find('}').ok_or_else(|| {
                "a `{` is never closed; placeholders are written `{name}`".to_owned()
            })?;
            let name = inside.get(..end).unwrap_or_default();
            if !is_argument_name(name) {
                return Err(format!(
                    "`{{{name}}}` is not a placeholder: write a lowercase name such as `{{count}}`"
                ));
            }
            parts.push(Part::Argument(name.to_owned()));
            rest = inside.get(end + 1..).unwrap_or_default();
        }
        if !rest.is_empty() {
            parts.push(Part::Text(rest.to_owned()));
        }
        Ok(Self { parts })
    }

    pub(crate) fn parts(&self) -> &[Part] {
        &self.parts
    }

    #[cfg(feature = "i18n")]
    pub(crate) fn plain(&self) -> Option<&str> {
        match self.parts.as_slice() {
            [] => Some(""),
            [Part::Text(text)] => Some(text),
            _ => None,
        }
    }

    /// With its `{name}` placeholders, as a language file writes it.
    #[cfg(any(feature = "parse", feature = "clients"))]
    pub(crate) fn written(&self) -> String {
        let mut text = String::new();
        for part in &self.parts {
            match part {
                Part::Text(literal) => text.push_str(literal),
                Part::Argument(name) => {
                    text.push('{');
                    text.push_str(name);
                    text.push('}');
                }
            }
        }
        text
    }

    /// Sorted, so two messages compare.
    #[cfg(all(feature = "parse", feature = "i18n"))]
    pub(crate) fn arguments(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .parts
            .iter()
            .filter_map(|part| match part {
                Part::Argument(name) => Some(name.as_str()),
                Part::Text(_) => None,
            })
            .collect();
        names.sort_unstable();
        names.dedup();
        names
    }
}

/// A `t(…)` call's text in the source language, compiled into the screen, so a build without
/// translations still writes it.
#[derive(Clone, Debug)]
pub(crate) struct Recorded {
    pub other: Message,
    pub one: Option<Message>,
}

impl Recorded {
    /// The source language is English (`sofie_model::naming::SOURCE_LANGUAGE`), whose `one` is
    /// exactly 1.
    pub(crate) fn counted(&self, count: Option<Shown<'_>>) -> &Message {
        let number = match count {
            Some(Shown::Integer(count)) => return self.pick(count == 1),
            Some(Shown::Number(count)) => count.as_f64(),
            Some(Shown::Float(count)) => Some(count),
            _ => None,
        };
        self.pick(number.is_some_and(|number| (number - 1.0).abs() < f64::EPSILON))
    }

    fn pick(&self, is_one: bool) -> &Message {
        match &self.one {
            Some(one) if is_one => one,
            _ => &self.other,
        }
    }
}

pub(crate) fn is_argument_name(name: &str) -> bool {
    name.starts_with(|first: char| first.is_ascii_lowercase())
        && name.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
}

#[cfg(feature = "parse")]
pub(crate) fn is_key(key: &str) -> bool {
    key.split('.').all(|name| {
        !name.is_empty()
            && name.chars().all(|character| {
                character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
            })
    })
}

#[cfg(test)]
mod tests {
    use sofie_core::Shown;

    use super::{Message, Part, Recorded};

    #[test]
    fn splits_text_and_placeholders() {
        let message = Message::parse("{count} orders for {name}, {count}").unwrap();

        assert!(matches!(
            message.parts(),
            [Part::Argument(count), Part::Text(text), Part::Argument(_), Part::Text(_), Part::Argument(_)]
                if count == "count" && text == " orders for "
        ));
        #[cfg(all(feature = "parse", feature = "i18n"))]
        assert_eq!(message.arguments(), ["count", "name"]);
        #[cfg(feature = "i18n")]
        assert_eq!(message.plain(), None);
        #[cfg(feature = "i18n")]
        assert_eq!(Message::parse("Orders").unwrap().plain(), Some("Orders"));
    }

    #[test]
    fn a_recorded_count_of_one_takes_the_one_form() {
        let recorded = Recorded {
            other: Message::parse("{count} orders").unwrap(),
            one: Some(Message::parse("one order").unwrap()),
        };

        assert!(matches!(
            recorded.counted(Some(Shown::Integer(1))).parts(),
            [Part::Text(text)] if text == "one order"
        ));
        for count in [Some(Shown::Integer(2)), Some(Shown::Integer(0)), None] {
            assert!(matches!(
                recorded.counted(count).parts(),
                [Part::Argument(name), Part::Text(_)] if name == "count"
            ));
        }
    }

    #[test]
    fn refuses_lone_braces_and_odd_names() {
        for text in ["a } b", "{count", "{Count}", "{}", "{a b}"] {
            assert!(Message::parse(text).is_err(), "{text}");
        }
    }
}
