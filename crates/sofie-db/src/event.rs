use serde::Serialize;

use crate::Input;

pub trait Event: Input + Serialize + Send + 'static {
    /// `<crate>.<Type>`: what it is stored and delivered as.
    const NAME: &'static str;
}

/// No field named in Rust can take it, and `Meta::raise` refuses an app event that does.
pub(crate) const PRUNED: &str = "sofie.pruned";

pub(crate) fn is_pruned(payload: &serde_json::Value) -> bool {
    payload.get(PRUNED) == Some(&serde_json::Value::Bool(true))
}
