use std::marker::PhantomData;

use bytes::Bytes;
use sofie_core::{Error, Result, Uuid};
use sofie_db::{Context, Db, Reach, Writes};

use crate::Area;
use crate::session::RequestSession;
use crate::urlencoded::Encoding;

#[derive(Clone)]
pub(crate) struct Matched {
    pub(crate) parameters: Vec<(String, String)>,
    pub(crate) query: Option<String>,
    pub(crate) body: Option<Bytes>,
    pub(crate) encoding: Encoding,
    pub(crate) session: RequestSession,
    /// Every `Read` and commit is held to it.
    pub(crate) reach: Reach,
    pub(crate) deadline: crate::deadline::Deadline,
    pub(crate) app: crate::AppName,
    pub(crate) route: Option<&'static str>,
    #[cfg(feature = "idempotency")]
    pub(crate) work: Option<sofie_db::Work>,
    #[cfg(feature = "signed-links")]
    pub(crate) keys: std::sync::Arc<sofie_auth::SigningKeys>,
    #[cfg(any(feature = "flags", feature = "permissions"))]
    pub(crate) gates: crate::gate::RequestGates,
    #[cfg(feature = "shield")]
    pub(crate) shield: Option<crate::shield::RequestShield>,
    #[cfg(feature = "storage")]
    pub(crate) storage: Option<sofie_storage::Storage>,
    /// A list's refused filter is then a 422; a page shows it beside the filter.
    #[cfg(feature = "listing")]
    pub(crate) prefers_json: bool,
}

// Kept off the per-area `RequestContext`, so each app compiles the body parse once.
impl Matched {
    fn bound(&self, name: &str) -> Result<Uuid> {
        bound_id(&self.parameters, name)
    }

    fn input(&mut self) -> Result<serde_json::Value> {
        let body = self.body.take().ok_or_else(body_already_read)?;
        if self.encoding == Encoding::Form {
            return crate::form::parse(&body).map(serde_json::Value::Object);
        }
        if body.is_empty() {
            return Ok(serde_json::Value::Object(serde_json::Map::new()));
        }
        serde_json::from_slice(&body).map_err(|error| {
            Error::BadRequest(format!("the request body is not valid JSON: {error}"))
        })
    }
}

#[cfg(feature = "engine")]
impl Matched {
    pub(crate) fn for_loader(&self) -> Self {
        Self {
            body: None,
            encoding: Encoding::Json,
            session: self.session.for_loader(),
            #[cfg(feature = "idempotency")]
            work: None,
            ..self.clone()
        }
    }
}

/// A malformed id is a URL that names nothing; a missing parameter is a
/// handler bound to a route without it.
fn bound_id(parameters: &[(String, String)], name: &str) -> Result<Uuid> {
    let value = parameter(parameters, name)
        .ok_or_else(|| Error::internal_message(format!("the route has no `{name}` parameter")))?;
    value.parse().map_err(|_| Error::NotFound)
}

/// Two extractors taking the body is a handler bug, not the request's.
pub(crate) fn body_already_read() -> Error {
    Error::internal_message("the request body was already read")
}

pub(crate) fn parameter<'p>(parameters: &'p [(String, String)], name: &str) -> Option<&'p str> {
    let (_, value) = parameters.iter().find(|(given, _)| given == name)?;
    Some(value)
}

/// Answers a GET, a screen or an island: a handler resolved in it never writes.
pub enum Reading {}

/// Answers every other method: the unit of work commits its handler's outcome.
pub enum Writing {}

pub type ReadContext<A> = RequestContext<A, Reading>;

pub struct RequestContext<A: Area, M = Writing> {
    db: Db,
    pub(crate) matched: Matched,
    kind: PhantomData<fn() -> (A, M)>,
}

impl<A: Area, M> RequestContext<A, M> {
    pub(crate) fn new(db: Db, matched: Matched) -> Self {
        Self {
            db,
            matched,
            kind: PhantomData,
        }
    }

    pub(crate) fn session(&self) -> &RequestSession {
        &self.matched.session
    }

    pub(crate) fn query(&self) -> Option<&str> {
        self.matched.query.as_deref()
    }

    /// A decoded path segment that is not a record's id, such as a page's slug.
    #[must_use]
    pub fn path_parameter(&self, name: &str) -> Option<&str> {
        parameter(&self.matched.parameters, name)
    }
}

impl<A: Area, M> Context for RequestContext<A, M> {
    fn db(&self) -> &Db {
        &self.db
    }

    fn reach(&self) -> Reach {
        self.matched.reach
    }

    fn bound(&self, name: &str) -> Result<Uuid> {
        self.matched.bound(name)
    }

    fn input(&mut self) -> Result<serde_json::Value> {
        self.matched.input()
    }
}

impl<A: Area> Writes for RequestContext<A> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_malformed_id_names_nothing_and_a_missing_parameter_is_a_bug() {
        let parameters = vec![("id".to_owned(), "not-a-uuid".to_owned())];

        assert!(matches!(bound_id(&parameters, "id"), Err(Error::NotFound)));
        assert!(matches!(
            bound_id(&parameters, "order"),
            Err(Error::Internal { .. })
        ));
    }
}
