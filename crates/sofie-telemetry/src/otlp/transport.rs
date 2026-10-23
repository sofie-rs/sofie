//! OTLP/HTTP: each request a POST of one protobuf body, over one kept-alive connection.

use std::fmt;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use http::header::{CONTENT_TYPE, HOST, RETRY_AFTER};
use http::{HeaderMap, Request, StatusCode};
use http_body_util::{BodyExt as _, Full, Limited};
use hyper::client::conn::http1::{self, SendRequest};
use hyper_util::rt::TokioIo;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use super::settings::Settings;

const RESPONSE_LIMIT: usize = 64 * 1024;

type Sender = SendRequest<Full<Bytes>>;

const PROTOBUF: &str = "application/x-protobuf";
const ATTEMPTS: u32 = 4;
const FIRST_BACKOFF: Duration = Duration::from_secs(1);
/// A longer `Retry-After` is cut to this, so a shutdown's flush still ends.
const LONGEST_WAIT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub(crate) struct ExportError(String);

impl fmt::Display for ExportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

enum Failure {
    Transient {
        reason: String,
        retry_after: Option<Duration>,
    },
    Rejected(String),
}

impl Failure {
    fn transient(reason: impl fmt::Display) -> Self {
        Self::Transient {
            reason: reason.to_string(),
            retry_after: None,
        }
    }
}

pub(crate) struct Client {
    settings: Arc<Settings>,
    path: String,
    tls: TlsConnector,
    connection: Option<Sender>,
}

impl Client {
    pub(crate) fn new(settings: Arc<Settings>, signal_path: &str) -> Self {
        Self {
            path: format!("{}{signal_path}", settings.endpoint.path),
            settings,
            tls: TlsConnector::from(tls_config()),
            connection: None,
        }
    }

    /// Retries what a collector answers 429, 502, 503 or 504 to, and a request that never got
    /// an answer, waiting as long as `Retry-After` asks or else backing off.
    pub(crate) async fn export(&mut self, body: Bytes) -> Result<(), ExportError> {
        let mut backoff = FIRST_BACKOFF;
        let mut attempt = 1;
        loop {
            match self.attempt(body.clone()).await {
                Ok(()) => return Ok(()),
                Err(Failure::Rejected(reason)) => {
                    return Err(ExportError(format!(
                        "the collector refused the export to {}: {reason}",
                        self.path
                    )));
                }
                Err(Failure::Transient { reason, .. }) if attempt == ATTEMPTS => {
                    return Err(ExportError(format!(
                        "exporting to {} failed {ATTEMPTS} times, last: {reason}",
                        self.path
                    )));
                }
                Err(Failure::Transient { retry_after, .. }) => {
                    tokio::time::sleep(retry_after.unwrap_or(backoff).min(LONGEST_WAIT)).await;
                    backoff *= 2;
                    attempt += 1;
                }
            }
        }
    }

    async fn attempt(&mut self, body: Bytes) -> Result<(), Failure> {
        let timeout = self.settings.timeout;
        tokio::time::timeout(timeout, self.send(body))
            .await
            .unwrap_or_else(|_| Err(Failure::transient(format!("no answer within {timeout:?}"))))
    }

    async fn send(&mut self, body: Bytes) -> Result<(), Failure> {
        let mut sender = self.sender().await?;
        let response = sender
            .send_request(self.request(body)?)
            .await
            .map_err(Failure::transient)?;
        let status = response.status();
        let retry_after = retry_after(response.headers());
        // Read to its end, or the connection cannot carry the next request; a collector's
        // answer is a status and a short message, so a longer one is cut rather than held.
        Limited::new(response.into_body(), RESPONSE_LIMIT)
            .collect()
            .await
            .map_err(Failure::transient)?;
        self.connection = Some(sender);
        if status.is_success() {
            return Ok(());
        }
        if is_transient(status) {
            return Err(Failure::Transient {
                reason: format!("answered {status}"),
                retry_after,
            });
        }
        Err(Failure::Rejected(format!("answered {status}")))
    }

    fn request(&self, body: Bytes) -> Result<Request<Full<Bytes>>, Failure> {
        let mut request = Request::post(self.path.as_str())
            .header(HOST, self.settings.endpoint.authority.as_str())
            .header(CONTENT_TYPE, PROTOBUF)
            .body(Full::new(body))
            .map_err(|error| Failure::Rejected(format!("building the request: {error}")))?;
        for (name, value) in &self.settings.headers {
            request.headers_mut().insert(name.clone(), value.clone());
        }
        Ok(request)
    }

    async fn sender(&mut self) -> Result<Sender, Failure> {
        if let Some(mut sender) = self.connection.take()
            && sender.ready().await.is_ok()
        {
            return Ok(sender);
        }
        let mut sender = self.connect().await?;
        sender.ready().await.map_err(Failure::transient)?;
        Ok(sender)
    }

    async fn connect(&self) -> Result<Sender, Failure> {
        let endpoint = &self.settings.endpoint;
        let stream = TcpStream::connect((endpoint.host.as_str(), endpoint.port))
            .await
            .map_err(|error| {
                Failure::transient(format!("connecting to {}: {error}", endpoint.authority))
            })?;
        stream.set_nodelay(true).map_err(Failure::transient)?;
        if !endpoint.secure {
            return handshake(TokioIo::new(stream)).await;
        }
        let name = ServerName::try_from(endpoint.host.clone())
            .map_err(|error| Failure::Rejected(format!("{}: {error}", endpoint.host)))?;
        let stream = self
            .tls
            .connect(name, stream)
            .await
            .map_err(Failure::transient)?;
        handshake(TokioIo::new(stream)).await
    }
}

async fn handshake<Io>(io: Io) -> Result<Sender, Failure>
where
    Io: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    let (sender, connection) = http1::handshake(io).await.map_err(Failure::transient)?;
    // A closed connection shows as a sender that is no longer ready.
    tokio::spawn(async move { drop(connection.await) });
    Ok(sender)
}

fn is_transient(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::TOO_MANY_REQUESTS
            | StatusCode::BAD_GATEWAY
            | StatusCode::SERVICE_UNAVAILABLE
            | StatusCode::GATEWAY_TIMEOUT
    )
}

/// Only the delay in seconds: collectors do not send an HTTP date.
fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let seconds = headers
        .get(RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(Duration::from_secs(seconds))
}

fn tls_config() -> Arc<ClientConfig> {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    let config = CONFIG.get_or_init(|| {
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("ring's default provider supports the safe default protocol versions")
            .with_root_certificates(roots)
            .with_no_client_auth();
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Arc::new(config)
    });
    Arc::clone(config)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{SocketAddr, TcpListener};
    use std::sync::Arc;
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    use bytes::Bytes;

    use super::Client;
    use crate::otlp::settings::{Endpoint, Settings};

    pub(crate) struct Received {
        pub(crate) head: String,
        pub(crate) body: Vec<u8>,
    }

    /// Answers each request with the next status, on any connection, and hands back what came.
    pub(crate) fn collector(statuses: &'static [u16]) -> (SocketAddr, JoinHandle<Vec<Received>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let receiving = thread::spawn(move || {
            let mut received = Vec::new();
            while received.len() < statuses.len() {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream);
                while received.len() < statuses.len() {
                    let Some(request) = read_request(&mut reader) else {
                        break;
                    };
                    let status = statuses[received.len()];
                    let answer = format!(
                        "HTTP/1.1 {status} X\r\nretry-after: 0\r\ncontent-length: 0\r\n\r\n"
                    );
                    reader.get_mut().write_all(answer.as_bytes()).unwrap();
                    received.push(request);
                }
            }
            received
        });
        (address, receiving)
    }

    fn read_request(reader: &mut BufReader<std::net::TcpStream>) -> Option<Received> {
        let mut head = String::new();
        while !head.ends_with("\r\n\r\n") {
            if reader.read_line(&mut head).unwrap() == 0 {
                return None;
            }
        }
        let head = head.to_ascii_lowercase();
        let length = head
            .lines()
            .find_map(|line| line.strip_prefix("content-length: "))
            .unwrap()
            .parse()
            .unwrap();
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        Some(Received { head, body })
    }

    pub(crate) fn settings(address: SocketAddr) -> Arc<Settings> {
        Arc::new(Settings {
            endpoint: Endpoint::parse(&format!("http://{address}/otlp")).unwrap(),
            headers: vec![("x-tenant".try_into().unwrap(), "shop".try_into().unwrap())],
            timeout: Duration::from_secs(5),
            resource: Vec::new(),
        })
    }

    #[test]
    fn an_export_answered_503_is_sent_again() {
        let (address, receiving) = collector(&[503, 200]);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let result = runtime.block_on(async {
            let mut client = Client::new(settings(address), "/v1/traces");
            client.export(Bytes::from_static(b"spans")).await
        });

        result.unwrap();
        let received = receiving.join().unwrap();
        assert_eq!(received.len(), 2);
        for request in received {
            assert!(
                request.head.starts_with("post /otlp/v1/traces "),
                "{}",
                request.head
            );
            assert!(
                request
                    .head
                    .contains("content-type: application/x-protobuf"),
                "{}",
                request.head
            );
            assert!(request.head.contains("x-tenant: shop"), "{}", request.head);
            assert_eq!(request.body, b"spans");
        }
    }

    #[test]
    fn an_export_answered_400_is_not_sent_again() {
        let (address, receiving) = collector(&[400]);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let result = runtime.block_on(async {
            let mut client = Client::new(settings(address), "/v1/logs");
            client.export(Bytes::from_static(b"logs")).await
        });

        let error = result.unwrap_err().to_string();
        assert!(error.contains("400"), "{error}");
        assert_eq!(receiving.join().unwrap().len(), 1);
    }
}
