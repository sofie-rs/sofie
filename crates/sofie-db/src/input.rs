use std::fmt::Display;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};

use sofie_core::{Error, FieldError, Result};

pub trait Input: Sized {
    fn from_json(value: Value) -> Result<Self>;
}

/// The fields an input reads, so a query string's other keys are told apart before reading.
pub trait FieldNames {
    const FIELD_NAMES: &'static [&'static str];
}

/// No fields: reading it refuses every field given.
impl Input for () {
    fn from_json(value: Value) -> Result<Self> {
        InputFields::new(value)?.finish()
    }
}

impl FieldNames for () {
    const FIELD_NAMES: &'static [&'static str] = &[];
}

pub trait Validate {
    fn validate(&self) -> Result<(), Vec<FieldError>>;
}

pub struct InputFields {
    object: Map<String, Value>,
    errors: Vec<FieldError>,
}

impl InputFields {
    pub fn new(value: Value) -> Result<Self> {
        let Value::Object(object) = value else {
            return Err(Error::BadRequest(
                "the input must be a JSON object".to_owned(),
            ));
        };
        Ok(Self {
            object,
            errors: Vec::new(),
        })
    }

    pub fn take<T: DeserializeOwned>(&mut self, name: &str) -> Result<T> {
        match self.object.remove(name) {
            Some(value) => self.read(name, &value),
            None => Err(self.refuse(FieldError::required(name))),
        }
    }

    pub fn optional<T: DeserializeOwned>(&mut self, name: &str) -> Result<Option<T>> {
        match self.object.remove(name) {
            Some(value) => self.read(name, &value),
            None => Ok(None),
        }
    }

    pub fn finish(self) -> Result<()> {
        let Self { object, mut errors } = self;
        errors.extend(
            object
                .into_iter()
                .map(|(field, _)| FieldError::new(field, "validation.unknown", "is not accepted")),
        );
        if errors.is_empty() {
            return Ok(());
        }
        Err(Error::Invalid(errors))
    }

    /// A form posts strings, and a list's name once for one ticked box; both are reread as a `T`.
    fn read<T: DeserializeOwned>(&mut self, name: &str, value: &Value) -> Result<T> {
        let error = match T::deserialize(value) {
            Ok(read) => return Ok(read),
            Err(error) => error,
        };
        let reread = match value {
            Value::String(_) => vec![
                scalar_literal(value),
                Some(Value::Array(vec![value.clone()])),
                scalar_literal(value).map(|literal| Value::Array(vec![literal])),
            ],
            Value::Array(items) => vec![Some(Value::Array(
                items
                    .iter()
                    .map(|item| scalar_literal(item).unwrap_or_else(|| item.clone()))
                    .collect(),
            ))],
            _ => Vec::new(),
        };
        if let Some(read) = reread
            .into_iter()
            .flatten()
            .find_map(|value| T::deserialize(value).ok())
        {
            return Ok(read);
        }
        Err(self.refuse(FieldError::invalid(name, error.to_string())))
    }

    pub fn checked<T>(
        &mut self,
        read: Result<T>,
        refusal: impl FnOnce(&T) -> Option<FieldError>,
    ) -> Result<T> {
        let value = read?;
        match refusal(&value) {
            Some(error) => Err(self.refuse(error)),
            None => Ok(value),
        }
    }

    fn refuse(&mut self, error: FieldError) -> Error {
        self.errors.push(error.clone());
        Error::Invalid(vec![error])
    }
}

pub trait InputValue: DeserializeOwned {
    const FIELD: &'static str;
}

pub fn lone_value<V: InputValue>(input: Value) -> Result<V> {
    read_alone(input, |fields| fields.take(V::FIELD))
}

pub fn optional_lone_value<V: InputValue>(input: Value) -> Result<Option<V>> {
    read_alone(input, |fields| fields.optional(V::FIELD))
}

fn read_alone<T>(input: Value, read: impl FnOnce(&mut InputFields) -> Result<T>) -> Result<T> {
    let mut fields = InputFields::new(input)?;
    let value = read(&mut fields);
    fields.finish()?;
    value
}

/// Every missing, invalid or unknown field is reported before the refusal of `try_new`.
pub fn deserialize_fields<'de, V, E: Display, D: Deserializer<'de>>(
    deserializer: D,
    build: impl FnOnce(&mut InputFields) -> Result<Result<V, E>>,
) -> Result<V, D::Error> {
    let object = serde_json::Value::deserialize(deserializer)?;
    match read_alone(object, build) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(refusal)) => Err(serde::de::Error::custom(format_args!("{refusal:#}"))),
        Err(error) => Err(error.into_deserialize_error()),
    }
}

fn scalar_literal(value: &Value) -> Option<Value> {
    let Value::String(text) = value else {
        return None;
    };
    serde_json::from_str(text)
        .ok()
        .filter(|literal| matches!(literal, Value::Number(_) | Value::Bool(_)))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use sofie_core::{Constraint, FieldRules};

    fn invalid_fields(result: Result<()>) -> Vec<(String, String)> {
        match result {
            Err(Error::Invalid(errors)) => errors
                .into_iter()
                .map(|error| (error.field, error.message))
                .collect(),
            other => panic!("expected invalid input, got {other:?}"),
        }
    }

    #[test]
    fn reads_required_and_optional_fields() {
        let mut fields = InputFields::new(json!({ "name": "Ada", "age": null })).unwrap();

        assert_eq!(fields.take::<String>("name").unwrap(), "Ada");
        assert_eq!(fields.optional::<i64>("age").unwrap(), None);
        assert_eq!(fields.optional::<i64>("height").unwrap(), None);
        assert!(fields.finish().is_ok());
    }

    #[test]
    fn collects_missing_unacceptable_and_unknown_fields() {
        let mut fields = InputFields::new(json!({ "age": "old", "colour": "red" })).unwrap();

        assert!(fields.take::<String>("name").is_err());
        assert!(fields.optional::<i64>("age").is_err());

        let errors = invalid_fields(fields.finish());
        let names: Vec<&str> = errors.iter().map(|(field, _)| field.as_str()).collect();
        assert_eq!(names, ["name", "age", "colour"]);
        assert_eq!(errors[0].1, "is required");
        assert_eq!(errors[2].1, "is not accepted");
    }

    #[test]
    fn reads_numbers_and_booleans_a_form_posts_as_strings() {
        let posted = json!({ "count": "3", "gift": "true", "ratio": "0.5", "name": "7" });
        let mut fields = InputFields::new(posted).unwrap();

        assert_eq!(fields.take::<u32>("count").unwrap(), 3);
        assert!(fields.take::<bool>("gift").unwrap());
        assert_eq!(fields.optional::<f64>("ratio").unwrap(), Some(0.5));
        assert_eq!(fields.take::<String>("name").unwrap(), "7");
        assert!(fields.finish().is_ok());
    }

    #[test]
    fn a_list_posted_once_by_a_form_reads_as_one_item() {
        let posted = json!({ "colours": "red", "sizes": "3", "name": "Ada" });
        let mut fields = InputFields::new(posted).unwrap();

        assert_eq!(fields.take::<Vec<String>>("colours").unwrap(), ["red"]);
        assert_eq!(fields.take::<Vec<i64>>("sizes").unwrap(), [3]);
        assert_eq!(fields.take::<String>("name").unwrap(), "Ada");
        assert!(fields.finish().is_ok());
    }

    #[test]
    fn list_items_a_form_posts_as_strings_read_as_what_they_spell() {
        let posted =
            json!({ "sizes": ["3", "4"], "gifts": ["true", "false"], "codes": ["7", "x"] });
        let mut fields = InputFields::new(posted).unwrap();

        assert_eq!(fields.take::<Vec<i64>>("sizes").unwrap(), [3, 4]);
        assert_eq!(fields.take::<Vec<bool>>("gifts").unwrap(), [true, false]);
        assert_eq!(fields.take::<Vec<String>>("codes").unwrap(), ["7", "x"]);
        assert!(fields.finish().is_ok());
    }

    #[test]
    fn a_list_item_that_spells_no_number_is_refused() {
        let mut fields = InputFields::new(json!({ "sizes": ["3", "large"] })).unwrap();

        assert!(fields.take::<Vec<i64>>("sizes").is_err());
        let errors = invalid_fields(fields.finish());
        assert_eq!(errors[0].0, "sizes");
    }

    #[test]
    fn a_missing_list_is_still_required() {
        let mut fields = InputFields::new(json!({})).unwrap();

        assert!(fields.take::<Vec<String>>("colours").is_err());
        assert_eq!(fields.optional::<Vec<String>>("sizes").unwrap(), None);
        assert_eq!(invalid_fields(fields.finish())[0].1, "is required");
    }

    #[test]
    fn a_string_spelling_anything_else_keeps_its_first_error() {
        let posted = json!({ "count": "[3]", "gift": "null", "size": "-1" });
        let mut fields = InputFields::new(posted).unwrap();

        assert!(fields.take::<Vec<u32>>("count").is_err());
        assert!(fields.optional::<bool>("gift").is_err());
        assert!(fields.take::<u32>("size").is_err());

        let messages: Vec<String> = invalid_fields(fields.finish())
            .into_iter()
            .map(|(_, message)| message)
            .collect();
        assert_eq!(
            messages,
            [
                "invalid type: string \"[3]\", expected a sequence",
                "invalid type: string \"null\", expected a boolean",
                "invalid type: string \"-1\", expected u32",
            ]
        );
    }

    #[test]
    fn a_value_refusing_its_input_reports_only_its_messages() {
        struct Even;
        impl<'de> Deserialize<'de> for Even {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let number = u32::deserialize(deserializer)?;
                if number % 2 == 1 {
                    return Err(Error::invalid("even", "must be even").into_deserialize_error());
                }
                Ok(Self)
            }
        }
        let mut fields = InputFields::new(json!({ "count": 3 })).unwrap();

        assert!(fields.take::<Even>("count").is_err());

        assert_eq!(
            invalid_fields(fields.finish()),
            [("count".to_owned(), "must be even".to_owned())]
        );
    }

    #[test]
    fn keys_each_error_for_translation() {
        let mut fields = InputFields::new(json!({ "age": "old", "colour": "red" })).unwrap();

        assert!(fields.take::<String>("name").is_err());
        assert!(fields.take::<i64>("age").is_err());

        let Err(Error::Invalid(errors)) = fields.finish() else {
            panic!("expected invalid input");
        };
        let keys: Vec<&str> = errors.iter().map(|error| error.key.as_ref()).collect();
        assert_eq!(
            keys,
            [
                "validation.required",
                "validation.invalid",
                "validation.unknown"
            ]
        );
    }

    #[test]
    fn a_field_breaking_a_constraint_is_refused_with_the_others() {
        let mut fields = InputFields::new(json!({ "name": "Ada Lovelace", "age": 3 })).unwrap();

        let name_rules = FieldRules::<String>::new(&[Constraint::MaxLength(4)]);
        let age_rules = FieldRules::<i64>::new(&[Constraint::Min(1)]);

        let name = fields.take::<String>("name");
        let name = fields.checked(name, |value| name_rules.refusal("name", value));
        let age = fields.take::<i64>("age");
        let age = fields.checked(age, |value| age_rules.refusal("age", value));

        assert!(name.is_err());
        assert_eq!(age.unwrap(), 3);
        assert_eq!(
            invalid_fields(fields.finish()),
            [("name".to_owned(), "must be at most 4 characters".to_owned())]
        );
    }

    #[test]
    fn refuses_anything_but_an_object() {
        assert!(matches!(
            InputFields::new(json!([1])),
            Err(Error::BadRequest(_))
        ));
    }

    #[derive(Debug, PartialEq)]
    struct Span {
        low: i64,
        high: i64,
    }

    fn read_span(value: serde_json::Value) -> Result<Span, serde_json::Error> {
        deserialize_fields(value, |fields| {
            let low = fields.take("low");
            let high = fields.take("high");
            let (low, high) = (low?, high?);
            if low > high {
                return Ok(Err("low must not exceed high"));
            }
            Ok(Ok(Span { low, high }))
        })
    }

    #[test]
    fn reads_several_fields_through_the_check() {
        assert_eq!(
            read_span(json!({"low": 1, "high": "2"})).unwrap(),
            Span { low: 1, high: 2 }
        );
        let refused = read_span(json!({"low": 3, "high": 2})).unwrap_err();
        assert_eq!(refused.to_string(), "low must not exceed high");
    }

    #[test]
    fn reports_every_field_before_the_check() {
        let missing = read_span(json!({"low": "x", "width": 2})).unwrap_err();

        assert_eq!(
            missing.to_string(),
            "invalid type: string \"x\", expected i64; is required; is not accepted"
        );
        assert!(read_span(json!([1, 2])).is_err());
    }
}
