//! An app without forms reads JSON only.

use serde_json::{Map, Value};
use sofie_core::{Error, Result};

pub(crate) fn parse(_body: &[u8]) -> Result<Map<String, Value>> {
    Err(Error::BadRequest(
        "this app reads JSON request bodies only".to_owned(),
    ))
}
