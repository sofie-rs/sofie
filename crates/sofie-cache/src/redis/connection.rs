use std::sync::{Arc, OnceLock};

use bytes::BytesMut;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};
use sofie_core::{Error, Result};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use super::resp::{self, Command, Reply};
use super::url::RedisUrl;

trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

/// TLS for `rediss://` trusts the public roots.
pub(crate) struct Target {
    pub(crate) url: RedisUrl,
    tls: Option<TlsConnector>,
}

impl Target {
    pub(crate) fn new(url: RedisUrl) -> Self {
        static PUBLIC_ROOTS: OnceLock<Arc<ClientConfig>> = OnceLock::new();
        let config = PUBLIC_ROOTS.get_or_init(|| {
            let mut roots = RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            Arc::new(client_config(roots))
        });
        Self::with_config(url, Arc::clone(config))
    }

    #[cfg(test)]
    pub(crate) fn trusting(url: RedisUrl, roots: RootCertStore) -> Self {
        Self::with_config(url, Arc::new(client_config(roots)))
    }

    fn with_config(url: RedisUrl, config: Arc<ClientConfig>) -> Self {
        let tls = url.secure.then(|| TlsConnector::from(config));
        Self { url, tls }
    }
}

fn client_config(roots: RootCertStore) -> ClientConfig {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("ring's default provider supports the safe default protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth()
}

pub(crate) struct Connection {
    stream: Box<dyn Stream>,
    buffer: BytesMut,
}

fn unreachable(target: &Target, error: &dyn std::fmt::Display) -> Error {
    Error::Unavailable(format!(
        "connecting to Redis at {}:{} failed: {error}",
        target.url.host, target.url.port
    ))
}

impl Connection {
    pub(crate) async fn open(target: &Target) -> Result<Self> {
        let url = &target.url;
        let tcp = TcpStream::connect((url.host.as_str(), url.port))
            .await
            .map_err(|error| unreachable(target, &error))?;
        tcp.set_nodelay(true)
            .map_err(|error| unreachable(target, &error))?;
        let stream: Box<dyn Stream> = match &target.tls {
            None => Box::new(tcp),
            Some(tls) => {
                let name = ServerName::try_from(url.host.clone())
                    .map_err(|error| unreachable(target, &error))?;
                Box::new(
                    tls.connect(name, tcp)
                        .await
                        .map_err(|error| unreachable(target, &error))?,
                )
            }
        };
        let mut connection = Self {
            stream,
            buffer: BytesMut::with_capacity(4096),
        };
        connection.prepare(url).await?;
        Ok(connection)
    }

    async fn prepare(&mut self, url: &RedisUrl) -> Result<()> {
        let mut commands = Vec::new();
        if let Some(password) = &url.password {
            let auth = Command::new("AUTH");
            let auth = match &url.user {
                Some(user) => auth.argument(user.as_str()),
                None => auth,
            };
            commands.push(auth.argument(password.as_str()));
        }
        if url.database != 0 {
            commands.push(Command::new("SELECT").argument(url.database.to_string()));
        }
        let replies = self.call(&commands).await?;
        checked(&commands, replies).map(drop)
    }

    pub(crate) async fn call(&mut self, commands: &[Command]) -> Result<Vec<Reply>> {
        self.send(commands).await?;
        let mut replies = Vec::with_capacity(commands.len());
        for _ in commands {
            replies.push(self.read().await?);
        }
        Ok(replies)
    }

    pub(crate) async fn send(&mut self, commands: &[Command]) -> Result<()> {
        if commands.is_empty() {
            return Ok(());
        }
        let mut out = BytesMut::with_capacity(commands.iter().map(Command::encoded_length).sum());
        for command in commands {
            command.encode(&mut out);
        }
        self.stream
            .write_all(&out)
            .await
            .map_err(|error| lost(&error))?;
        self.stream.flush().await.map_err(|error| lost(&error))
    }

    /// Cancel safe: what was read stays buffered for the next call.
    pub(crate) async fn read(&mut self) -> Result<Reply> {
        loop {
            if let Some(reply) = resp::parse(&mut self.buffer)? {
                return Ok(reply);
            }
            if self.buffer.len() > resp::MAX_REPLY_LENGTH {
                return Err(Error::Unavailable(format!(
                    "Redis sent a reply past {} bytes without finishing it",
                    resp::MAX_REPLY_LENGTH
                )));
            }
            let read = self
                .stream
                .read_buf(&mut self.buffer)
                .await
                .map_err(|error| lost(&error))?;
            if read == 0 {
                return Err(Error::Unavailable("Redis closed the connection".to_owned()));
            }
        }
    }
}

fn lost(error: &std::io::Error) -> Error {
    Error::Unavailable(format!("the Redis connection failed: {error}"))
}

/// An error reply fails the call, naming the command but never its arguments.
pub(crate) fn checked(commands: &[Command], replies: Vec<Reply>) -> Result<Vec<Reply>> {
    for (command, reply) in commands.iter().zip(&replies) {
        if let Reply::Error(message) = reply {
            return Err(Error::internal_message(format!(
                "Redis refused {}: {message}",
                command.name()
            )));
        }
    }
    Ok(replies)
}
