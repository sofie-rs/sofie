//! One WebSocket per tab carrying calls to the app's own routes: a GET is watched, answered
//! again whenever a record its loader read changes, and any other method runs once. Each
//! message passes the whole pipeline with the socket's opening headers, so every guard,
//! throttle and session check applies to it as to a request.

mod connection;
mod frames;
mod protocol;

use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use http::header::{
    CONNECTION, ORIGIN, RETRY_AFTER, SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_VERSION, UPGRADE,
};
use http::request::Parts;
use http::{HeaderValue, Method, StatusCode};
use hyper::body::Incoming;
use hyper::ext::Protocol;
use hyper::upgrade::OnUpgrade;
use hyper_util::rt::TokioIo;
use tokio::sync::watch;

use crate::answer::{self, Response, TEXT};
use crate::cookie::CookieNames;
use crate::csrf;
use crate::holders::Holders;
use crate::pipeline::Pipeline;
use crate::throttle::Requester;

use connection::Link;
use frames::{Handshake, HandshakeError};

pub(crate) const PATH: &str = "/_sofie/socket";
const RETRY: &str = "5";

pub(crate) struct Sockets {
    holders: Holders,
    closing: watch::Sender<bool>,
}

impl Default for Sockets {
    fn default() -> Self {
        Self {
            holders: Holders::default(),
            closing: watch::channel(false).0,
        }
    }
}

impl Sockets {
    pub(crate) fn close(&self) {
        self.closing.send_replace(true);
    }
}

pub(crate) fn is_opening(request: &http::Request<Incoming>) -> bool {
    let is_extended_connect = request.method() == Method::CONNECT
        && request
            .extensions()
            .get::<Protocol>()
            .is_some_and(|protocol| protocol.as_str().eq_ignore_ascii_case("websocket"));
    request.uri().path() == PATH && (request.method() == Method::GET || is_extended_connect)
}

/// The connection's place under the server's caps, kept by its sockets: an HTTP/1.1
/// connection is done serving once upgraded, but the socket still holds it.
#[derive(Clone)]
pub(crate) struct Hold {
    _held: Arc<dyn Send + Sync>,
}

impl Hold {
    pub(crate) fn new(held: impl Send + Sync + 'static) -> Self {
        Self {
            _held: Arc::new(held),
        }
    }
}

pub(crate) async fn open(
    pipeline: &Arc<Pipeline>,
    request: http::Request<Incoming>,
    peer: SocketAddr,
) -> Response {
    let (mut request, _) = request.into_parts();
    #[cfg(feature = "shield")]
    if let Some(refusal) = crate::pipeline::shielded(pipeline, &request, peer) {
        return answer::secure(refusal);
    }
    let answer = opened(pipeline, &mut request, peer).unwrap_or_else(|refusal| *refusal);
    answer::secure(answer)
}

fn opened(
    pipeline: &Arc<Pipeline>,
    request: &mut Parts,
    peer: SocketAddr,
) -> Result<Response, Box<Response>> {
    let refuse = |status, text| Box::new(answer::response(status, Some(TEXT), text));
    let handshake =
        frames::handshake(&request.method, &request.headers).map_err(refused_handshake)?;
    if is_cross_site(request, &pipeline.sessions.cookies) {
        return Err(refuse(StatusCode::FORBIDDEN, "Refused."));
    }
    let upgrading = request
        .extensions
        .remove::<OnUpgrade>()
        .ok_or_else(|| refuse(StatusCode::BAD_REQUEST, "Not a WebSocket handshake."))?;
    let now = pipeline.db.clock().now().timestamp();
    let session = pipeline.sessions.open(&request.headers, now);
    let holder = Requester::of(session.principal(), pipeline.client(request, peer).0);
    let held = pipeline.sockets.holders.admit(holder).ok_or_else(|| {
        let mut busy = refuse(StatusCode::TOO_MANY_REQUESTS, "Too many open sockets.");
        busy.headers_mut()
            .insert(RETRY_AFTER, HeaderValue::from_static(RETRY));
        busy
    })?;
    let link = Link {
        pipeline: Arc::clone(pipeline),
        peer,
        headers: protocol::forwarded(&request.headers),
        session,
        closing: pipeline.sockets.closing.subscribe(),
        _held: held,
        _connection: request.extensions.remove::<Hold>(),
    };
    let switching = accepted(handshake)?;
    tokio::spawn(async move {
        match upgrading.await {
            Ok(upgraded) => connection::run(TokioIo::new(upgraded), link).await,
            Err(error) => tracing::debug!(error = %error, "a socket's upgrade did not complete"),
        }
    });
    Ok(switching)
}

fn refused_handshake(error: HandshakeError) -> Box<Response> {
    let refused = match error {
        HandshakeError::NotAnUpgrade => answer::response(
            StatusCode::BAD_REQUEST,
            Some(TEXT),
            "Not a WebSocket handshake.",
        ),
        HandshakeError::UnsupportedVersion => {
            let mut refused = answer::response(
                StatusCode::UPGRADE_REQUIRED,
                Some(TEXT),
                "Speak WebSocket version 13.",
            );
            refused
                .headers_mut()
                .insert(SEC_WEBSOCKET_VERSION, HeaderValue::from_static("13"));
            refused
        }
    };
    Box::new(refused)
}

fn accepted(handshake: Handshake) -> Result<Response, Box<Response>> {
    let Handshake::Upgrade(accept) = handshake else {
        return Ok(answer::response(StatusCode::OK, None, Bytes::new()));
    };
    let accept = HeaderValue::try_from(accept).map_err(|_| {
        let text = "The handshake could not be answered.";
        Box::new(answer::response(
            StatusCode::INTERNAL_SERVER_ERROR,
            Some(TEXT),
            text,
        ))
    })?;
    let mut switching = answer::response(StatusCode::SWITCHING_PROTOCOLS, None, Bytes::new());
    let headers = switching.headers_mut();
    headers.insert(UPGRADE, HeaderValue::from_static("websocket"));
    headers.insert(CONNECTION, HeaderValue::from_static("upgrade"));
    headers.insert(SEC_WEBSOCKET_ACCEPT, accept);
    Ok(switching)
}

/// Browsers send cookies on a handshake from any site and no CORS check guards it, so a page
/// elsewhere could act as the person (cross-site WebSocket hijacking); every browser names
/// the page's origin. A client sending none acts only as itself, without the session cookie.
fn is_cross_site(request: &Parts, cookies: &CookieNames) -> bool {
    match request.headers.get(ORIGIN) {
        Some(origin) => {
            !csrf::host_of(request).is_some_and(|host| csrf::is_origin_of(origin, host))
        }
        None => cookies.session_value(&request.headers).is_some(),
    }
}
