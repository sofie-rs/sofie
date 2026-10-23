use std::net::IpAddr;
use std::time::Duration;

use http::header::{
    CONTENT_SECURITY_POLICY, REFERRER_POLICY, SET_COOKIE, STRICT_TRANSPORT_SECURITY,
    X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS,
};
use http::request::Parts;
use http::{HeaderMap, HeaderName, HeaderValue};
use sofie_core::{Error, Result, Uuid};
use sofie_db::BoxFuture;

use crate::admission::Visit;
use crate::answer::{self, Response};
use crate::pipeline::Asked;
use crate::target::Target;

/// `check` can only refuse and runs after CSRF, the guard and rate limits, so a
/// step cannot let through what they refused.
pub trait PipelineStep: Send + Sync + 'static {
    /// Before the body is read.
    fn check<'a>(&'a self, _request: &'a RequestHead<'a>) -> BoxFuture<'a, Result<()>> {
        Box::pin(std::future::ready(Ok(())))
    }

    /// Before compression and the framework's security and CORS headers.
    fn answered(&self, _request: &RequestHead<'_>, _answer: &mut ResponseHead<'_>) {}
}

pub struct RequestHead<'r> {
    parts: &'r Parts,
    client: IpAddr,
    principal: Option<Uuid>,
}

impl RequestHead<'_> {
    #[must_use]
    pub fn method(&self) -> &str {
        self.parts.method.as_str()
    }

    #[must_use]
    pub fn path(&self) -> &str {
        self.parts.uri.path()
    }

    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.parts.headers.get(name)?.to_str().ok()
    }

    /// Read through trusted proxies only.
    #[must_use]
    pub fn client(&self) -> IpAddr {
        self.client
    }

    #[must_use]
    pub fn principal(&self) -> Option<Uuid> {
        self.principal
    }
}

/// The status and body are the handler's and the framework's; a step changes headers only.
pub struct ResponseHead<'r> {
    status: u16,
    headers: &'r mut HeaderMap,
    elapsed: Duration,
}

const FRAMEWORK_HEADERS: [HeaderName; 6] = [
    SET_COOKIE,
    CONTENT_SECURITY_POLICY,
    X_CONTENT_TYPE_OPTIONS,
    REFERRER_POLICY,
    X_FRAME_OPTIONS,
    STRICT_TRANSPORT_SECURITY,
];

impl ResponseHead<'_> {
    #[must_use]
    pub fn status(&self) -> u16 {
        self.status
    }

    /// Since the request was routed, until its answer was built.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        self.elapsed
    }

    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name)?.to_str().ok()
    }

    /// The session cookie, security and CORS headers are the framework's and refused.
    pub fn set_header(&mut self, name: &str, value: &str) -> Result<()> {
        let header = HeaderName::try_from(name)
            .map_err(|error| Error::internal(format!("header name `{name}`"), error))?;
        if is_framework_header(&header) {
            return Err(Error::internal_message(format!(
                "a pipeline step may not set `{header}`, which the framework sets"
            )));
        }
        let value = HeaderValue::try_from(value)
            .map_err(|error| Error::internal(format!("the value of header `{header}`"), error))?;
        self.headers.insert(header, value);
        Ok(())
    }
}

fn is_framework_header(name: &HeaderName) -> bool {
    FRAMEWORK_HEADERS.contains(name)
        || name.as_str() == answer::PERMISSIONS_POLICY_HEADER
        || name.as_str().starts_with("access-control-")
}

fn request_head(asked: Asked<'_>, principal: Option<Uuid>) -> RequestHead<'_> {
    RequestHead {
        parts: asked.request,
        client: asked.client,
        principal,
    }
}

pub(crate) async fn refusal(
    everywhere: &[Box<dyn PipelineStep>],
    area: &[Box<dyn PipelineStep>],
    visit: Visit<'_>,
) -> Option<Response> {
    let head = request_head(visit.asked, visit.principal);
    for step in everywhere.iter().chain(area) {
        if let Err(error) = step.check(&head).await {
            return Some(answer::failure(error, visit.asked.caller));
        }
    }
    None
}

/// In reverse of `check`'s order, so the outermost step sees the answer last.
pub(crate) fn answered(
    everywhere: &[Box<dyn PipelineStep>],
    target: Option<&Target>,
    asked: Asked<'_>,
    principal: Option<Uuid>,
    mut response: Response,
    elapsed: Duration,
) -> Response {
    let area = target.map_or(&[][..], |target| &target.admission.area.steps[..]);
    if everywhere.is_empty() && area.is_empty() {
        return response;
    }
    let request = request_head(asked, principal);
    let status = response.status().as_u16();
    let mut head = ResponseHead {
        status,
        headers: response.headers_mut(),
        elapsed,
    };
    for step in area.iter().rev().chain(everywhere.iter().rev()) {
        step.answered(&request, &mut head);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_may_not_replace_the_content_security_policy() {
        let mut headers = HeaderMap::new();
        let mut head = ResponseHead {
            status: 200,
            headers: &mut headers,
            elapsed: Duration::ZERO,
        };

        assert!(
            head.set_header("Content-Security-Policy", "script-src *")
                .is_err()
        );
        assert!(head.set_header("x-answered-by", "step").is_ok());
    }
}
