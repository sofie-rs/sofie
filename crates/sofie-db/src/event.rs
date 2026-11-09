use serde::Serialize;

use crate::Input;

pub trait Event: Input + Serialize + Send + 'static {
    /// `<crate>.<Type>`: what it is stored and delivered as.
    const NAME: &'static str;
}

/// No field named in Rust can take it, and `Meta::raise` refuses an app event that does.
pub(crate) const PRUNED: &str = "sofie.pruned";

#[cfg(any(feature = "contracts", feature = "publishing", feature = "retention"))]
pub(crate) fn change_payload(
    mut fields: serde_json::Map<String, serde_json::Value>,
    id: sofie_core::Uuid,
    version: i64,
) -> serde_json::Map<String, serde_json::Value> {
    fields.insert("id".to_owned(), serde_json::Value::String(id.to_string()));
    fields.insert("version".to_owned(), serde_json::Value::from(version));
    fields
}

pub(crate) fn is_pruned(payload: &serde_json::Value) -> bool {
    payload.get(PRUNED) == Some(&serde_json::Value::Bool(true))
}
