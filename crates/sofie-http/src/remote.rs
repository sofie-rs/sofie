//! What a split app serves the apps it left: contracts, batch lookups and its event feed, each
//! call proven by a principal token signed with the keys every app of the deployment shares.

use std::collections::HashMap;
use std::ops::ControlFlow;
use std::sync::Arc;

use http::{Method, StatusCode};
use hyper::body::Incoming;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sofie_auth::{Acting, SigningKeys};
use sofie_core::{Error, Result, Uuid};
use sofie_db::remote::{CONTRACTS_PATH, FEED_PATH, LOOKUPS_PATH, PRINCIPAL_HEADER, lookup_name};
use sofie_db::{BoxFuture, Db, Lookup};

use crate::answer::{self, JSON, Response};
use crate::caller::Caller;
use crate::pipeline::{Asked, read_body};
use crate::routes::BODY_LIMIT;

const UNVERIFIED: &str = "The call between apps could not be verified.";

/// Written by `#[contract]` for every contract trait, so a split app can serve it.
pub trait Serve {
    /// `<contract crate>/<trait>`, the same in the app that calls and the one that serves.
    const CONTRACT: &'static str;

    fn serve<'a>(&'a self, method: &'a str, arguments: Value) -> BoxFuture<'a, Result<Value>>;
}

pub fn arguments<T: DeserializeOwned>(arguments: Value) -> Result<T> {
    serde_json::from_value(arguments).map_err(|error| {
        Error::BadRequest(format!(
            "the arguments of a contract call do not fit: {error}"
        ))
    })
}

pub fn encoded<T: Serialize>(arguments: &T) -> Result<Value> {
    serde_json::to_value(arguments)
        .map_err(|error| Error::internal("serializing a contract call's arguments", error))
}

pub fn answered<T: Serialize>(answer: &T) -> Result<Value> {
    serde_json::to_value(answer)
        .map_err(|error| Error::internal("serializing a contract's answer", error))
}

#[must_use]
pub fn unknown_method(contract: &str, method: &str) -> Error {
    Error::BadRequest(format!("`{contract}` has no method `{method}`"))
}

type Dispatch = Arc<dyn Fn(&Db, &str, Value) -> BoxFuture<'static, Result<Value>> + Send + Sync>;

#[derive(Default)]
pub struct Served {
    contracts: HashMap<&'static str, Dispatch>,
    lookups: HashMap<&'static str, Dispatch>,
    feed: bool,
}

/// Looked up only for a path under `/_sofie/`, so no other request pays for it.
pub(crate) async fn answer(
    asked: Asked<'_>,
    db: &Db,
    keys: &SigningKeys,
    body: &mut Incoming,
) -> ControlFlow<Response> {
    if !asked.request.uri.path().starts_with("/_sofie/") {
        return ControlFlow::Continue(());
    }
    match sofie_db::provided::<Served>(db) {
        Some(served) => served.answer(asked, db, keys, body).await,
        None => ControlFlow::Continue(()),
    }
}

impl Served {
    #[must_use]
    pub fn contract<T: ?Sized + Serve + Send + Sync + 'static>(mut self) -> Self {
        let dispatch: Dispatch = Arc::new(|db, method, arguments| {
            let implementation = sofie_db::provided::<T>(db);
            let method = method.to_owned();
            Box::pin(async move {
                let implementation = implementation.ok_or_else(|| unprovided::<T>())?;
                implementation.serve(&method, arguments).await
            })
        });
        self.contracts.insert(T::CONTRACT, dispatch);
        self
    }

    #[must_use]
    pub fn lookup<D: Serialize + Send + 'static>(mut self) -> Self {
        let dispatch: Dispatch = Arc::new(|db, _, arguments| {
            let lookup = sofie_db::provided::<dyn Lookup<D>>(db);
            Box::pin(async move {
                let lookup = lookup.ok_or_else(unprovided::<dyn Lookup<D>>)?;
                let ids: Vec<Uuid> = self::arguments(arguments)?;
                answered(&lookup.get_many(&ids).await?)
            })
        });
        self.lookups.insert(lookup_name::<D>(), dispatch);
        self
    }

    #[must_use]
    pub fn feed(mut self) -> Self {
        self.feed = true;
        self
    }

    /// Answered under `/_sofie/`, never through an area; the reverse proxy keeps those paths inside.
    pub fn serve(self, db: &Db) {
        sofie_db::provide::<Self>(db, Arc::new(self));
    }

    async fn answer(
        &self,
        asked: Asked<'_>,
        db: &Db,
        keys: &SigningKeys,
        body: &mut Incoming,
    ) -> ControlFlow<Response> {
        let request = asked.request;
        let path = request.uri.path();
        let call = if let Some(rest) = path.strip_prefix(CONTRACTS_PATH) {
            rest.rsplit_once('/')
                .and_then(|(contract, method)| Some((self.contracts.get(contract)?, method)))
        } else if let Some(name) = path.strip_prefix(LOOKUPS_PATH) {
            self.lookups.get(name).map(|dispatch| (dispatch, ""))
        } else if path == FEED_PATH && self.feed && request.method == Method::GET {
            let feed = verified(asked, db, keys).map(|_| feed(db, request.uri.query()));
            return ControlFlow::Break(match feed {
                Ok(feed) => verified_answer(json(feed.await)),
                Err(refused) => *refused,
            });
        } else {
            return ControlFlow::Continue(());
        };
        let Some((dispatch, method)) = call.filter(|_| request.method == Method::POST) else {
            return ControlFlow::Break(answer::failure(Error::NotFound, Caller::Json));
        };
        let acting = match verified(asked, db, keys) {
            Ok(acting) => acting,
            Err(refused) => return ControlFlow::Break(*refused),
        };
        let body = match read_body(body, BODY_LIMIT, Caller::Json).await {
            Ok(body) => body,
            Err(refused) => return ControlFlow::Break(verified_answer(*refused)),
        };
        let called = async {
            let arguments = serde_json::from_slice(&body).map_err(|error| {
                Error::BadRequest(format!("a contract call's body is not JSON: {error}"))
            })?;
            sofie_db::remote::acting_as(acting, dispatch(db, method, arguments)).await
        };
        ControlFlow::Break(verified_answer(json(called.await)))
    }
}

fn verified_answer(response: Response) -> Response {
    #[cfg(feature = "shield")]
    return crate::shield::unobserved(response);
    #[cfg(not(feature = "shield"))]
    response
}

fn verified(
    asked: Asked<'_>,
    db: &Db,
    keys: &SigningKeys,
) -> std::result::Result<Option<Acting>, Box<Response>> {
    let request = asked.request;
    let token = request
        .headers
        .get(PRINCIPAL_HEADER)
        .and_then(|token| token.to_str().ok())
        .unwrap_or_default();
    let now = db.clock().now().timestamp();
    keys.verify_principal_token(token, request.uri.path(), now)
        .map_err(|refused| {
            tracing::warn!(
                http.route = request.uri.path(),
                error.kind = "principal_token",
                error = %refused,
                "refused a call between apps"
            );
            Box::new(answer::refusal(
                StatusCode::UNAUTHORIZED,
                UNVERIFIED,
                Caller::Json,
            ))
        })
}

async fn feed(db: &Db, query: Option<&str>) -> Result<Value> {
    let number = |name: &str| {
        query
            .unwrap_or_default()
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .find(|(key, _)| *key == name)
            .map(|(_, value)| {
                value
                    .parse::<i64>()
                    .map_err(|_| Error::BadRequest(format!("`{name}` is not a whole number")))
            })
            .transpose()
    };
    let after = number("after")?.unwrap_or(0);
    let limit = number("limit")?.unwrap_or(sofie_db::FEED_PAGE);
    answered(&sofie_db::read_feed(db, after, limit).await?)
}

fn json(result: Result<Value>) -> Response {
    match result {
        Ok(value) => answer::response(StatusCode::OK, Some(JSON), value.to_string()),
        Err(error) => answer::failure(error, Caller::Json),
    }
}

fn unprovided<T: ?Sized>() -> Error {
    Error::internal_message(format!(
        "nothing provides {}; run `sofie gen` so the wiring does",
        std::any::type_name::<T>()
    ))
}
