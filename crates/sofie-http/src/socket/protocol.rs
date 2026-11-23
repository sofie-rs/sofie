use bytes::Bytes;
use http::header::{
    ACCEPT, ACCEPT_ENCODING, CONNECTION, CONTENT_LENGTH, CONTENT_TYPE, IF_MODIFIED_SINCE,
    IF_NONE_MATCH, SEC_WEBSOCKET_EXTENSIONS, SEC_WEBSOCKET_KEY, SEC_WEBSOCKET_PROTOCOL,
    SEC_WEBSOCKET_VERSION, TRANSFER_ENCODING, UPGRADE,
};
use http::uri::PathAndQuery;
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Version};
use serde::Deserialize;

use crate::answer::JSON;
use crate::inbound::Inbound;

/// Name the socket's handshake or a body other than a message's own JSON, or would turn its
/// answer into a `304` or a compressed one.
const NOT_FORWARDED: [HeaderName; 13] = [
    CONNECTION,
    UPGRADE,
    SEC_WEBSOCKET_KEY,
    SEC_WEBSOCKET_VERSION,
    SEC_WEBSOCKET_EXTENSIONS,
    SEC_WEBSOCKET_PROTOCOL,
    ACCEPT,
    ACCEPT_ENCODING,
    CONTENT_TYPE,
    CONTENT_LENGTH,
    TRANSFER_ENCODING,
    IF_NONE_MATCH,
    IF_MODIFIED_SINCE,
];

#[derive(Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub(crate) enum Message {
    Stop {
        stop: u64,
    },
    Call {
        id: u64,
        method: Verb,
        path: String,
        #[serde(default)]
        body: Option<serde_json::Value>,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub(crate) enum Verb {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl Verb {
    fn method(self) -> Method {
        match self {
            Self::Get => Method::GET,
            Self::Post => Method::POST,
            Self::Put => Method::PUT,
            Self::Patch => Method::PATCH,
            Self::Delete => Method::DELETE,
        }
    }
}

/// The handshake's headers each message's request carries: its cookie, origin, host,
/// language and the client address a proxy forwarded.
pub(crate) fn forwarded(handshake: &HeaderMap) -> HeaderMap {
    let mut headers = handshake.clone();
    for name in &NOT_FORWARDED {
        headers.remove(name);
    }
    headers.insert(ACCEPT, HeaderValue::from_static(JSON));
    headers
}

/// A path of this app only: `//host/…` would name another.
pub(crate) fn path(path: &str) -> Option<PathAndQuery> {
    if !path.starts_with('/') || path.starts_with("//") {
        return None;
    }
    PathAndQuery::try_from(path).ok()
}

pub(crate) fn request(
    headers: &HeaderMap,
    verb: Verb,
    path: PathAndQuery,
    body: Option<&serde_json::Value>,
) -> http::Request<Inbound> {
    let body = body.map(|body| Bytes::from(body.to_string()));
    let mut request = http::Request::new(Inbound::Whole(body.clone()));
    *request.method_mut() = verb.method();
    *request.uri_mut() = http::Uri::from(path);
    *request.version_mut() = Version::HTTP_11;
    *request.headers_mut() = headers.clone();
    if let Some(body) = body {
        let headers = request.headers_mut();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static(JSON));
        headers.insert(CONTENT_LENGTH, HeaderValue::from(body.len()));
    }
    request
}

pub(crate) fn reply(
    id: u64,
    status: StatusCode,
    content_type: Option<&HeaderValue>,
    body: &[u8],
) -> String {
    let is_json = content_type
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with(JSON));
    let body = match (body.is_empty(), is_json) {
        (true, _) => "null".to_owned(),
        (false, true) => String::from_utf8_lossy(body).into_owned(),
        (false, false) => {
            serde_json::Value::String(String::from_utf8_lossy(body).into_owned()).to_string()
        }
    };
    format!(
        "{{\"id\":{id},\"status\":{},\"body\":{body}}}",
        status.as_u16()
    )
}

pub(crate) fn refused(id: u64, status: StatusCode, text: &str) -> String {
    let body = serde_json::Value::String(text.to_owned());
    format!(
        "{{\"id\":{id},\"status\":{},\"body\":{body}}}",
        status.as_u16()
    )
}

#[cfg(test)]
mod tests {
    use http::header::{COOKIE, ORIGIN};

    use super::*;

    #[test]
    fn a_message_is_a_call_or_a_stop() {
        let call: Message =
            serde_json::from_str(r#"{"id":2,"method":"POST","path":"/orders","body":{"n":1}}"#)
                .unwrap();
        assert_eq!(
            call,
            Message::Call {
                id: 2,
                method: Verb::Post,
                path: "/orders".to_owned(),
                body: Some(serde_json::json!({"n": 1})),
            }
        );
        assert_eq!(
            serde_json::from_str::<Message>(r#"{"stop":2}"#).unwrap(),
            Message::Stop { stop: 2 }
        );
        assert!(
            serde_json::from_str::<Message>(r#"{"id":1,"method":"TRACE","path":"/"}"#).is_err()
        );
    }

    #[test]
    fn only_a_path_of_this_app_is_called() {
        assert!(path("/orders?page=2").is_some());
        assert!(path("//elsewhere.test/orders").is_none());
        assert!(path("https://elsewhere.test/").is_none());
        assert!(path("orders").is_none());
    }

    #[test]
    fn a_message_carries_the_handshake_s_cookie_but_not_its_upgrade() {
        let mut handshake = HeaderMap::new();
        handshake.insert(COOKIE, HeaderValue::from_static("session=sealed"));
        handshake.insert(ORIGIN, HeaderValue::from_static("https://shop.test"));
        handshake.insert(UPGRADE, HeaderValue::from_static("websocket"));
        handshake.insert(ACCEPT_ENCODING, HeaderValue::from_static("br"));

        let headers = forwarded(&handshake);
        let request = request(
            &headers,
            Verb::Post,
            path("/orders").unwrap(),
            Some(&serde_json::json!({})),
        );

        let sent = request.headers();
        assert_eq!(sent[COOKIE], "session=sealed");
        assert_eq!(sent[ORIGIN], "https://shop.test");
        assert_eq!(sent[ACCEPT], JSON);
        assert_eq!(sent[CONTENT_TYPE], JSON);
        assert!(!sent.contains_key(UPGRADE) && !sent.contains_key(ACCEPT_ENCODING));
    }

    #[test]
    fn a_reply_keeps_json_as_it_was_rendered_and_quotes_other_text() {
        let json = HeaderValue::from_static("application/json");
        let text = HeaderValue::from_static("text/plain; charset=utf-8");

        assert_eq!(
            reply(1, StatusCode::OK, Some(&json), br#"{"a":1}"#),
            r#"{"id":1,"status":200,"body":{"a":1}}"#
        );
        assert_eq!(
            reply(1, StatusCode::NOT_FOUND, Some(&text), b"Not \"found\"."),
            r#"{"id":1,"status":404,"body":"Not \"found\"."}"#
        );
        assert_eq!(
            reply(1, StatusCode::NO_CONTENT, None, b""),
            r#"{"id":1,"status":204,"body":null}"#
        );
    }
}
