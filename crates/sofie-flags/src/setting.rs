use std::fmt;

use serde_json::{Value, json};
use sofie_core::{Error, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingValue {
    Switch(bool),
    Number(i64),
    Text(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclaredSetting {
    /// `<module>.<setting>`, unique in the app.
    pub name: String,
    pub default: SettingValue,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldSetting {
    pub name: String,
    pub value: SettingValue,
    pub default: SettingValue,
}

impl SettingValue {
    #[must_use]
    pub fn same_kind(&self, other: &Self) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }

    pub fn parse_like(&self, text: &str) -> Result<Self> {
        let trimmed = text.trim();
        match self {
            Self::Switch(_) => match trimmed {
                "true" | "on" => Ok(Self::Switch(true)),
                "false" | "off" => Ok(Self::Switch(false)),
                _ => Err(Error::invalid("value", "is `true` or `false`")),
            },
            Self::Number(_) => trimmed
                .parse()
                .map(Self::Number)
                .map_err(|_| Error::invalid("value", "is a whole number")),
            Self::Text(_) => Ok(Self::Text(text.to_owned())),
        }
    }

    pub(crate) fn to_json(&self) -> String {
        match self {
            Self::Switch(on) => json!({ "switch": on }),
            Self::Number(number) => json!({ "number": number }),
            Self::Text(text) => json!({ "text": text }),
        }
        .to_string()
    }

    pub(crate) fn from_json(name: &str, text: &str) -> Result<Self> {
        let value: Value = serde_json::from_str(text).map_err(|error| {
            Error::internal(
                format!("reading the stored value of setting `{name}`"),
                error,
            )
        })?;
        let read = value.as_object().and_then(|object| {
            let (kind, value) = object.iter().next()?;
            match kind.as_str() {
                "switch" => value.as_bool().map(Self::Switch),
                "number" => value.as_i64().map(Self::Number),
                "text" => value.as_str().map(|text| Self::Text(text.to_owned())),
                _ => None,
            }
        });
        read.ok_or_else(|| {
            Error::internal_message(format!("the stored value of setting `{name}` is malformed"))
        })
    }
}

impl fmt::Display for SettingValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Switch(on) => write!(formatter, "{on}"),
            Self::Number(number) => write!(formatter, "{number}"),
            Self::Text(text) => formatter.write_str(text),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SettingValue;

    #[test]
    fn a_posted_value_is_read_as_its_declared_kind() {
        let number = SettingValue::Number(5000);
        assert_eq!(
            number.parse_like(" 7500 ").unwrap(),
            SettingValue::Number(7500)
        );
        assert!(number.parse_like("lots").is_err());
        let switch = SettingValue::Switch(false);
        assert_eq!(switch.parse_like("on").unwrap(), SettingValue::Switch(true));
        assert!(switch.parse_like("1").is_err());
        let text = SettingValue::Text(String::new());
        assert_eq!(
            text.parse_like(" a ").unwrap(),
            SettingValue::Text(" a ".to_owned())
        );
    }

    #[test]
    fn a_value_survives_its_stored_form() {
        for value in [
            SettingValue::Switch(true),
            SettingValue::Number(-3),
            SettingValue::Text("hi \"there\"".to_owned()),
        ] {
            assert_eq!(
                SettingValue::from_json("a.b", &value.to_json()).unwrap(),
                value
            );
        }
        assert!(SettingValue::from_json("a.b", "{\"number\":\"x\"}").is_err());
    }
}
