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
use http::{HeaderValue, Method, StatusCode, Version};
use hyper::body::Incoming;
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
use frames::HandshakeError;

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
    request.method() == Method::GET && request.uri().path() == PATH
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
    let accept = frames::accept(&request.headers).map_err(|error| match error {
        HandshakeError::NotAnUpgrade => {
            refuse(StatusCode::BAD_REQUEST, "Not a WebSocket handshake.")
        }
        HandshakeError::UnsupportedVersion => {
            let mut refused = refuse(StatusCode::UPGRADE_REQUIRED, "Speak WebSocket version 13.");
            refused
                .headers_mut()
                .insert(SEC_WEBSOCKET_VERSION, HeaderValue::from_static("13"));
            refused
        }
    })?;
    if is_cross_site(request, &pipeline.sessions.cookies) {
        return Err(refuse(StatusCode::FORBIDDEN, "Refused."));
    }
    let upgrading = request
        .extensions
        .remove::<OnUpgrade>()
        .filter(|_| request.version == Version::HTTP_11)
        .ok_or_else(|| refuse(StatusCode::BAD_REQUEST, "A socket opens over HTTP/1.1."))?;
    let now = pipeline.db.clock().now().timestamp();
    let session = pipeline.sessions.open(&request.headers, now);
    let holder = Requester::of(session.principal(), pipeline.client(request, peer).0);
    let held = pipeline.sockets.holders.admit(holder).ok_or_else(|| {
        let mut busy = refuse(StatusCode::TOO_MANY_REQUESTS, "Too many open sockets.");
        busy.headers_mut()
            .insert(RETRY_AFTER, HeaderValue::from_static(RETRY));
        busy
    })?;
    let accept = HeaderValue::from_str(&accept).map_err(|_| {
        refuse(
            StatusCode::INTERNAL_SERVER_ERROR,
            "The handshake could not be answered.",
        )
    })?;
    let link = Link {
        pipeline: Arc::clone(pipeline),
        peer,
        headers: protocol::forwarded(&request.headers),
        session,
        closing: pipeline.sockets.closing.subscribe(),
        _held: held,
    };
    tokio::spawn(async move {
        match upgrading.await {
            Ok(upgraded) => connection::run(TokioIo::new(upgraded), link).await,
            Err(error) => tracing::debug!(error = %error, "a socket's upgrade did not complete"),
        }
    });
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
