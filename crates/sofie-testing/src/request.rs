use std::fmt;
use std::future::IntoFuture;
use std::pin::Pin;

use bytes::Bytes;
use http::header::{ACCEPT, CONTENT_TYPE, COOKIE, SET_COOKIE};
use http::request::Builder;
use http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use serde_json::Value;
use sofie_core::encoding::percent;
use sofie_http::CookieNames;

pub(crate) type HttpClient = Client<HttpConnector, Full<Bytes>>;

const JSON: &str = "application/json";

#[must_use = "a request is sent only when awaited"]
pub struct TestRequest {
    client: HttpClient,
    request: Builder,
    body: Bytes,
    cookies: CookieNames,
}

/// Its value is never printed.
#[derive(Clone, PartialEq, Eq)]
pub struct TestSession {
    cookie: String,
}

impl TestSession {
    pub(crate) fn new(cookie: String) -> Self {
        Self { cookie }
    }

    pub(crate) fn cookie(&self) -> &str {
        &self.cookie
    }
}

impl fmt::Debug for TestSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TestSession(…)")
    }
}

pub struct TestResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: String,
    cookies: CookieNames,
}

impl TestRequest {
    pub(crate) fn new(
        client: HttpClient,
        method: Method,
        url: String,
        cookies: CookieNames,
    ) -> Self {
        Self {
            client,
            request: Request::builder().method(method).uri(url),
            body: Bytes::new(),
            cookies,
        }
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        let name = HeaderName::try_from(name).expect("a valid header name");
        let value = HeaderValue::try_from(value).expect("a valid header value");
        if let Some(headers) = self.request.headers_mut() {
            headers.insert(name, value);
        }
        self
    }

    pub fn accept_json(self) -> Self {
        self.header(ACCEPT.as_str(), JSON)
    }

    pub fn json(self, body: &Value) -> Self {
        Self {
            body: Bytes::from(body.to_string()),
            ..self.header(CONTENT_TYPE.as_str(), JSON).accept_json()
        }
    }

    pub fn form(self, fields: &[(&str, &str)]) -> Self {
        Self {
            body: Bytes::from(percent::form(fields.iter().copied())),
            ..self.header(CONTENT_TYPE.as_str(), "application/x-www-form-urlencoded")
        }
    }

    pub fn body(self, body: impl Into<Bytes>) -> Self {
        Self {
            body: body.into(),
            ..self
        }
    }

    pub fn session(self, session: &TestSession) -> Self {
        self.cookie(session).same_origin()
    }

    pub fn cookie(self, session: &TestSession) -> Self {
        self.header(COOKIE.as_str(), &session.cookie)
    }

    pub fn htmx(self, target: &str) -> Self {
        self.header("hx-request", "true")
            .header("hx-target", target)
            .same_origin()
    }

    fn same_origin(self) -> Self {
        self.header("sec-fetch-site", "same-origin")
    }

    pub fn trigger(self, id: &str) -> Self {
        self.header("hx-trigger", id)
    }

    async fn send(self) -> TestResponse {
        let request = self
            .request
            .body(Full::new(self.body))
            .expect("building the test request");
        let response = self
            .client
            .request(request)
            .await
            .expect("sending the test request");
        let (parts, body) = response.into_parts();
        let body = body
            .collect()
            .await
            .expect("reading the test response")
            .to_bytes();
        let response = TestResponse {
            status: parts.status,
            headers: parts.headers,
            body: String::from_utf8_lossy(&body).into_owned(),
            cookies: self.cookies,
        };
        if let Some(error) = response.internal_error() {
            eprintln!("the request failed with {}: {error}", response.status);
        }
        response
    }
}

impl IntoFuture for TestRequest {
    type Output = TestResponse;
    type IntoFuture = Pin<Box<dyn Future<Output = TestResponse> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.send())
    }
}

const TEST_ERROR: &str = "x-sofie-test-error";

/// Header values are left out: a cookie's would print a session.
impl fmt::Debug for TestResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = self.headers.keys().map(http::HeaderName::as_str).collect();
        formatter
            .debug_struct("TestResponse")
            .field("status", &self.status)
            .field("headers", &names)
            .field("body", &self.body)
            .finish_non_exhaustive()
    }
}

impl TestResponse {
    #[must_use]
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body)
            .unwrap_or_else(|error| panic!("the body is not JSON ({error}): {}", self.body))
    }

    #[must_use]
    pub fn session(&self) -> Option<TestSession> {
        self.headers
            .get_all(SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .filter_map(|value| value.split(';').next())
            .find(|pair| {
                pair.strip_prefix(self.cookies.session())
                    .and_then(|rest| rest.strip_prefix('='))
                    .is_some_and(|value| !value.is_empty())
            })
            .map(|pair| TestSession {
                cookie: pair.to_owned(),
            })
    }

    pub fn assert_status(&self, status: u16) {
        assert_eq!(
            self.status.as_u16(),
            status,
            "{}{}",
            self.body,
            self.cause()
        );
    }

    pub fn assert_ok(&self) {
        let (status, body, cause) = (self.status, &self.body, self.cause());
        assert!(status.is_success(), "{status}: {body}{cause}");
    }

    /// Why a 500 failed, as the server logged it; a debug build's test server alone sends it.
    #[must_use]
    pub fn internal_error(&self) -> Option<&str> {
        self.headers.get(TEST_ERROR)?.to_str().ok()
    }

    fn cause(&self) -> String {
        self.internal_error()
            .map(|error| format!("\ncaused by: {error}"))
            .unwrap_or_default()
    }

    #[must_use]
    pub fn record_id(&self) -> sofie_core::Uuid {
        self.assert_ok();
        match self.json()["id"].as_str().map(str::parse) {
            Some(Ok(id)) => id,
            _ => panic!("the answer names no record by its id: {}", self.body),
        }
    }

    #[must_use]
    pub fn header(&self, name: &str) -> &str {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
    }
}
