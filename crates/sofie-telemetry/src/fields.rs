use std::fmt::{self, Debug, Write as _};

use tracing::field::{Field, Visit};

/// Named by `tracing-opentelemetry`, which collectors' dashboards follow.
#[cfg(any(feature = "otlp", feature = "development"))]
pub(crate) const NAME_FIELD: &str = "otel.name";
pub(crate) const UNFORMATTABLE: &str = "<the value's Debug implementation failed>";

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum FieldValue {
    Text(String),
    Formatted(String),
    /// Laid out by the framework across lines, its values already escaped as they were added.
    #[cfg_attr(
        not(feature = "development"),
        expect(dead_code, reason = "only the development span tree is composed")
    )]
    Composed(String),
    Signed(i64),
    Unsigned(u64),
    Float(f64),
    Bool(bool),
}

impl fmt::Display for FieldValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(text) | Self::Formatted(text) | Self::Composed(text) => {
                formatter.write_str(text)
            }
            Self::Signed(number) => write!(formatter, "{number}"),
            Self::Unsigned(number) => write!(formatter, "{number}"),
            Self::Float(number) => write!(formatter, "{number:?}"),
            Self::Bool(value) => write!(formatter, "{value}"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Fields(Vec<(&'static str, FieldValue)>);

impl Fields {
    pub(crate) fn set(&mut self, name: &'static str, value: FieldValue) {
        match self.0.iter_mut().find(|(existing, _)| *existing == name) {
            Some((_, slot)) => *slot = value,
            None => self.0.push((name, value)),
        }
    }

    pub(crate) fn merge(&mut self, recorded: Self) {
        for (name, value) in recorded.0 {
            self.set(name, value);
        }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&'static str, &FieldValue)> {
        self.0.iter().map(|(name, value)| (*name, value))
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = (&'static str, &mut FieldValue)> {
        self.0.iter_mut().map(|(name, value)| (*name, value))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl IntoIterator for Fields {
    type Item = (&'static str, FieldValue);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl Visit for Fields {
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.set(field.name(), FieldValue::Float(value));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.set(field.name(), FieldValue::Signed(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.set(field.name(), FieldValue::Unsigned(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.set(field.name(), FieldValue::Bool(value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.set(field.name(), FieldValue::Text(value.to_owned()));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        let mut text = String::new();
        if write!(text, "{value:?}").is_err() {
            text.push_str(UNFORMATTABLE);
        }
        self.set(field.name(), FieldValue::Formatted(text));
    }
}

/// Formatting numbers into a `String` cannot fail.
pub(crate) fn push_formatted(line: &mut String, arguments: fmt::Arguments<'_>) {
    let _ = line.write_fmt(arguments);
}

#[cfg(test)]
mod tests {
    use super::{FieldValue, Fields};

    #[test]
    fn recording_a_field_again_replaces_it_in_place() {
        let mut fields = Fields::default();
        fields.set("first", FieldValue::Signed(1));
        fields.set("second", FieldValue::Bool(true));

        fields.set("first", FieldValue::Text("again".to_owned()));

        let rendered: Vec<_> = fields
            .iter()
            .map(|(name, value)| (name, value.to_string()))
            .collect();
        assert_eq!(
            rendered,
            [("first", "again".to_owned()), ("second", "true".to_owned())]
        );
    }
}
