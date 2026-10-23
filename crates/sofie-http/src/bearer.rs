use http::header::{AUTHORIZATION, WWW_AUTHENTICATE};
use http::{HeaderMap, HeaderValue, StatusCode};
use sofie_auth::BearerToken;
use sofie_core::{Timestamp, Uuid};

use crate::admission::too_many_requests;
use crate::answer::{self, Response};
use crate::pipeline::{Asked, Pipeline};
use crate::session::RequestSession;
use crate::target::Target;
use crate::throttle::{Limiter, Rate, Requester};

const REFUSED: &str = "The bearer token is unknown, expired or revoked.";
const FAILURES: Rate = Rate::per_minute(10);

pub struct Bearer {
    pub token: Uuid,
    pub principal: Uuid,
    /// Permission patterns narrowing what the principal holds in a `Permitted` area.
    pub scopes: Vec<String>,
}

/// Failed lookups per caller; past them the caller is throttled, which the shield counts.
pub(crate) struct Tokens {
    failures: Limiter,
}

impl Default for Tokens {
    fn default() -> Self {
        Self {
            failures: Limiter::new(FAILURES),
        }
    }
}

pub(crate) async fn open(
    pipeline: &Pipeline,
    asked: Asked<'_>,
    target: &Target,
    now: Timestamp,
) -> Result<RequestSession, Box<Response>> {
    let headers = &asked.request.headers;
    let Some(text) = presented(target, headers) else {
        return Ok(pipeline.sessions.open(headers, now));
    };
    #[cfg(feature = "shield")]
    if let Some(refusal) = pipeline.shield.refusal(asked, None) {
        return Err(Box::new(refusal));
    }
    let caller = Requester::of(None, asked.client);
    if let Err(wait) = pipeline.tokens.failures.check(caller, now) {
        return Err(Box::new(too_many_requests(wait, asked)));
    }
    let area = target.audience.name;
    let members = pipeline.sessions.members.as_deref();
    let found =
        match (BearerToken::presented(text), members) {
            (Some(token), Some(members)) => members
                .bearer(&pipeline.db, area, &token.digest())
                .await
                .map_err(|error| Box::new(answer::failure(error, asked.caller)))?,
            _ => None,
        };
    let Some(bearer) = found else {
        return Err(Box::new(refused(&pipeline.tokens, asked, area, now)));
    };
    tracing::debug!(token.id = %bearer.token, area, "a bearer token opened the request");
    Ok(RequestSession::bearer(area, bearer, now))
}

fn presented<'h>(target: &Target, headers: &'h HeaderMap) -> Option<&'h str> {
    if !target.audience.tokens {
        return None;
    }
    crate::caller::bearer_credentials(headers.get(AUTHORIZATION)?)
}

fn refused(tokens: &Tokens, asked: Asked<'_>, area: &str, now: Timestamp) -> Response {
    if let Err(wait) = tokens
        .failures
        .admit(Requester::of(None, asked.client), now)
    {
        return too_many_requests(wait, asked);
    }
    tracing::warn!(area, "refused an unknown, expired or revoked bearer token");
    let mut response = answer::refusal(StatusCode::UNAUTHORIZED, REFUSED, asked.caller);
    response.headers_mut().insert(
        WWW_AUTHENTICATE,
        HeaderValue::from_static("Bearer error=\"invalid_token\""),
    );
    response
}
