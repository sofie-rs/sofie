use sofie_core::{Error, FieldError};

/// Keyed, so the viewer's language replaces `english`; the key's English text in sofie-engine's
/// `lang/en.toml` must read exactly as `english` once its arguments are filled.
pub(crate) fn refusal(field: &str, key: &'static str, english: impl Into<String>) -> Error {
    refusal_of(FieldError::new(field, key, english))
}

pub(crate) fn refusal_of(error: FieldError) -> Error {
    Error::Invalid(vec![error])
}
