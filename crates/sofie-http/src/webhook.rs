use std::sync::Arc;

use bytes::Bytes;
use http::{HeaderMap, Method, StatusCode};
use sofie_auth::digest;
use sofie_core::{Result, Timestamp};
use sofie_db::{Call, Work};

use crate::admission::Visit;
use crate::answer::{self, Response};
use crate::handler::{Endpoint, endpoint};
use crate::routes::{RouteRules, View};
use crate::{RequestContext, Routes, Web};

const UNVERIFIED: &str = "The delivery could not be verified.";

#[must_use = "a webhook is served only once `.verify(…)` names how its deliveries are verified"]
pub struct WebhookRoute<'r> {
    routes: &'r mut Routes<Web>,
    path: String,
    endpoint: Endpoint,
    body_limit: Option<usize>,
}

impl Routes<Web> {
    /// Opens no session, needs no same-origin proof; each delivery id is answered once.
    pub fn webhook<Args: 'static, H>(&mut self, path: &str, handler: H) -> WebhookRoute<'_>
    where
        H: Call<RequestContext<Web>, Args, Outcome: Send>,
    {
        WebhookRoute {
            path: path.to_owned(),
            endpoint: endpoint(handler),
            body_limit: None,
            routes: self,
        }
    }
}

impl WebhookRoute<'_> {
    pub fn body_limit(self, bytes: usize) -> Self {
        Self {
            body_limit: Some(bytes),
            ..self
        }
    }

    pub fn verify(self, verifier: impl Verify) {
        let rules = RouteRules {
            body_limit: self.body_limit,
            verify: Some(Arc::new(verifier)),
            ..RouteRules::default()
        };
        self.routes
            .push(Method::POST, &self.path, self.endpoint, View::Json, rules);
    }
}

pub trait Verify: Send + Sync + 'static {
    /// The id must not change between attempts: a redelivery answers as the first did.
    fn verify(&self, delivery: &Delivery<'_>) -> Result<String>;
}

/// The body exactly as sent.
pub struct Delivery<'r> {
    headers: &'r HeaderMap,
    body: &'r [u8],
    now: Timestamp,
}

impl Delivery<'_> {
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name)?.to_str().ok()
    }

    #[must_use]
    pub fn body(&self) -> &[u8] {
        self.body
    }

    /// For checking a signed timestamp against replays.
    #[must_use]
    pub fn now(&self) -> Timestamp {
        self.now
    }
}

pub(crate) async fn deliver(
    pipeline: &crate::pipeline::Pipeline,
    asked: crate::pipeline::Asked<'_>,
    target: &crate::target::Target,
    parameters: Vec<(String, String)>,
    body: &mut Option<hyper::body::Incoming>,
) -> std::result::Result<Response, Box<Response>> {
    #[cfg(feature = "shield")]
    if let Some(refusal) = pipeline.shield.refusal(asked, None) {
        return Err(Box::new(refusal));
    }
    let now = pipeline.db.clock().now().timestamp();
    if let Some(refusal) = target.admission.refusal(pipeline.visit(asked, None, now)) {
        return Err(Box::new(refusal));
    }
    let found = crate::pipeline::Found {
        target,
        parameters,
        session: crate::session::RequestSession::default(),
        reach: sofie_db::Reach::All,
        now,
        #[cfg(feature = "gates")]
        gates: crate::gate::RequestGates::default(),
    };
    pipeline.guarded(asked, found, body).await
}

/// A resend may carry other headers or a fresh signature, so the verified id alone decides.
pub(crate) fn delivered_work(
    verify: &dyn Verify,
    visit: Visit<'_>,
    pattern: &str,
    body: &Bytes,
) -> Result<Work, Box<Response>> {
    let asked = visit.asked;
    let delivery = Delivery {
        headers: &asked.request.headers,
        body,
        now: visit.now,
    };
    match verify.verify(&delivery) {
        Ok(id) => Ok(Work::new(
            digest(&[b"delivery", pattern.as_bytes(), id.as_bytes()]),
            String::new(),
        )),
        Err(error) => {
            tracing::warn!(
                http.route = pattern,
                error.kind = "webhook",
                error = %error,
                "refused a webhook delivery"
            );
            Err(Box::new(answer::refusal(
                StatusCode::UNAUTHORIZED,
                UNVERIFIED,
                asked.caller,
            )))
        }
    }
}
