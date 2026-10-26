use std::sync::Arc;
use std::time::Duration;

use sofie_core::{Error, Timestamp, Uuid};

use crate::answer::{self, Response};
use crate::pipeline::Asked;
use crate::routes::RouteRules;
use crate::throttle::{Limiter, Requester};

#[derive(Default)]
pub(crate) struct AreaPolicy {
    pub(crate) throttle: Option<Limiter>,
    #[cfg(feature = "steps")]
    pub(crate) steps: Vec<Box<dyn crate::PipelineStep>>,
    #[cfg(feature = "cors")]
    pub(crate) cors: Option<crate::cors::AreaCors>,
    #[cfg(feature = "engine")]
    pub(crate) content_security: crate::content_security::AreaContentSecurity,
}

pub(crate) struct Admission {
    pub(crate) area: Arc<AreaPolicy>,
    throttle: Option<Limiter>,
    pub(crate) rules: RouteRules,
}

#[derive(Clone, Copy)]
pub(crate) struct Visit<'r> {
    pub(crate) asked: Asked<'r>,
    pub(crate) principal: Option<Uuid>,
    pub(crate) now: Timestamp,
    #[cfg(feature = "signed-links")]
    pub(crate) keys: &'r sofie_auth::SigningKeys,
}

impl Admission {
    pub(crate) fn new(rules: &RouteRules, area: Arc<AreaPolicy>) -> Self {
        Self {
            area,
            throttle: rules.throttle.map(Limiter::new),
            rules: rules.clone(),
        }
    }

    /// Before the body is read; the area's steps run after. The area's shared budget is spent
    /// last, so a request this route refuses costs the caller's other routes nothing.
    pub(crate) fn refusal(&self, visit: Visit<'_>) -> Option<Response> {
        let requester = Requester::of(visit.principal, visit.asked.client);
        let throttled = |limiter: Option<&Limiter>| {
            let wait = limiter?.admit(requester, visit.now).err()?;
            Some(too_many_requests(wait, visit.asked))
        };
        if let Some(refusal) = throttled(self.throttle.as_ref()) {
            return Some(refusal);
        }
        #[cfg(feature = "signed-links")]
        if self.rules.signed && !crate::signed::is_signed_request(visit) {
            return Some(crate::signed::unsigned(visit.asked.caller));
        }
        throttled(self.area.throttle.as_ref())
    }
}

/// The caller is not logged, as an address is personal data.
pub(crate) fn too_many_requests(wait: Duration, asked: Asked<'_>) -> Response {
    tracing::warn!(
        http.request.method = asked.request.method.as_str(),
        retry_after = answer::whole_seconds(wait),
        "refused a request over its rate limit"
    );
    answer::failure(Error::TooManyRequests(wait), asked.caller)
}
