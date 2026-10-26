use std::any::Any;
use std::future::poll_fn;
use std::net::{IpAddr, SocketAddr};
use std::ops::ControlFlow;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::task::Poll;
use std::time::Duration;

use bytes::Bytes;
use http::request::Parts;
use http::{Method, StatusCode};
use http_body_util::{BodyExt, Full, LengthLimitError, Limited};
use hyper::body::Incoming;
use sofie_core::{Error, Timestamp, Uuid};
use sofie_db::{Db, Reach};
use tracing::Instrument;

use crate::admission::Visit;
use crate::answer::{self, Response};
#[cfg(feature = "engine")]
use crate::assets::Assets;
use crate::caller::Caller;
use crate::context::Matched;
use crate::deadline::Deadline;
use crate::health::{Endpoints, Probe};
use crate::router::{Lookup, Router};
use crate::routes::BODY_LIMIT;
#[cfg(feature = "engine")]
use crate::screens::Screens;
use crate::session::{RequestSession, Sessions};
use crate::target::Target;
use crate::trace::{RequestTrace, RouteLabel};
use crate::urlencoded::Encoding;
use crate::view::RouteAnswer;
use crate::{csrf, guard};

const DISCARD_LIMIT: usize = 4 * 1024 * 1024;
const DISCARD_TIME: Duration = Duration::from_secs(2);
const CROSS_SITE: &str = "This request came from another site and was refused.";

pub(crate) struct Pipeline {
    pub(crate) db: Db,
    pub(crate) sessions: Sessions,
    pub(crate) router: Router,
    #[cfg(feature = "trusted-proxies")]
    pub(crate) proxies: crate::TrustedProxies,
    #[cfg(feature = "engine")]
    pub(crate) screens: Screens,
    #[cfg(feature = "engine")]
    pub(crate) assets: Assets,
    #[cfg(feature = "gates")]
    pub(crate) gates: crate::Gates,
    pub(crate) endpoints: Endpoints,
    #[cfg(feature = "live")]
    pub(crate) live: crate::live::Live,
    #[cfg(feature = "screen-cache")]
    pub(crate) screen_cache: crate::screen_cache::ScreenCache,
    #[cfg(feature = "shield")]
    pub(crate) shield: crate::shield::Guard,
    #[cfg(feature = "tokens")]
    pub(crate) tokens: crate::bearer::Tokens,
    #[cfg(feature = "steps")]
    pub(crate) steps: Vec<Box<dyn crate::PipelineStep>>,
}

#[derive(Clone, Copy)]
pub(crate) struct Asked<'r> {
    pub(crate) request: &'r Parts,
    pub(crate) caller: Caller<'r>,
    pub(crate) client: IpAddr,
    pub(crate) deadline: &'r Deadline,
}

impl Pipeline {
    pub(crate) async fn handle(
        &self,
        request: http::Request<Incoming>,
        peer: SocketAddr,
    ) -> Response {
        let (request, body) = request.into_parts();
        #[cfg(all(feature = "development", debug_assertions))]
        if let Some(response) = crate::development::answer(&request).await {
            return response;
        }
        let (client, edge_id_header) = self.client(&request, peer);
        let deadline = Deadline::default();
        let asked = Asked {
            request: &request,
            caller: Caller::of(&request.headers),
            client,
            deadline: &deadline,
        };
        let lookup = self.router.find(&request.method, request.uri.path());
        let probe = Probe::of(&request);
        let route = label(probe, &lookup);
        let started = match probe {
            Some(_) => RequestTrace::probe(&request, route),
            None => RequestTrace::start(&request, route, edge_id_header),
        };
        let trace = match started {
            Ok(trace) => trace,
            Err(error) => return answer::secure(answer::failure(error, asked.caller)),
        };
        let response = caught(self.respond(asked, lookup, body), asked.caller)
            .instrument(trace.span())
            .await;
        #[cfg(feature = "shield")]
        self.shield.observe(asked, route, &response);
        #[cfg(any(
            feature = "browser-telemetry",
            all(feature = "toolbar", debug_assertions),
            all(feature = "development", debug_assertions)
        ))]
        let response = self
            .endpoints
            .with_extras(
                response,
                &trace,
                route.and_then(|route| route.name),
                &request.headers,
            )
            .await;
        #[cfg(feature = "compression")]
        let response = crate::compression::compress(&request, response).await;
        trace.finish(answer::secure(without_head_body(&request.method, response)))
    }

    async fn respond(&self, asked: Asked<'_>, lookup: Lookup<'_>, mut body: Incoming) -> Response {
        let framework = asked
            .deadline
            .within(self.framework_answer(asked, &mut body));
        let answered = match framework.await {
            None => Some(timed_out(asked.caller)),
            Some(ControlFlow::Break(response)) => Some(response),
            Some(ControlFlow::Continue(())) => None,
        };
        if let Some(response) = answered {
            discard_rest(&mut body).await;
            return response;
        }
        #[cfg(feature = "cors")]
        if let Some(preflight) = crate::cors::preflight(&self.router, asked) {
            return preflight;
        }
        #[cfg(feature = "engine")]
        if let Some(asset) = self.asset(asked.request) {
            discard_rest(&mut body).await;
            return asset;
        }
        #[cfg(any(feature = "steps", feature = "cors"))]
        let target = target_of(&lookup);
        #[cfg(feature = "steps")]
        let started_routing = std::time::Instant::now();
        let mut unread = Some(body);
        let routed = async {
            let routed = self.routed(asked, lookup, &mut unread).await;
            routed.unwrap_or_else(|refusal| (*refusal, None))
        };
        #[cfg_attr(not(feature = "steps"), expect(unused_variables))]
        let (response, principal) = asked
            .deadline
            .within(routed)
            .await
            .unwrap_or_else(|| (timed_out(asked.caller), None));
        if let Some(mut refused) = unread {
            discard_rest(&mut refused).await;
        }
        #[cfg(feature = "steps")]
        let response = crate::step::answered(
            &self.steps,
            target,
            asked,
            principal,
            response,
            started_routing.elapsed(),
        );
        #[cfg(feature = "cors")]
        let response = crate::cors::answered(target, asked.request, response);
        response
    }

    #[cfg_attr(
        not(feature = "trusted-proxies"),
        expect(clippy::unused_self, reason = "only a proxy forwards the client")
    )]
    pub(crate) fn client(
        &self,
        #[cfg_attr(not(feature = "trusted-proxies"), expect(unused_variables))] request: &Parts,
        peer: SocketAddr,
    ) -> (IpAddr, Option<&http::HeaderName>) {
        #[cfg(feature = "trusted-proxies")]
        return self.proxies.client(peer.ip(), &request.headers);
        #[cfg(not(feature = "trusted-proxies"))]
        (peer.ip().to_canonical(), None)
    }

    #[cfg(feature = "engine")]
    fn asset(&self, request: &Parts) -> Option<Response> {
        if request.method != Method::GET && request.method != Method::HEAD {
            return None;
        }
        self.assets.response(request.uri.path())
    }

    /// For what is answered outside the pipeline, by the caller's address alone.
    #[cfg(all(feature = "shield", any(feature = "storage", feature = "live")))]
    pub(crate) fn shielded(&self, request: &Parts, peer: SocketAddr) -> Option<Response> {
        let deadline = Deadline::default();
        let asked = Asked {
            request,
            caller: Caller::of(&request.headers),
            client: self.client(request, peer).0,
            deadline: &deadline,
        };
        self.shield.refusal(asked, None)
    }

    async fn framework_answer(
        &self,
        asked: Asked<'_>,
        body: &mut Incoming,
    ) -> ControlFlow<Response> {
        #[cfg(feature = "shield")]
        if crate::health::is_reserved(asked.request.uri.path())
            && let Some(refusal) = self.shield.refusal(asked, None)
        {
            return ControlFlow::Break(refusal);
        }
        #[cfg(feature = "remote-contracts")]
        crate::remote::answer(asked, &self.db, &self.sessions.keys, body).await?;
        self.endpoints.answer(asked, &self.db, body).await
    }

    /// A refusal before the guard leaves the session as it came; one after it is finished.
    async fn routed(
        &self,
        asked: Asked<'_>,
        lookup: Lookup<'_>,
        body: &mut Option<Incoming>,
    ) -> Result<(Response, Option<Uuid>), Box<Response>> {
        let (request, caller) = (asked.request, asked.caller);
        #[cfg(feature = "shield")]
        if !matches!(lookup, Lookup::Found(..)) {
            refuse(self.shield.refusal(asked, None))?;
        }
        let (target, parameters) = match lookup {
            Lookup::Found(target, parameters) => (target, parameters),
            Lookup::WrongMethod(allowed) => {
                return Ok((answer::method_not_allowed(&allowed, caller), None));
            }
            Lookup::Missing => return Ok((answer::failure(Error::NotFound, caller), None)),
        };
        #[cfg(feature = "webhooks")]
        if target.admission.rules.verify.is_some() {
            return crate::webhook::deliver(self, asked, target, parameters, body)
                .await
                .map(|response| (response, None));
        }
        let now = self.db.clock().now().timestamp();
        #[cfg(feature = "tokens")]
        let session = crate::bearer::open(self, asked, target, now).await?;
        #[cfg(not(feature = "tokens"))]
        let session = self.sessions.open(&request.headers, now);
        #[cfg(feature = "client-headers")]
        let session = session.with_client(request);
        #[cfg(feature = "session-list")]
        let session = session
            .confirmed(&self.sessions, &self.db, now)
            .await
            .map_err(|error| Box::new(answer::failure(error, caller)))?;
        if !session.is_bearer()
            && let Some(check) =
                csrf::refused_in(&target.admission.area, request, &self.sessions.cookies)
        {
            tracing::warn!(
                http.request.method = request.method.as_str(),
                http.route = target.pattern.as_str(),
                csrf.check = check.name(),
                "refused a cross-site request"
            );
            return Ok((
                answer::refusal(StatusCode::FORBIDDEN, CROSS_SITE, caller),
                None,
            ));
        }
        #[cfg(feature = "shield")]
        refuse(self.shield.refusal(asked, session.principal()))?;
        // Another area's cookie, which anyone can get by signing up there, keys no budget here.
        let budgeted = session.principal_in(target.audience.name);
        refuse(target.admission.refusal(self.visit(asked, budgeted, now)))?;
        #[cfg(feature = "gates")]
        let gates = self.gates.open(target, &session, &self.db, caller).await?;
        let response = match target.audience.reach(&session) {
            Some(reach) => {
                let found = Found {
                    target,
                    parameters,
                    session: session.clone(),
                    reach,
                    #[cfg(any(feature = "steps", feature = "idempotency", feature = "etag"))]
                    now,
                    #[cfg(feature = "gates")]
                    gates,
                };
                self.guarded(asked, found, body)
                    .await
                    .unwrap_or_else(|refusal| *refusal)
            }
            None => guard::unauthenticated(target.audience, request, caller),
        };
        let response = session.finish(response, &self.sessions, now, caller);
        #[cfg(feature = "shield")]
        let response = crate::shield::tag(response, session.principal());
        Ok((response, session.principal()))
    }

    #[cfg_attr(
        not(feature = "signed-links"),
        expect(clippy::unused_self, reason = "only a signed link needs the keys")
    )]
    pub(crate) fn visit<'r>(
        &'r self,
        asked: Asked<'r>,
        principal: Option<Uuid>,
        now: Timestamp,
    ) -> Visit<'r> {
        Visit {
            asked,
            principal,
            now,
            #[cfg(feature = "signed-links")]
            keys: &self.sessions.keys,
        }
    }

    /// Admitted already: its throttle and signature are checked before anything is looked up.
    pub(crate) async fn guarded(
        &self,
        asked: Asked<'_>,
        found: Found<'_>,
        body: &mut Option<Incoming>,
    ) -> Result<Response, Box<Response>> {
        let target = found.target;
        #[cfg(feature = "product-events")]
        if let Some(principal) = found.session.principal() {
            sofie_telemetry::attach_principal(&tracing::Span::current(), principal);
        }
        #[cfg(any(
            feature = "steps",
            feature = "idempotency",
            feature = "etag",
            feature = "live"
        ))]
        let principal = found.session.principal();
        #[cfg(any(feature = "steps", feature = "idempotency"))]
        let visit = self.visit(asked, principal, found.now);
        #[cfg(feature = "steps")]
        refuse(crate::step::refusal(&self.steps, &target.admission.area.steps, visit).await)?;
        let received = self.received(asked, target, body).await?;
        #[cfg(feature = "idempotency")]
        let work = target
            .admission
            .work(visit, &target.pattern, &received.fingerprint)?;
        let request = asked.request;
        #[cfg(feature = "remote-contracts")]
        let acting = found.session.acting();
        let matched = Matched {
            parameters: found.parameters,
            query: request.uri.query().map(str::to_owned),
            body: Some(received.body),
            encoding: received.encoding,
            session: found.session,
            reach: found.reach,
            deadline: asked.deadline.clone(),
            app: crate::AppName::of_serving(&self.endpoints.serving),
            route: target.name,
            #[cfg(feature = "idempotency")]
            work,
            #[cfg(feature = "signed-links")]
            keys: std::sync::Arc::clone(&self.sessions.keys),
            #[cfg(any(feature = "flags", feature = "permissions"))]
            gates: found.gates.clone(),
            #[cfg(feature = "shield")]
            shield: self.shield.for_request(asked, target.audience.name),
            #[cfg(feature = "storage")]
            storage: self.endpoints.serving.storage.clone(),
            #[cfg(feature = "listing")]
            prefers_json: asked.caller == Caller::Json
                || matches!(target.view, crate::routes::View::Json),
        };
        #[cfg(all(feature = "toolbar", debug_assertions))]
        let clocked = crate::toolbar::clocked(&self.db, &matched.session);
        #[cfg(all(feature = "toolbar", debug_assertions))]
        let db = &clocked;
        #[cfg(not(all(feature = "toolbar", debug_assertions)))]
        let db = &self.db;
        let route_answer = RouteAnswer {
            db,
            router: &self.router,
            #[cfg(feature = "engine")]
            screens: &self.screens,
            target,
            asked,
            #[cfg(all(feature = "gates", feature = "engine"))]
            gates: &found.gates,
            #[cfg(feature = "screen-cache")]
            screen_cache: &self.screen_cache,
            #[cfg(feature = "live")]
            keys: &self.sessions.keys,
            #[cfg(feature = "live")]
            principal,
        };
        let answer = sofie_core::moves::recorded(route_answer.answer(matched));
        #[cfg(feature = "remote-contracts")]
        let answer = sofie_db::remote::acting_as(acting, answer);
        #[cfg(feature = "etag")]
        let answer = crate::etag::Keyed::of(
            asked,
            principal,
            target,
            #[cfg(feature = "gates")]
            &found.gates,
            found.now,
        )
        .tagged(answer);
        #[cfg(feature = "uploads")]
        let answer = received.stored.taking(answer).await;
        #[cfg(not(feature = "uploads"))]
        let answer = answer.await;
        #[cfg(feature = "uploads")]
        received.stored.settle(answer.status()).await;
        #[cfg(feature = "gates")]
        found.gates.record();
        Ok(answer)
    }

    /// Takes the body, which is drained instead if the request is refused before this.
    async fn received(
        &self,
        asked: Asked<'_>,
        target: &Target,
        body: &mut Option<Incoming>,
    ) -> Result<RequestBody, Box<Response>> {
        let body = body.take().ok_or_else(|| {
            let error = Error::internal_message("the request body was taken twice");
            Box::new(answer::failure(error, asked.caller))
        })?;
        let limit = target.admission.rules.body_limit.unwrap_or(BODY_LIMIT);
        #[cfg(feature = "uploads")]
        return crate::uploads::read(asked, target, &self.endpoints.serving, body, limit).await;
        #[cfg(not(feature = "uploads"))]
        let mut body = body;
        #[cfg(not(feature = "uploads"))]
        Ok(RequestBody::plain(
            read_body(&mut body, limit, asked.caller).await?,
            &asked.request.headers,
        ))
    }
}

pub(crate) struct RequestBody {
    pub(crate) body: Bytes,
    pub(crate) encoding: Encoding,
    #[cfg(feature = "uploads")]
    pub(crate) stored: crate::uploads::Stored,
    /// What a retry sends again, which a stored file's key, expiry and seal are not.
    #[cfg(feature = "idempotency")]
    pub(crate) fingerprint: Bytes,
}

impl RequestBody {
    pub(crate) fn plain(body: Bytes, headers: &http::HeaderMap) -> Self {
        Self {
            #[cfg(feature = "idempotency")]
            fingerprint: body.clone(),
            body,
            encoding: Encoding::of(headers),
            #[cfg(feature = "uploads")]
            stored: crate::uploads::Stored::default(),
        }
    }
}

/// A panic would otherwise unwind through hyper's connection task: the client gets a reset
/// and no log line, and on HTTP/2 every other request on the connection dies with it.
fn caught(
    work: impl Future<Output = Response>,
    caller: Caller<'_>,
) -> impl Future<Output = Response> {
    let mut work = Box::pin(work);
    poll_fn(
        move |context| match catch_unwind(AssertUnwindSafe(|| work.as_mut().poll(context))) {
            Ok(polled) => polled,
            Err(panic) => {
                let message = format!("the request panicked: {}", panic_message(panic.as_ref()));
                Poll::Ready(answer::failure(Error::internal_message(message), caller))
            }
        },
    )
}

fn panic_message(panic: &(dyn Any + Send)) -> &str {
    if let Some(message) = panic.downcast_ref::<&str>() {
        return message;
    }
    panic
        .downcast_ref::<String>()
        .map_or("a value that is not text", String::as_str)
}

fn timed_out(caller: Caller<'_>) -> Response {
    tracing::warn!(error.kind = "timeout", "the request took too long");
    answer::refusal(
        StatusCode::SERVICE_UNAVAILABLE,
        "The request took too long.",
        caller,
    )
}

fn refuse(refusal: Option<Response>) -> Result<(), Box<Response>> {
    refusal.map_or(Ok(()), |refusal| Err(Box::new(refusal)))
}

pub(crate) struct Found<'r> {
    pub(crate) target: &'r Target,
    pub(crate) parameters: Vec<(String, String)>,
    pub(crate) session: RequestSession,
    pub(crate) reach: Reach,
    /// Read once, so every check on the request sees the same instant.
    #[cfg(any(feature = "steps", feature = "idempotency", feature = "etag"))]
    pub(crate) now: Timestamp,
    #[cfg(feature = "gates")]
    pub(crate) gates: crate::gate::RequestGates,
}

#[cfg(any(feature = "steps", feature = "cors"))]
fn target_of<'r>(lookup: &Lookup<'r>) -> Option<&'r Target> {
    match lookup {
        Lookup::Found(target, _) => Some(target),
        Lookup::WrongMethod(_) | Lookup::Missing => None,
    }
}

fn label<'r>(probe: Option<Probe>, lookup: &Lookup<'r>) -> Option<RouteLabel<'r>> {
    match (probe, lookup) {
        (Some(probe), _) => Some(RouteLabel {
            pattern: probe.path(),
            name: Some(probe.path()),
            area: None,
            module: None,
        }),
        (None, Lookup::Found(target, _)) => Some(RouteLabel {
            pattern: target.pattern.as_str(),
            name: target.name,
            area: Some(target.audience.name),
            module: Some(target.module),
        }),
        (None, Lookup::WrongMethod(_) | Lookup::Missing) => None,
    }
}

/// The body is dropped here rather than left to each protocol's implementation; its length
/// stays, as GET would send it in the encoding the headers name.
fn without_head_body(method: &Method, response: Response) -> Response {
    if method != Method::HEAD {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    if let Some(length) = hyper::body::Body::size_hint(&body).exact() {
        parts
            .headers
            .entry(http::header::CONTENT_LENGTH)
            .or_insert_with(|| http::HeaderValue::from(length));
    }
    Response::from_parts(parts, Full::new(Bytes::new()))
}

pub(crate) async fn read_body(
    body: &mut Incoming,
    limit: usize,
    caller: Caller<'_>,
) -> Result<Bytes, Box<Response>> {
    let error = match Limited::new(&mut *body, limit).collect().await {
        Ok(collected) => return Ok(collected.to_bytes()),
        Err(error) => error,
    };
    if error.is::<LengthLimitError>() {
        discard_rest(body).await;
        return Err(Box::new(too_large(caller)));
    }
    tracing::warn!(error.kind = "body", error = %error, "the request body could not be read");
    Err(Box::new(answer::refusal(
        StatusCode::BAD_REQUEST,
        "The request body could not be read.",
        caller,
    )))
}

pub(crate) fn too_large(caller: Caller<'_>) -> Response {
    answer::refusal(
        StatusCode::PAYLOAD_TOO_LARGE,
        "The request body is too large.",
        caller,
    )
}

/// Closing an HTTP/1.1 connection with unread bytes makes the kernel send a reset that can
/// destroy the answer, so the rest is read within bounds first (nginx's lingering close).
pub(crate) async fn discard_rest(body: &mut Incoming) {
    if hyper::body::Body::is_end_stream(body) {
        return;
    }
    let reading = async {
        let mut discarded = 0;
        while discarded < DISCARD_LIMIT {
            let Some(Ok(frame)) = body.frame().await else {
                return;
            };
            discarded += frame.data_ref().map_or(0, Bytes::len);
        }
    };
    if tokio::time::timeout(DISCARD_TIME, reading).await.is_err() {
        tracing::debug!(
            error.kind = "body_discard",
            "the rest of a refused body was not read in time"
        );
    }
}
