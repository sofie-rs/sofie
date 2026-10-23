use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body_util::Full;
use hyper::client::conn::http1::{self, SendRequest};
use hyper_util::rt::TokioIo;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};
use sofie_core::{Error, Result, unpoisoned};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use crate::guard::{Failure, Reach};
use crate::target::Target;

pub(crate) type Sender = SendRequest<Full<Bytes>>;

/// Below the minute servers idle out after, so a request is rarely sent on a closing connection.
const IDLE_FOR: Duration = Duration::from_secs(20);
const IDLE_PER_HOST: usize = 8;
/// hyper takes a released connection back only once its task has run again after the response.
const READY_WITHIN: Duration = Duration::from_millis(50);
const MAX_HOSTS: usize = 64;

pub(crate) struct Connector {
    tls: TlsConnector,
    idle: Mutex<HashMap<PoolKey, Vec<Idle>>>,
}

/// Untrusted URLs are never pooled: they name hosts without end, and a vendor's connection may
/// lead to an allowed internal address.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) struct PoolKey {
    reach: Reach,
    secure: bool,
    host: String,
    port: u16,
}

impl PoolKey {
    pub(crate) fn of(target: &Target, reach: Reach) -> Self {
        Self {
            reach,
            secure: target.secure,
            host: target.host.clone(),
            port: target.port,
        }
    }
}

struct Idle {
    sender: Sender,
    since: Instant,
}

impl Idle {
    fn usable(&self) -> bool {
        self.since.elapsed() < IDLE_FOR && !self.sender.is_closed()
    }
}

impl Connector {
    pub(crate) fn trusting_public_roots() -> Self {
        static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
        let config = CONFIG.get_or_init(|| {
            let mut roots = RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            Arc::new(client_config(roots))
        });
        Self::with(Arc::clone(config))
    }

    #[cfg(feature = "testing")]
    pub(crate) fn trusting(roots: RootCertStore) -> Self {
        Self::with(Arc::new(client_config(roots)))
    }

    fn with(config: Arc<ClientConfig>) -> Self {
        Self {
            tls: TlsConnector::from(config),
            idle: Mutex::default(),
        }
    }

    pub(crate) async fn reuse(&self, key: &PoolKey) -> Option<Sender> {
        while let Some(mut sender) = self.take_idle(key) {
            let ready = tokio::time::timeout(READY_WITHIN, sender.ready()).await;
            if matches!(ready, Ok(Ok(()))) {
                return Some(sender);
            }
        }
        None
    }

    fn take_idle(&self, key: &PoolKey) -> Option<Sender> {
        let mut idle = self.idle();
        let connections = idle.get_mut(key)?;
        let reused = std::iter::from_fn(|| connections.pop())
            .find(Idle::usable)
            .map(|connection| connection.sender);
        if connections.is_empty() {
            idle.remove(key);
        }
        reused
    }

    /// Sweeps stale connections first, so none stays open past `IDLE_FOR` for long.
    pub(crate) fn release(&self, key: PoolKey, sender: Sender) {
        if key.reach == Reach::Untrusted || sender.is_closed() {
            return;
        }
        let mut idle = self.idle();
        idle.retain(|_, connections| {
            connections.retain(Idle::usable);
            !connections.is_empty()
        });
        if idle.len() >= MAX_HOSTS && !idle.contains_key(&key) {
            return;
        }
        let connections = idle.entry(key).or_default();
        if connections.len() < IDLE_PER_HOST {
            connections.push(Idle {
                sender,
                since: Instant::now(),
            });
        }
    }

    fn idle(&self) -> MutexGuard<'_, HashMap<PoolKey, Vec<Idle>>> {
        unpoisoned::lock(&self.idle)
    }

    /// `addresses` were already checked, so the host cannot resolve elsewhere in between. Each
    /// address gets an even share of what is left before `deadline`, so a silent one leaves the
    /// rest time to answer.
    pub(crate) async fn open(
        &self,
        target: &Target,
        addresses: &[SocketAddr],
        deadline: Instant,
    ) -> Result<Sender, Failure> {
        let mut last_error = String::new();
        for (index, address) in addresses.iter().enumerate() {
            let untried = u32::try_from(addresses.len() - index).unwrap_or(u32::MAX);
            let share = deadline.saturating_duration_since(Instant::now()) / untried;
            match tokio::time::timeout(share, TcpStream::connect(address)).await {
                Ok(Ok(stream)) => return self.handshake(target, stream).await,
                Ok(Err(error)) => last_error = error.to_string(),
                Err(_) => last_error = format!("{address} did not answer in time"),
            }
        }
        Err(failed(target, &last_error))
    }

    async fn handshake(&self, target: &Target, stream: TcpStream) -> Result<Sender, Failure> {
        stream
            .set_nodelay(true)
            .map_err(|error| failed(target, &error))?;
        if !target.secure {
            return http1_handshake(target, TokioIo::new(stream)).await;
        }
        let name =
            ServerName::try_from(target.host.clone()).map_err(|error| failed(target, &error))?;
        let stream = self
            .tls
            .connect(name, stream)
            .await
            .map_err(|error| failed(target, &error))?;
        http1_handshake(target, TokioIo::new(stream)).await
    }
}

fn client_config(roots: RootCertStore) -> ClientConfig {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("ring's default provider supports the safe default protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    config
}

async fn http1_handshake<Io>(target: &Target, io: Io) -> Result<Sender, Failure>
where
    Io: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    let (sender, connection) = http1::handshake(io)
        .await
        .map_err(|error| failed(target, &error))?;
    let host = target.host.clone();
    tokio::spawn(async move {
        if let Err(error) = connection.await {
            tracing::debug!(server.address = host, error = %error, "outgoing connection closed");
        }
    });
    Ok(sender)
}

fn failed(target: &Target, error: &dyn std::fmt::Display) -> Failure {
    Failure::Failed(Error::Unavailable(format!(
        "connecting to `{}` failed: {error}",
        target.host
    )))
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
    use tokio::net::TcpListener;

    use super::*;
    use crate::guard::Reach;

    /// A connection held open by the returned peer.
    async fn ready_sender() -> (Sender, DuplexStream) {
        let (client, server) = tokio::io::duplex(64);
        let (mut sender, connection) = http1::handshake(TokioIo::new(client)).await.unwrap();
        tokio::spawn(connection);
        sender.ready().await.unwrap();
        (sender, server)
    }

    fn key(host: usize, reach: Reach) -> PoolKey {
        PoolKey::of(
            &Target::parse(&format!("https://host-{host}.test")).unwrap(),
            reach,
        )
    }

    #[tokio::test]
    async fn the_pool_forgets_an_emptied_host_and_caps_how_many_it_keeps() {
        let connector = Connector::trusting_public_roots();
        let mut peers = Vec::new();
        for host in 0..=MAX_HOSTS {
            let (sender, peer) = ready_sender().await;
            peers.push(peer);
            connector.release(key(host, Reach::Vendor), sender);
        }
        let kept = connector.idle().len();
        let reused = connector.reuse(&key(0, Reach::Vendor)).await;

        assert_eq!(kept, MAX_HOSTS);
        assert!(reused.is_some());
        assert!(!connector.idle().contains_key(&key(0, Reach::Vendor)));
    }

    #[tokio::test]
    async fn a_connection_released_before_it_is_ready_again_is_reused() {
        let connector = Connector::trusting_public_roots();
        let (client, mut server) = tokio::io::duplex(1024);
        let (mut sender, connection) = http1::handshake(TokioIo::new(client)).await.unwrap();
        tokio::spawn(connection);
        sender.ready().await.unwrap();
        let request = http::Request::get("/")
            .header("host", "host-0.test")
            .body(Full::new(Bytes::new()))
            .unwrap();
        let answered = sender.send_request(request);

        connector.release(key(0, Reach::Vendor), sender);
        let mut received = [0; 1024];
        let _ = server.read(&mut received).await.unwrap();
        server
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
            .await
            .unwrap();
        answered.await.unwrap();

        assert!(connector.reuse(&key(0, Reach::Vendor)).await.is_some());
    }

    #[tokio::test]
    async fn an_untrusted_reach_is_not_pooled() {
        let connector = Connector::trusting_public_roots();
        let (sender, _peer) = ready_sender().await;

        connector.release(key(0, Reach::Untrusted), sender);

        assert!(connector.idle().is_empty());
    }

    #[tokio::test]
    async fn releasing_a_connection_closes_those_idle_too_long() {
        let connector = Connector::trusting_public_roots();
        let (stale, _stale_peer) = ready_sender().await;
        connector.idle().insert(
            key(0, Reach::Vendor),
            vec![Idle {
                sender: stale,
                since: Instant::now().checked_sub(IDLE_FOR).unwrap(),
            }],
        );
        let (sender, _peer) = ready_sender().await;

        connector.release(key(1, Reach::Vendor), sender);

        assert!(!connector.idle().contains_key(&key(0, Reach::Vendor)));
        assert!(connector.idle().contains_key(&key(1, Reach::Vendor)));
    }

    #[tokio::test]
    async fn a_silent_address_leaves_the_next_one_a_share_of_the_budget() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listening = listener.local_addr().unwrap();
        let target = Target::parse(&format!("http://127.0.0.1:{}", listening.port())).unwrap();
        // Unrouted, so a connection attempt is dropped unanswered rather than refused.
        let silent: SocketAddr = "10.255.255.1:80".parse().unwrap();
        let budget = Duration::from_secs(2);

        let opened = tokio::time::timeout(
            budget * 2,
            Connector::trusting_public_roots().open(
                &target,
                &[silent, listening],
                Instant::now() + budget,
            ),
        )
        .await;

        assert!(matches!(opened, Ok(Ok(_))));
    }
}
