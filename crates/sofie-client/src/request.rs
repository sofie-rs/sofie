use bytes::Bytes;
use http::header::{AUTHORIZATION, CONTENT_TYPE, HeaderName};
use http::{HeaderMap, HeaderValue, Method, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sofie_core::encoding::percent;
use sofie_core::{Error, Result};
use sofie_db::IdempotencyKey;

use crate::Outgoing;
use crate::exchange::Exchange;
use crate::guard::Reach;
use crate::target::Target;

const IDEMPOTENCY_KEY: HeaderName = HeaderName::from_static("idempotency-key");

/// A mistake while building, such as an invalid header, is returned by [`Request::send`].
pub struct Request<'a> {
    outgoing: &'a Outgoing,
    method: Method,
    target: Result<Target>,
    reach: Reach,
    headers: HeaderMap,
    body: Bytes,
    keyed: bool,
    problem: Option<Error>,
}

impl<'a> Request<'a> {
    pub(crate) fn new(
        outgoing: &'a Outgoing,
        method: Method,
        target: Result<Target>,
        reach: Reach,
    ) -> Self {
        Self {
            outgoing,
            method,
            target,
            reach,
            headers: HeaderMap::new(),
            body: Bytes::new(),
            keyed: false,
            problem: None,
        }
    }

    /// Header values are never recorded in spans or logs.
    #[must_use]
    pub fn header(mut self, name: &str, value: &str) -> Self {
        let Ok(header) = HeaderName::from_bytes(name.as_bytes()) else {
            self.fail(&format!("`{name}` is not a valid header name"));
            return self;
        };
        match HeaderValue::from_str(value) {
            Ok(mut value) => {
                value.set_sensitive(true);
                self.headers.insert(header, value);
            }
            Err(_) => self.fail(&format!("the value of header `{name}` is not valid")),
        }
        self
    }

    #[must_use]
    pub fn bearer(self, token: &str) -> Self {
        self.header(AUTHORIZATION.as_str(), &format!("Bearer {token}"))
    }

    #[must_use]
    pub fn json<T: Serialize + ?Sized>(mut self, body: &T) -> Self {
        match serde_json::to_vec(body) {
            Ok(json) => self.body("application/json", json),
            Err(error) => {
                self.fail(&format!("serializing the body failed: {error}"));
                self
            }
        }
    }

    /// `application/x-www-form-urlencoded`, as some vendors' APIs take it.
    #[must_use]
    pub fn form(self, fields: &[(&str, &str)]) -> Self {
        let body = percent::form(fields.iter().copied());
        self.body("application/x-www-form-urlencoded", body)
    }

    #[must_use]
    pub fn body(mut self, content_type: &str, body: impl Into<Bytes>) -> Self {
        match HeaderValue::from_str(content_type) {
            Ok(value) => {
                self.body = body.into();
                self.headers.insert(CONTENT_TYPE, value);
            }
            Err(_) => self.fail(&format!("`{content_type}` is not a valid content type")),
        }
        self
    }

    #[must_use]
    pub fn idempotency(mut self, key: &IdempotencyKey) -> Self {
        match HeaderValue::from_str(key.as_str()) {
            Ok(value) => {
                self.headers.insert(IDEMPOTENCY_KEY, value);
                self.keyed = true;
            }
            Err(_) => self.fail("the idempotency key is not a valid header value"),
        }
        self
    }

    /// A `POST` the vendor applies once however often it arrives, such as a search, is retried
    /// as a `GET` would be.
    #[must_use]
    pub fn repeatable(mut self) -> Self {
        self.keyed = true;
        self
    }

    pub async fn send(self) -> Result<Response> {
        if let Some(problem) = self.problem {
            return Err(problem);
        }
        let target = self.target?;
        sofie_db::read_untracked();
        let exchange = Exchange {
            shared: &self.outgoing.shared,
            method: self.method,
            reach: self.reach,
            headers: self.headers,
            body: self.body,
            keyed: self.keyed,
        };
        // Boxed: the connection's futures would otherwise make every caller's future kilobytes larger.
        Box::pin(exchange.send(target)).await
    }

    fn fail(&mut self, message: &str) {
        self.problem.get_or_insert_with(|| {
            Error::internal_message(format!(
                "building a request to {}: {message}",
                self.outgoing.shared.vendor
            ))
        });
    }
}

#[derive(Debug)]
pub struct Response {
    pub(crate) vendor: &'static str,
    pub(crate) status: StatusCode,
    pub(crate) headers: HeaderMap,
    pub(crate) body: Bytes,
}

impl Response {
    #[must_use]
    pub fn status(&self) -> u16 {
        self.status.as_u16()
    }

    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name)?.to_str().ok()
    }

    #[must_use]
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    #[must_use]
    pub fn into_body(self) -> Bytes {
        self.body
    }

    /// The body is never put in the error: it may hold what a log must not.
    pub fn json<T: DeserializeOwned>(&self) -> Result<T> {
        if !self.status.is_success() {
            return Err(Error::internal_message(format!(
                "{} answered {}",
                self.vendor, self.status
            )));
        }
        serde_json::from_slice(&self.body).map_err(|error| {
            Error::internal(format!("reading {}'s answer as JSON", self.vendor), error)
        })
    }
}
