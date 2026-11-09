use std::borrow::Cow;
use std::fmt;
use std::time::Duration;

#[derive(Debug)]
pub enum Error {
    /// Also answered when the principal may not know that the record exists.
    NotFound,
    /// Asked for by a name it had before, such as a slug: a GET is sent on to `to`.
    Moved {
        from: String,
        to: String,
    },
    Conflict,
    BadRequest(String),
    Invalid(Vec<FieldError>),
    Rule(String),
    KeyedRule(Box<KeyedRule>),
    TooManyRequests(Duration),
    Unavailable(String),
    Internal {
        context: String,
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldError {
    pub field: String,
    pub key: Cow<'static, str>,
    pub arguments: Vec<(Cow<'static, str>, String)>,
    pub message: String,
}

impl FieldError {
    pub fn new(
        field: impl Into<String>,
        key: impl Into<Cow<'static, str>>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            field: field.into(),
            key: key.into(),
            arguments: Vec::new(),
            message: message.into(),
        }
    }

    pub fn required(field: impl Into<String>) -> Self {
        Self::new(field, REQUIRED, "is required")
    }

    pub fn invalid(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(field, INVALID, message)
    }

    #[must_use]
    pub fn with(mut self, name: impl Into<Cow<'static, str>>, value: impl Into<String>) -> Self {
        self.arguments.push((name.into(), value.into()));
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyedRule {
    pub key: Cow<'static, str>,
    pub arguments: Vec<(Cow<'static, str>, String)>,
    pub message: String,
}

#[derive(Clone, Copy, Debug)]
pub struct RuleText {
    key: &'static str,
    message: &'static str,
}

impl RuleText {
    #[must_use]
    pub const fn new(key: &'static str, message: &'static str) -> Self {
        Self { key, message }
    }
}

impl From<RuleText> for Error {
    fn from(rule: RuleText) -> Self {
        Self::rule_keyed(rule.key, [], rule.message)
    }
}

const INVALID: &str = "validation.invalid";
const REQUIRED: &str = "validation.required";

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub fn internal(
        context: impl Into<String>,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        Self::Internal {
            context: context.into(),
            source: Some(source.into()),
        }
    }

    pub fn internal_message(context: impl Into<String>) -> Self {
        Self::Internal {
            context: context.into(),
            source: None,
        }
    }

    pub fn invalid(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Invalid(vec![FieldError::invalid(field, message)])
    }

    pub fn rule_keyed(
        key: &'static str,
        arguments: impl IntoIterator<Item = (&'static str, String)>,
        message: impl Into<String>,
    ) -> Self {
        Self::KeyedRule(Box::new(KeyedRule {
            key: Cow::Borrowed(key),
            arguments: arguments
                .into_iter()
                .map(|(name, value)| (Cow::Borrowed(name), value))
                .collect(),
            message: message.into(),
        }))
    }

    /// Alternate form: the field being read already names the value.
    #[must_use]
    pub fn into_deserialize_error<E: serde::de::Error>(self) -> E {
        E::custom(format_args!("{self:#}"))
    }

    fn fmt_message(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadRequest(message) | Self::Rule(message) => f.write_str(message),
            Self::KeyedRule(rule) => f.write_str(&rule.message),
            Self::Invalid(errors) => {
                let messages: Vec<&str> =
                    errors.iter().map(|error| error.message.as_str()).collect();
                f.write_str(&messages.join("; "))
            }
            _ => write!(f, "{self}"),
        }
    }
}

/// `{:#}` prints the message alone, as shown under a field.
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if f.alternate() {
            return self.fmt_message(f);
        }
        match self {
            Self::NotFound => f.write_str("not found"),
            Self::Moved { from, to } => write!(f, "moved from `{from}` to `{to}`"),
            Self::Conflict => f.write_str("the record changed since it was loaded"),
            Self::BadRequest(message) => write!(f, "bad request: {message}"),
            Self::Invalid(errors) => {
                f.write_str("invalid input:")?;
                for error in errors {
                    write!(f, " {}: {};", error.field, error.message)?;
                }
                Ok(())
            }
            Self::Rule(message) => write!(f, "refused: {message}"),
            Self::KeyedRule(rule) => write!(f, "refused: {}", rule.message),
            Self::TooManyRequests(wait) => {
                write!(f, "too many requests; try again in {wait:?}")
            }
            Self::Unavailable(message) => write!(f, "unavailable: {message}"),
            Self::Internal { context, source } => match source {
                Some(source) => write!(f, "{context}: {source}"),
                None => f.write_str(context),
            },
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Internal {
                source: Some(source),
                ..
            } => Some(source.as_ref()),
            _ => None,
        }
    }
}
