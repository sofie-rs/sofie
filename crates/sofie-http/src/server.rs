use std::convert::Infallible;
use std::future::poll_fn;
use std::io;
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::Poll;
use std::time::Duration;

use http::header::CONNECTION;
use http::{HeaderValue, Request, StatusCode, Version};
use hyper::body::Incoming;
use hyper::service::{Service, service_fn};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto::Builder;
use hyper_util::server::graceful::{GracefulShutdown, Watcher};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use sofie_auth::SigningKeys;
use sofie_core::Result;

use crate::App;
use crate::answer::{self, Response};
#[cfg(feature = "engine")]
use crate::assets::Assets;
use crate::cookie::CookieNames;
use crate::deadline::REQUEST_TIMEOUT;
use crate::health::Endpoints;
use crate::peers::{PeerSlot, Peers, Refusals};
use crate::pipeline::Pipeline;
use crate::session::Sessions;
use crate::silence::Silenced;
use crate::target::route_table;

const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);
/// hyper's ~408 KB default lets each connection hold that much; nginx allows 32 KB.
const MAX_HEADER_BUFFER: usize = 32 * 1024;
/// Each stream may buffer a body up to its route's limit, so a client's share stays small.
const MAX_CONCURRENT_STREAMS: u32 = 32;
/// Past it the connection is closed (Rapid Reset, CVE-2023-44487).
const MAX_PENDING_RESETS: usize = 20;
/// RUSTSEC-2024-0003.
const MAX_ERROR_RESETS: usize = 256;
/// Long enough for a request accepted just before shutdown to read its headers, run and commit.
const SHUTDOWN_GRACE: Duration = HEADER_READ_TIMEOUT
    .saturating_add(REQUEST_TIMEOUT)
    .saturating_add(Duration::from_secs(5));
/// Past the cap, new connections wait in the kernel's backlog instead of exhausting descriptors.
const DEFAULT_MAX_CONNECTIONS: NonZeroUsize = NonZeroUsize::new(4096).unwrap();
/// Accept fails mostly when file descriptors run out; retrying at once spins.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);
const REFUSED_LIFETIME: Duration = HEADER_READ_TIMEOUT;
#[cfg(feature = "tls")]
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) async fn serve(
    mut app: App,
    listener: TcpListener,
    shutdown: impl Future<Output = ()>,
) -> Result<()> {
    let cookies = CookieNames::of_serving(&app.serving)?;
    #[cfg(any(feature = "idempotency", feature = "shield"))]
    let mut background = crate::background::Background::default();
    #[cfg(feature = "gates")]
    crate::gate::prepare(&mut app)?;
    #[cfg(feature = "cors")]
    crate::cors::prepare(&mut app)?;
    #[cfg(feature = "engine")]
    crate::content_security::prepare(&mut app)?;
    #[cfg(feature = "etag")]
    crate::etag::identify_build()?;
    #[cfg(feature = "islands")]
    crate::islands::install(&mut app.screens, &app.islands)?;
    #[cfg(feature = "shield")]
    let shield = crate::shield::Guard::start(&mut app, &mut background)?;
    #[cfg(feature = "live")]
    let live = crate::live::Live::new(&app.routes)?;
    let router = route_table(
        &app.routes,
        app.areas,
        &app.names,
        #[cfg(feature = "engine")]
        &app.screens,
    )?;
    #[cfg(feature = "tls")]
    let tls = app.serving.tls.take();
    let max_connections = app
        .serving
        .max_connections
        .unwrap_or(DEFAULT_MAX_CONNECTIONS);
    let keys = match app.serving.keys.take() {
        Some(keys) => keys,
        None => SigningKeys::from_environment()?,
    };
    #[cfg(feature = "idempotency")]
    background.spawn(crate::keyed::sweep_expired_keys(app.db.clone()));
    #[cfg(all(feature = "toolbar", debug_assertions))]
    let toolbar = crate::toolbar::Toolbar::new(&app.routes, cookies.clone());
    let pipeline = Pipeline {
        router,
        db: app.db,
        sessions: Sessions {
            keys: Arc::new(keys),
            #[cfg(any(feature = "tokens", feature = "session-list"))]
            members: app.members,
            cookies: cookies.clone(),
            #[cfg(feature = "session-list")]
            records: crate::session_cache::SessionCache::default(),
        },
        #[cfg(feature = "trusted-proxies")]
        proxies: std::mem::take(&mut app.serving.proxies),
        #[cfg(feature = "engine")]
        assets: served_assets(&app.screens),
        #[cfg(feature = "engine")]
        screens: app.screens,
        #[cfg(feature = "gates")]
        gates: app.gates,
        endpoints: Endpoints {
            #[cfg(feature = "browser-telemetry")]
            browser: crate::browser::Browser::new(
                &app.names,
                &app.routes,
                app.serving.browser_sample,
                cookies,
            ),
            serving: app.serving,
            #[cfg(all(feature = "toolbar", debug_assertions))]
            toolbar,
        },
        #[cfg(feature = "shield")]
        shield,
        #[cfg(feature = "tokens")]
        tokens: crate::bearer::Tokens::default(),
        #[cfg(feature = "steps")]
        steps: app.steps,
        #[cfg(feature = "live")]
        live,
        #[cfg(feature = "screen-cache")]
        screen_cache: crate::screen_cache::ScreenCache::default(),
    };
    let connections = Connections {
        builder: connection_builder(),
        graceful: GracefulShutdown::new(),
        pipeline: Arc::new(pipeline),
        permits: Arc::new(Semaphore::new(max_connections.get())),
        peers: Peers::default(),
        refusals: Refusals::default(),
        proxies_hinted: AtomicBool::new(false),
        #[cfg(feature = "tls")]
        tls,
    };
    run(connections, listener, shutdown).await;
    Ok(())
}

struct Connections {
    builder: Builder<TokioExecutor>,
    graceful: GracefulShutdown,
    pipeline: Arc<Pipeline>,
    permits: Arc<Semaphore>,
    peers: Peers,
    refusals: Refusals,
    proxies_hinted: AtomicBool,
    #[cfg(feature = "tls")]
    tls: Option<crate::Tls>,
}

async fn run(connections: Connections, listener: TcpListener, shutdown: impl Future<Output = ()>) {
    let mut shutdown = pin!(shutdown);
    while let Some((permit, accepted)) =
        next_connection(&listener, &connections.permits, shutdown.as_mut()).await
    {
        match accepted {
            Ok((stream, peer)) => connections.serve(stream, peer, permit),
            Err(error) => {
                tracing::warn!(
                    error.kind = "accept",
                    error = %error,
                    "accepting a connection failed"
                );
                tokio::time::sleep(ACCEPT_BACKOFF).await;
            }
        }
    }
    drop(listener);
    #[cfg(feature = "live")]
    connections.pipeline.live.close();
    if tokio::time::timeout(SHUTDOWN_GRACE, connections.graceful.shutdown())
        .await
        .is_err()
    {
        tracing::warn!(
            error.kind = "shutdown",
            "open connections did not finish within the grace period"
        );
    }
}

/// `shutdown` is checked first, so a stopping server accepts nothing more.
async fn next_connection(
    listener: &TcpListener,
    permits: &Arc<Semaphore>,
    mut shutdown: Pin<&mut impl Future<Output = ()>>,
) -> Option<(OwnedSemaphorePermit, io::Result<(TcpStream, SocketAddr)>)> {
    let mut acquiring = pin!(Arc::clone(permits).acquire_owned());
    let mut permit = None;
    poll_fn(|context| {
        if shutdown.as_mut().poll(context).is_ready() {
            return Poll::Ready(None);
        }
        if permit.is_none() {
            let acquired = std::task::ready!(acquiring.as_mut().poll(context));
            permit = Some(acquired.expect("the connection semaphore is never closed"));
        }
        let accepted = std::task::ready!(listener.poll_accept(context));
        Poll::Ready(permit.take().map(|permit| (permit, accepted)))
    })
    .await
}

fn connection_builder() -> Builder<TokioExecutor> {
    let mut builder = Builder::new(TokioExecutor::new());
    builder
        .http1()
        .timer(TokioTimer::new())
        .header_read_timeout(HEADER_READ_TIMEOUT)
        .max_buf_size(MAX_HEADER_BUFFER);
    builder
        .http2()
        .timer(TokioTimer::new())
        .max_concurrent_streams(MAX_CONCURRENT_STREAMS)
        .max_pending_accept_reset_streams(MAX_PENDING_RESETS)
        .max_local_error_reset_streams(MAX_ERROR_RESETS);
    builder
}

impl Connections {
    /// A trusted proxy or loopback speaks for many clients, so it is held to no share.
    fn peer_slot(&self, peer: SocketAddr) -> Result<Option<PeerSlot>, ()> {
        let address = peer.ip().to_canonical();
        #[cfg(feature = "trusted-proxies")]
        if self.pipeline.proxies.is_trusted(address) {
            return Ok(None);
        }
        if address.is_loopback() {
            return Ok(None);
        }
        self.peers.admit(address).map(Some).ok_or(())
    }

    fn serve(&self, stream: TcpStream, peer: SocketAddr, permit: OwnedSemaphorePermit) {
        if let Err(error) = stream.set_nodelay(true) {
            tracing::warn!(
                error.kind = "nodelay",
                error = %error,
                "disabling Nagle's algorithm failed"
            );
        }
        let Ok(slot) = self.peer_slot(peer) else {
            self.refused();
            let Some(answering) = self.refusals.admit(permit) else {
                return;
            };
            return self.spawn(
                (stream, peer),
                service_fn(refuse),
                answering,
                Some(REFUSED_LIFETIME),
            );
        };
        let pipeline = Arc::clone(&self.pipeline);
        let service = service_fn(move |request| {
            let pipeline = Arc::clone(&pipeline);
            #[cfg(feature = "storage")]
            return async move {
                Ok::<_, Infallible>(crate::storage::answer(&pipeline, request, peer).await)
            };
            #[cfg(all(feature = "live", not(feature = "storage")))]
            return async move {
                Ok::<_, Infallible>(crate::live::answer(&pipeline, request, peer).await)
            };
            #[cfg(not(any(feature = "live", feature = "storage")))]
            async move {
                Ok::<_, Infallible>(pipeline.handle(request, peer).await)
            }
        });
        self.spawn((stream, peer), service, (permit, slot), None);
    }

    fn refused(&self) {
        if let Some(refused) = self.refusals.counted(std::time::Instant::now()) {
            tracing::warn!(
                error.kind = "connections",
                refused,
                "refused connections from addresses that hold their share already"
            );
        }
        #[cfg(feature = "trusted-proxies")]
        let proxies_named = !self.pipeline.proxies.is_empty();
        #[cfg(not(feature = "trusted-proxies"))]
        let proxies_named = false;
        if proxies_named || self.proxies_hinted.swap(true, Ordering::Relaxed) {
            return;
        }
        tracing::warn!(
            error.kind = "connections",
            "one address reached the connection cap and no proxy is trusted; behind a load \
             balancer, name it in SOFIE_TRUSTED_PROXIES (feature `trusted-proxies`)"
        );
    }

    fn spawn<S, B>(
        &self,
        (stream, peer): (TcpStream, SocketAddr),
        service: S,
        held: impl Send + 'static,
        lifetime: Option<Duration>,
    ) where
        S: Service<Request<Incoming>, Response = http::Response<B>, Error = Infallible>
            + Send
            + 'static,
        S::Future: Send + 'static,
        B: hyper::body::Body + Send + 'static,
        B::Data: Send,
        B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        #[cfg(feature = "shield")]
        let shield = self.pipeline.shield.clone();
        let failed = move |error: &(dyn std::error::Error + 'static)| {
            if ends_routinely(error) {
                tracing::debug!(error = %error, "a connection ended");
                return;
            }
            tracing::warn!(error.kind = "connection", error = %error, "a connection failed");
            #[cfg(feature = "shield")]
            shield.connection_failed(peer.ip());
            #[cfg(not(feature = "shield"))]
            let _ = peer;
        };
        let stream = Silenced::new(stream);
        let served = Served {
            builder: self.builder.clone(),
            watcher: self.graceful.watcher(),
            service,
            lifetime,
        };
        #[cfg(feature = "tls")]
        if let Some(tls) = &self.tls {
            let accepting = tls.acceptor.accept(stream);
            tokio::spawn(async move {
                match tokio::time::timeout(HANDSHAKE_TIMEOUT, accepting).await {
                    Ok(Ok(stream)) => served.run(stream, failed).await,
                    Ok(Err(error)) => failed(&error),
                    Err(elapsed) => failed(&elapsed),
                }
                drop(held);
            });
            return;
        }
        tokio::spawn(async move {
            served.run(stream, failed).await;
            drop(held);
        });
    }
}

struct Served<S> {
    builder: Builder<TokioExecutor>,
    watcher: Watcher,
    service: S,
    /// Bounds a refused connection, which HTTP/2 would otherwise keep open for more streams.
    lifetime: Option<Duration>,
}

impl<S> Served<S> {
    async fn run<I, B>(self, io: I, failed: impl FnOnce(&(dyn std::error::Error + 'static)))
    where
        I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
        S: Service<Request<Incoming>, Response = http::Response<B>, Error = Infallible>
            + Send
            + 'static,
        S::Future: Send + 'static,
        B: hyper::body::Body + Send + 'static,
        B::Data: Send,
        B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let connection = self
            .builder
            .serve_connection(TokioIo::new(io), self.service)
            .into_owned();
        let connection = self.watcher.watch(connection);
        let outcome = match self.lifetime {
            Some(lifetime) => match tokio::time::timeout(lifetime, connection).await {
                Ok(outcome) => outcome,
                Err(_) => return,
            },
            None => connection.await,
        };
        if let Err(error) = outcome {
            failed(&*error);
        }
    }
}

/// An idle keep-alive connection ends in hyper's header timeout, and a client may
/// reset or drop a connection at any time: neither is a protocol violation.
fn ends_routinely(error: &(dyn std::error::Error + 'static)) -> bool {
    std::iter::successors(Some(error), |error| error.source()).any(|cause| {
        if let Some(hyper) = cause.downcast_ref::<hyper::Error>() {
            return hyper.is_timeout()
                || hyper.is_incomplete_message()
                || hyper.is_closed()
                || hyper.is_canceled()
                || hyper.is_body_write_aborted()
                || hyper.is_shutdown();
        }
        cause.downcast_ref::<std::io::Error>().is_some_and(|io| {
            matches!(
                io.kind(),
                std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::TimedOut
            )
        })
    })
}

/// HTTP/2 forbids `Connection`; there the lifetime ends the connection instead.
async fn refuse(request: Request<Incoming>) -> Result<Response, Infallible> {
    let mut response = answer::response(
        StatusCode::SERVICE_UNAVAILABLE,
        Some(answer::TEXT),
        "Too many connections from this address; try again shortly.",
    );
    if request.version() < Version::HTTP_2 {
        response
            .headers_mut()
            .insert(CONNECTION, HeaderValue::from_static("close"));
    }
    Ok(response)
}

/// The engine's own files, and the island and client bundles the app's build compiled.
#[cfg(feature = "engine")]
fn served_assets(screens: &crate::Screens) -> Assets {
    let assets = Assets::embedded();
    #[cfg(feature = "islands")]
    let assets = assets.with_islands(screens);
    #[cfg(feature = "clients")]
    let assets = assets.with_clients(screens);
    let _ = screens;
    assets
}

/// In memory, as a paused clock outruns loopback TCP, whose bytes arrive asynchronously on macOS.
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, duplex};
    use tokio::time::Instant;

    use super::*;

    fn connected() -> DuplexStream {
        served(service_fn(|_| async {
            Ok::<_, Infallible>(http::Response::new(
                http_body_util::Empty::<bytes::Bytes>::new(),
            ))
        }))
    }

    fn served<S, B>(service: S) -> DuplexStream
    where
        S: hyper::service::Service<
                http::Request<hyper::body::Incoming>,
                Response = http::Response<B>,
                Error = Infallible,
            > + Send
            + 'static,
        S::Future: Send + 'static,
        B: hyper::body::Body<Data = bytes::Bytes, Error = Infallible> + Send + 'static,
    {
        let (client, server) = duplex(64 * 1024);
        let connection = connection_builder()
            .serve_connection(TokioIo::new(Silenced::new(server)), service)
            .into_owned();
        tokio::spawn(connection);
        client
    }

    async fn closed(client: &mut DuplexStream) -> (Duration, String) {
        let started = Instant::now();
        let mut answer = Vec::new();
        client.read_to_end(&mut answer).await.unwrap();
        (started.elapsed(), String::from_utf8(answer).unwrap())
    }

    fn within_a_second_of(elapsed: Duration, seconds: u64) -> bool {
        (Duration::from_secs(seconds)..Duration::from_secs(seconds + 1)).contains(&elapsed)
    }

    #[tokio::test(start_paused = true)]
    async fn a_client_that_sends_nothing_is_closed_after_ten_seconds() {
        let mut client = connected();

        let (elapsed, answer) = closed(&mut client).await;

        assert!(within_a_second_of(elapsed, 10), "{elapsed:?}");
        assert_eq!(answer, "");
    }

    #[tokio::test(start_paused = true)]
    async fn half_an_http2_preface_is_answered_400_after_a_minute_of_silence() {
        let mut client = connected();
        client.write_all(b"PRI * HTTP/2.0\r\n").await.unwrap();

        let (elapsed, answer) = closed(&mut client).await;

        assert!(within_a_second_of(elapsed, 60), "{elapsed:?}");
        assert!(answer.starts_with("HTTP/1.1 400"), "{answer}");
    }

    #[tokio::test(start_paused = true)]
    async fn an_idle_kept_alive_connection_is_closed_after_ten_seconds() {
        let mut client = connected();
        client
            .write_all(b"GET / HTTP/1.1\r\nHost: shop.test\r\n\r\n")
            .await
            .unwrap();

        let (elapsed, answer) = closed(&mut client).await;

        assert!(within_a_second_of(elapsed, 10), "{elapsed:?}");
        assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_connection_over_its_share_is_answered_503_and_closed() {
        let mut client = served(service_fn(refuse));
        client
            .write_all(b"GET / HTTP/1.1\r\nHost: shop.test\r\n\r\n")
            .await
            .unwrap();

        let (elapsed, answer) = closed(&mut client).await;

        assert!(elapsed < Duration::from_secs(1), "{elapsed:?}");
        assert!(answer.starts_with("HTTP/1.1 503"), "{answer}");
        assert!(answer.contains("connection: close"), "{answer}");
    }

    #[test]
    fn a_reset_or_timed_out_connection_ends_routinely() {
        for kind in [
            std::io::ErrorKind::ConnectionReset,
            std::io::ErrorKind::TimedOut,
        ] {
            assert!(ends_routinely(&std::io::Error::from(kind)), "{kind:?}");
        }
        assert!(!ends_routinely(&std::io::Error::from(
            std::io::ErrorKind::InvalidData
        )));
    }
}
