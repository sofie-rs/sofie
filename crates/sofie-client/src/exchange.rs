use std::time::{Duration, Instant};

use bytes::Bytes;
use http::header::{HOST, USER_AGENT};
use http::{HeaderMap, HeaderValue, Method, StatusCode, Uri};
use http_body_util::{BodyExt, Full, LengthLimitError, Limited};
use hyper::body::Incoming;
use sofie_core::{Error, Result};
use tracing::{Instrument, Span};

use crate::Shared;
use crate::connect::{PoolKey, Sender};
use crate::guard::{Failure, Reach};
use crate::request::Response;
use crate::target::Target;
use crate::telemetry;

const RETRIED_STATUSES: [StatusCode; 3] = [
    StatusCode::BAD_GATEWAY,
    StatusCode::SERVICE_UNAVAILABLE,
    StatusCode::GATEWAY_TIMEOUT,
];
const FIRST_BACKOFF: Duration = Duration::from_millis(100);
const MAX_BACKOFF: Duration = Duration::from_secs(2);

pub(crate) struct Exchange<'a> {
    pub(crate) shared: &'a Shared,
    pub(crate) method: Method,
    pub(crate) reach: Reach,
    pub(crate) headers: HeaderMap,
    pub(crate) body: Bytes,
    pub(crate) keyed: bool,
}

impl Exchange<'_> {
    pub(crate) async fn send(mut self, target: Target) -> Result<Response> {
        let span = telemetry::http_span(self.shared.vendor, &self.method, &target);
        let started = Instant::now();
        let deadline = tokio::time::Instant::from_std(started + self.shared.limits.total);
        let result = self.follow(target, deadline).instrument(span.clone()).await;
        telemetry::finish_http(&span, self.shared.vendor, &self.method, &result, started);
        result
    }

    /// The caller's headers, its credentials among them, stay with the origin they were meant for.
    async fn follow(
        &mut self,
        mut target: Target,
        deadline: tokio::time::Instant,
    ) -> Result<Response> {
        let mut followed = 0;
        loop {
            let response = self.with_retries(&target, deadline).await?;
            let Some(location) = self.redirect(&response, followed) else {
                return Ok(response);
            };
            let next = target.redirected(location).map_err(|problem| {
                Error::internal_message(format!(
                    "{} redirected to a location refused: {problem}",
                    self.shared.vendor
                ))
            })?;
            if !next.same_origin(&target) {
                self.headers.clear();
                self.keyed = false;
            }
            target = next;
            followed += 1;
        }
    }

    fn redirect<'r>(&self, response: &'r Response, followed: u32) -> Option<&'r str> {
        let allowed = match self.reach {
            Reach::Vendor => self.shared.limits.redirects,
            Reach::Untrusted => 0,
        };
        let follows = response.status.is_redirection()
            && response.status != StatusCode::NOT_MODIFIED
            && matches!(self.method, Method::GET | Method::HEAD)
            && followed < allowed;
        response.header("location").filter(|_| follows)
    }

    /// One request is one success or failure of its host and port, however often it was tried;
    /// running out of time is a failure, a refusal is neither.
    async fn with_retries(
        &self,
        target: &Target,
        deadline: tokio::time::Instant,
    ) -> Result<Response> {
        let breakers = match self.reach {
            Reach::Vendor => &self.shared.vendor_hosts,
            Reach::Untrusted => &self.shared.untrusted_hosts,
        };
        let circuit = format!("{}:{}", target.host, target.port);
        breakers.admit(&circuit)?;
        let result = tokio::time::timeout_at(deadline, self.retried(target))
            .await
            .unwrap_or_else(|_| {
                Err(Failure::Failed(Error::Unavailable(format!(
                    "{} did not answer within {} ms",
                    self.shared.vendor,
                    self.shared.limits.total.as_millis()
                ))))
            });
        let succeeded = match &result {
            Ok(response) => Some(!response.status.is_server_error()),
            Err(Failure::Failed(_)) => Some(false),
            Err(Failure::Refused(_)) => None,
        };
        if let Some(succeeded) = succeeded {
            breakers.record(&circuit, succeeded, Instant::now());
        }
        result.map_err(Failure::into_error)
    }

    /// Retried only when the vendor applies it once however often it arrives: idempotent or keyed.
    async fn retried(&self, target: &Target) -> Result<Response, Failure> {
        let tries = if self.keyed || self.method.is_idempotent() {
            1 + self.shared.limits.retries
        } else {
            1
        };
        let mut attempt = 0;
        loop {
            let result = self.attempt(target).await;
            attempt += 1;
            let again = match &result {
                Ok(response) => RETRIED_STATUSES.contains(&response.status),
                Err(Failure::Failed(_)) => true,
                Err(Failure::Refused(_)) => false,
            };
            if !again || attempt >= tries {
                return result;
            }
            tokio::time::sleep(backoff(attempt)).await;
            Span::current().record("http.request.resend_count", attempt);
        }
    }

    async fn attempt(&self, target: &Target) -> Result<Response, Failure> {
        self.reach.admit(target, &self.shared.allowed)?;
        let key = PoolKey::of(target, self.reach);
        if let Some(mut sender) = self.shared.connector.reuse(&key).await {
            match sender.try_send_request(self.http_request(target)).await {
                Ok(response) => return self.read(response, key, sender).await,
                Err(error) if error.message().is_none() => {
                    return Err(failed(target, &error.into_error()));
                }
                Err(_) => {}
            }
        }
        let mut sender = self.connect(target).await?;
        let response = sender
            .send_request(self.http_request(target))
            .await
            .map_err(|error| failed(target, &error))?;
        self.read(response, key, sender).await
    }

    async fn connect(&self, target: &Target) -> Result<Sender, Failure> {
        let limit = self.shared.limits.connect;
        let deadline = Instant::now() + limit;
        let connecting = async {
            let addresses = self.reach.resolve(target, &self.shared.allowed).await?;
            self.shared
                .connector
                .open(target, &addresses, deadline)
                .await
        };
        tokio::time::timeout(limit, connecting)
            .await
            .unwrap_or_else(|_| {
                Err(Failure::Failed(Error::Unavailable(format!(
                    "connecting to `{}` took longer than {} ms",
                    target.host,
                    limit.as_millis()
                ))))
            })
    }

    fn http_request(&self, target: &Target) -> http::Request<Full<Bytes>> {
        let mut request = http::Request::new(Full::new(self.body.clone()));
        *request.method_mut() = self.method.clone();
        *request.uri_mut() = Uri::from(target.path.clone());
        let headers = request.headers_mut();
        headers.clone_from(&self.headers);
        let host = HeaderValue::from_str(&target.authority())
            .expect("a host and port parsed from a URI are a valid header value");
        headers.insert(HOST, host);
        headers
            .entry(USER_AGENT)
            .or_insert(HeaderValue::from_static("sofie-client"));
        if self.reach == Reach::Vendor
            && let Some(parent) = sofie_telemetry::current_traceparent()
            && let Ok(value) = HeaderValue::from_str(&parent.to_string())
        {
            headers.insert("traceparent", value);
        }
        request
    }

    async fn read(
        &self,
        response: http::Response<Incoming>,
        key: PoolKey,
        sender: Sender,
    ) -> Result<Response, Failure> {
        let (parts, body) = response.into_parts();
        let limit = self.shared.limits.response_bytes;
        let vendor = self.shared.vendor;
        let body = match Limited::new(body, limit).collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(error) if error.is::<LengthLimitError>() => {
                return Err(Failure::Refused(Error::internal_message(format!(
                    "{vendor}'s answer is larger than {limit} bytes"
                ))));
            }
            Err(error) => {
                return Err(Failure::Failed(Error::Unavailable(format!(
                    "reading {vendor}'s answer failed: {error}"
                ))));
            }
        };
        self.shared.connector.release(key, sender);
        Ok(Response {
            vendor,
            status: parts.status,
            headers: parts.headers,
            body,
        })
    }
}

fn backoff(attempt: u32) -> Duration {
    FIRST_BACKOFF
        .saturating_mul(2_u32.saturating_pow(attempt.saturating_sub(1)))
        .min(MAX_BACKOFF)
}

fn failed(target: &Target, error: &hyper::Error) -> Failure {
    Failure::Failed(Error::Unavailable(format!(
        "the request to `{}` failed: {error}",
        target.host
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connect::Connector;
    use crate::{Outgoing, Vendor};

    fn sends_traceparent(reach: Reach) -> bool {
        let outgoing = Outgoing::build(
            "vendor",
            Vendor::default(),
            Connector::trusting_public_roots(),
        )
        .unwrap();
        let exchange = Exchange {
            shared: &outgoing.shared,
            method: Method::GET,
            reach,
            headers: HeaderMap::new(),
            body: Bytes::new(),
            keyed: false,
        };
        let target = Target::parse("https://vendor.test/").unwrap();
        let _telemetry = sofie_telemetry::capture();
        let request = tracing::info_span!("work").in_scope(|| exchange.http_request(&target));
        request.headers().contains_key("traceparent")
    }

    #[test]
    fn only_a_vendor_is_sent_the_trace_id() {
        assert!(sends_traceparent(Reach::Vendor));
        assert!(!sends_traceparent(Reach::Untrusted));
    }
}
