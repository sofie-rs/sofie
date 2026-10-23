//! The calling side of a split: contracts, batch lookups and the event feed of another app.

use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sofie_auth::SigningKeys;
use sofie_core::{Error, FieldError, Result, Timestamp, Uuid};
use sofie_db::remote::{CONTRACTS_PATH, FEED_PATH, LOOKUPS_PATH, PRINCIPAL_HEADER, lookup_name};
use sofie_db::{BoxFuture, FeedEntry, FeedSource, Lookup};
use tracing::Instrument;

use crate::{Outgoing, Request, Response};

#[derive(Clone)]
pub struct RemoteApp {
    shared: Arc<Connection>,
}

struct Connection {
    name: &'static str,
    outgoing: Outgoing,
    keys: SigningKeys,
}

impl RemoteApp {
    /// The wiring's constructor: `name` is the split module, whose keys name the app.
    #[doc(hidden)]
    #[must_use]
    pub fn new(name: &'static str, outgoing: Outgoing, keys: SigningKeys) -> Self {
        Self {
            shared: Arc::new(Connection {
                name,
                outgoing,
                keys,
            }),
        }
    }

    #[must_use]
    pub fn name(&self) -> &'static str {
        self.shared.name
    }

    async fn post<R: DeserializeOwned>(&self, path: &str, arguments: Result<Value>) -> Result<R> {
        let request = self.shared.outgoing.post(path).json(&arguments?);
        self.send(request, path).await
    }

    /// The principal token carries the principal the current work acts for, valid for `path` only.
    async fn send<R: DeserializeOwned>(&self, request: Request<'_>, path: &str) -> Result<R> {
        let acting = sofie_db::remote::acting();
        let token = self
            .shared
            .keys
            .principal_token(path, acting.as_ref(), Timestamp::now());
        let response = request.header(PRINCIPAL_HEADER, &token).send().await?;
        if response.status() == 200 {
            return response.json();
        }
        Err(refused(self.shared.name, &response))
    }
}

/// `{"error": …}`, or `{"errors": [{"field", "key", "arguments", "message"}]}` for invalid input.
#[derive(Default, Deserialize)]
#[serde(default)]
struct Refusal {
    error: String,
    errors: Option<Vec<RefusedField>>,
}

/// Keyed so the calling app translates the refusal as if it had raised it.
#[derive(Deserialize)]
struct RefusedField {
    field: String,
    /// An app deployed before keys travelled sends none; its message then stands as written.
    #[serde(default = "unkeyed")]
    key: String,
    #[serde(default)]
    arguments: BTreeMap<String, String>,
    message: String,
}

/// The error the serving app answered with, so a caller handles it as an in-process one.
fn refused(app: &str, response: &Response) -> Error {
    let refusal: Refusal = serde_json::from_slice(response.body()).unwrap_or_default();
    match response.status() {
        400 => Error::BadRequest(refusal.error),
        404 => Error::NotFound,
        409 => Error::Conflict,
        422 => match refusal.errors {
            Some(fields) => Error::Invalid(
                fields
                    .into_iter()
                    .map(RefusedField::into_field_error)
                    .collect(),
            ),
            None => Error::Rule(refusal.error),
        },
        429 => Error::TooManyRequests(retry_after(response)),
        503 => Error::Unavailable(format!("{app} is unavailable")),
        status => Error::internal_message(format!(
            "{app} answered {status}; its own trace holds the reason"
        )),
    }
}

fn unkeyed() -> String {
    "remote".to_owned()
}

impl RefusedField {
    fn into_field_error(self) -> FieldError {
        self.arguments.into_iter().fold(
            FieldError::new(self.field, self.key, self.message),
            |error, (name, value)| error.with(name, value),
        )
    }
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Seconds, or an HTTP date (`Sun, 06 Nov 1994 08:49:37 GMT`) after which to try again.
fn retry_after(response: &Response) -> Duration {
    let Some(value) = response.header("retry-after") else {
        return Duration::from_secs(1);
    };
    if let Ok(seconds) = value.trim().parse() {
        return Duration::from_secs(seconds);
    }
    http_date(value).map_or(Duration::from_secs(1), |at| {
        Duration::try_from(at.duration_since(Timestamp::now())).unwrap_or(Duration::ZERO)
    })
}

fn http_date(text: &str) -> Option<Timestamp> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let [_weekday, day, month, year, time, "GMT"] = words.as_slice() else {
        return None;
    };
    let month = MONTHS.iter().position(|name| name == month)? + 1;
    format!("{year}-{month:02}-{day}T{time}Z").parse().ok()
}

/// `#[contract]` implements each contract trait `T` for `Remote<dyn T>`, calling these.
pub struct Remote<T: ?Sized> {
    app: RemoteApp,
    contract: PhantomData<fn() -> Box<T>>,
}

impl<T: ?Sized> Remote<T> {
    #[must_use]
    pub fn new(app: RemoteApp) -> Self {
        Self {
            app,
            contract: PhantomData,
        }
    }

    /// `contract` is `<contract crate>/<trait>`; the arguments are serialized before the call.
    pub fn call<'a, R: DeserializeOwned + Send + 'a>(
        &'a self,
        contract: &'static str,
        method: &'static str,
        arguments: Result<Value>,
    ) -> BoxFuture<'a, Result<R>> {
        let name = contract.rsplit_once('/').map_or(contract, |(_, name)| name);
        let span = tracing::info_span!(
            "contract",
            otel.name = format!("{name}::{method}"),
            contract.app = self.app.name(),
        );
        let path = format!("{CONTRACTS_PATH}{contract}/{method}");
        Box::pin(async move { self.app.post(&path, arguments).await }.instrument(span))
    }
}

pub struct RemoteLookup<D> {
    app: RemoteApp,
    dto: PhantomData<fn() -> D>,
}

impl<D> RemoteLookup<D> {
    #[must_use]
    pub fn new(app: RemoteApp) -> Self {
        Self {
            app,
            dto: PhantomData,
        }
    }
}

impl<D: DeserializeOwned + Send> Lookup<D> for RemoteLookup<D> {
    fn get_many<'a>(&'a self, ids: &'a [Uuid]) -> BoxFuture<'a, Result<Vec<D>>> {
        let path = format!("{LOOKUPS_PATH}{}", lookup_name::<D>());
        let ids = serde_json::to_value(ids)
            .map_err(|error| Error::internal("serializing the ids of a lookup", error));
        Box::pin(async move {
            let request = self.app.shared.outgoing.post(&path).json(&ids?);
            self.app.send(request.repeatable(), &path).await
        })
    }
}

pub struct RemoteFeed {
    app: RemoteApp,
}

impl RemoteFeed {
    #[must_use]
    pub fn new(app: RemoteApp) -> Self {
        Self { app }
    }
}

impl FeedSource for RemoteFeed {
    fn producer(&self) -> &str {
        self.app.name()
    }

    fn after(&self, position: i64, limit: i64) -> BoxFuture<'_, Result<Vec<FeedEntry>>> {
        let page = format!("{FEED_PATH}?after={position}&limit={limit}");
        Box::pin(async move {
            let request = self.app.shared.outgoing.get(&page);
            self.app.send(request, FEED_PATH).await
        })
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use http::{HeaderMap, StatusCode};

    use super::*;

    fn answered(status: u16, body: &str) -> Error {
        let response = Response {
            vendor: "billing",
            status: StatusCode::from_u16(status).unwrap(),
            headers: HeaderMap::new(),
            body: Bytes::from(body.to_owned()),
        };
        refused("billing", &response)
    }

    #[test]
    fn a_refusal_is_read_back_into_the_error_the_serving_app_raised() {
        let invalid = answered(
            422,
            r#"{"errors":[{"field":"email","key":"validation.unique","arguments":{},"message":"is taken"}]}"#,
        );
        let Error::Invalid(fields) = invalid else {
            panic!("invalid input: {invalid:?}");
        };

        assert_eq!(
            fields,
            [FieldError::new("email", "validation.unique", "is taken")]
        );
        assert!(
            matches!(answered(422, r#"{"error":"closed"}"#), Error::Rule(message) if message == "closed")
        );
        assert!(
            matches!(answered(400, "not json"), Error::BadRequest(message) if message.is_empty())
        );
        assert!(matches!(answered(404, ""), Error::NotFound));
    }

    #[test]
    fn a_refused_field_keeps_its_key_and_arguments() {
        let invalid = answered(
            422,
            r#"{"errors":[{"field":"code","key":"validation.max_length","arguments":{"max":"4"},"message":"must be at most 4"}]}"#,
        );
        let Error::Invalid(fields) = invalid else {
            panic!("invalid input: {invalid:?}");
        };

        assert_eq!(
            fields,
            [
                FieldError::new("code", "validation.max_length", "must be at most 4")
                    .with("max", "4")
            ]
        );
    }

    #[test]
    fn a_refused_field_from_an_app_that_sends_no_key_keeps_its_message() {
        let invalid = answered(
            422,
            r#"{"errors":[{"field":"email","message":"is taken"}]}"#,
        );
        let Error::Invalid(fields) = invalid else {
            panic!("invalid input: {invalid:?}");
        };

        assert_eq!(fields, [FieldError::new("email", "remote", "is taken")]);
    }

    fn told_to_wait(retry_after: &str) -> Duration {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", retry_after.parse().unwrap());
        let response = Response {
            vendor: "billing",
            status: StatusCode::TOO_MANY_REQUESTS,
            headers,
            body: Bytes::new(),
        };
        let Error::TooManyRequests(wait) = refused("billing", &response) else {
            panic!("a 429 is too many requests");
        };
        wait
    }

    #[test]
    fn retry_after_is_read_as_seconds_or_as_a_date() {
        let in_two_minutes = Timestamp::now() + Duration::from_secs(120);
        let date = in_two_minutes
            .strftime("%a, %d %b %Y %H:%M:%S GMT")
            .to_string();

        let waited = told_to_wait(&date);

        assert!(
            (Duration::from_secs(100)..=Duration::from_secs(120)).contains(&waited),
            "{waited:?}"
        );
        assert_eq!(told_to_wait("7"), Duration::from_secs(7));
        assert_eq!(
            told_to_wait("Sun, 06 Nov 1994 08:49:37 GMT"),
            Duration::ZERO
        );
        assert_eq!(told_to_wait("soon"), Duration::from_secs(1));
    }
}
