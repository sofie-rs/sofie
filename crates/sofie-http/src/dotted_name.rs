use serde_json::{Map, Value};
use sofie_core::{Error, Result};

use crate::urlencoded::bad_form;

/// Forms nest one level; the bound keeps a hostile name from building a value too deep to drop.
const MAX_DEPTH: usize = 8;

pub(crate) fn insert(object: &mut Map<String, Value>, name: &str, value: String) -> Result<()> {
    let segments: Vec<&str> = name.split('.').collect();
    if segments.len() > MAX_DEPTH
        || !segments
            .iter()
            .all(|segment| sofie_core::is_field_name(segment))
    {
        return Err(bad_form(
            "a field name is not letters, digits and `_`, nested with `.`",
        ));
    }
    let Some((last, parents)) = segments.split_last() else {
        return Err(bad_form("a field has no name"));
    };
    let mut current = object;
    for parent in parents {
        let slot = current
            .entry(*parent)
            .or_insert_with(|| Value::Object(Map::new()));
        let Value::Object(nested) = slot else {
            return Err(value_and_group());
        };
        current = nested;
    }
    if current.get(*last).is_some_and(Value::is_object) {
        return Err(value_and_group());
    }
    current.insert((*last).to_owned(), Value::String(value));
    Ok(())
}

fn value_and_group() -> Error {
    bad_form("a field is posted both as a value and as a group of fields")
}
