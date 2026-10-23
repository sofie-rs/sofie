use std::path::Path;

use serde_json::{Value, json};

use super::instructions::{SERVER_NAME, instructions};
use super::session::DevSession;
use super::tools;

/// Newest first; a client asking for another gets the newest.
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

pub(super) fn answer(root: &Path, session: &mut DevSession, line: &str) -> Option<String> {
    let message: Value = match serde_json::from_str(line) {
        Ok(message) => message,
        Err(error) => {
            return Some(failure(
                &Value::Null,
                PARSE_ERROR,
                &format!("not JSON: {error}"),
            ));
        }
    };
    if !message.is_object() {
        return Some(failure(
            &Value::Null,
            INVALID_REQUEST,
            "a request is one JSON object per line; `sofie mcp` answers no batch",
        ));
    }
    let id = message.get("id").cloned();
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return id.map(|id| failure(&id, INVALID_REQUEST, "a request names its `method`"));
    };
    let id = id?;
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    Some(match respond(root, session, method, &params) {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string(),
        Err((code, text)) => failure(&id, code, &text),
    })
}

pub(super) fn not_utf8() -> String {
    failure(&Value::Null, PARSE_ERROR, "the line is not UTF-8")
}

fn respond(
    root: &Path,
    session: &mut DevSession,
    method: &str,
    params: &Value,
) -> Result<Value, (i64, String)> {
    match method {
        "initialize" => Ok(initialize(params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools::definitions() })),
        "tools/call" => call(root, session, params),
        _ => Err((
            METHOD_NOT_FOUND,
            format!("`sofie mcp` has no method `{method}`"),
        )),
    }
}

fn initialize(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = PROTOCOL_VERSIONS
        .into_iter()
        .find(|supported| Some(*supported) == asked)
        .unwrap_or(PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
        "instructions": instructions(),
    })
}

fn call(root: &Path, session: &mut DevSession, params: &Value) -> Result<Value, (i64, String)> {
    let name = params.get("name").and_then(Value::as_str).ok_or((
        INVALID_PARAMS,
        "`tools/call` names its tool in `name`".to_owned(),
    ))?;
    let empty = serde_json::Map::new();
    let arguments = match params.get("arguments") {
        None | Some(Value::Null) => &empty,
        Some(Value::Object(arguments)) => arguments,
        Some(_) => {
            return Err((INVALID_PARAMS, "`arguments` is an object".to_owned()));
        }
    };
    let output =
        tools::call(root, session, name, arguments).map_err(|error| (INVALID_PARAMS, error))?;
    Ok(json!({
        "content": [{ "type": "text", "text": output.text }],
        "isError": output.failed,
    }))
}

fn failure(id: &Value, code: i64, message: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
    .to_string()
}
