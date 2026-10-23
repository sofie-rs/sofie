//! A small SMTP client (RFC 5321) over tokio: EHLO, STARTTLS or implicit
//! TLS, AUTH PLAIN or LOGIN, then one message per connection.

mod conversation;
mod message;

use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};
use sofie_core::{Error, Result};
use sofie_db::BoxFuture;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

use crate::settings::{Security, SmtpSettings};
use crate::transport::{Envelope, Refused, Transport};
use conversation::{Conversation, DATA, Extensions, GREETING, MAIL_FROM, STARTTLS};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// A mail that has not left within this is retried by its job.
const TOTAL_TIMEOUT: Duration = Duration::from_secs(30);
const QUIT_TIMEOUT: Duration = Duration::from_secs(2);

trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<S: AsyncRead + AsyncWrite + Unpin + Send> Stream for S {}

type Open<'a> = Conversation<'a, Box<dyn Stream>>;

pub(crate) struct Smtp {
    settings: SmtpSettings,
    tls: TlsConnector,
    client: String,
}

impl Smtp {
    pub(crate) fn new(settings: SmtpSettings, client: String) -> Self {
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        Self::trusting(settings, client, roots)
    }

    pub(crate) fn trusting(settings: SmtpSettings, client: String, roots: RootCertStore) -> Self {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("ring's default provider supports the safe default protocol versions")
            .with_root_certificates(roots)
            .with_no_client_auth();
        Self {
            settings,
            tls: TlsConnector::from(Arc::new(config)),
            client,
        }
    }

    async fn transmit(&self, envelope: &Envelope<'_>) -> Result<Vec<Refused>> {
        let message = message::render(envelope)?;
        let sent = tokio::time::timeout(TOTAL_TIMEOUT, self.session(envelope, &message)).await;
        let (refused, mut conversation) = sent.unwrap_or_else(|_| {
            Err(Error::Unavailable(format!(
                "the SMTP server {} took longer than {} s",
                self.settings.host,
                TOTAL_TIMEOUT.as_secs()
            )))
        })?;
        // The server already took the mail: a slow QUIT failing the send would send it again.
        if tokio::time::timeout(QUIT_TIMEOUT, conversation.quit())
            .await
            .is_err()
        {
            tracing::debug!("the SMTP server did not answer QUIT in time");
        }
        Ok(refused)
    }

    async fn session(
        &self,
        envelope: &Envelope<'_>,
        message: &str,
    ) -> Result<(Vec<Refused>, Open<'_>)> {
        let host = self.settings.host.as_str();
        let stream = self.connect().await?;
        let stream: Box<dyn Stream> = match self.settings.security {
            Security::Tls => Box::new(self.handshake(stream).await?),
            Security::None => Box::new(stream),
            Security::StartTls => {
                let mut plain = Conversation::new(stream, host);
                plain.expect(GREETING).await?;
                if !plain.hello(&self.client).await?.starttls {
                    return Err(Error::Unavailable(format!(
                        "the SMTP server {host} does not offer STARTTLS; \
                         set SOFIE_MAIL_SMTP_SECURITY to `tls` if it expects TLS from the start"
                    )));
                }
                plain.send("STARTTLS", STARTTLS).await?;
                Box::new(self.handshake(plain.into_stream()?).await?)
            }
        };
        let mut conversation = Conversation::new(stream, host);
        if self.settings.security != Security::StartTls {
            conversation.expect(GREETING).await?;
        }
        let refused = self.deliver(&mut conversation, envelope, message).await?;
        Ok((refused, conversation))
    }

    async fn deliver(
        &self,
        conversation: &mut Open<'_>,
        envelope: &Envelope<'_>,
        message: &str,
    ) -> Result<Vec<Refused>> {
        let extensions = conversation.hello(&self.client).await?;
        if let Some(credentials) = &self.settings.credentials {
            let password = credentials.password.expose();
            conversation
                .authenticate(&extensions, &credentials.username, password)
                .await?;
        }
        let mail_from = self.mail_from(envelope, &extensions)?;
        conversation.send(&mail_from, MAIL_FROM).await?;
        let mut refused = Vec::new();
        for recipient in envelope.mail.to() {
            refused.extend(conversation.recipient(recipient).await?);
        }
        if refused.len() < envelope.mail.to().len() {
            conversation.send("DATA", DATA).await?;
            conversation.message(message).await?;
        }
        Ok(refused)
    }

    /// RFC 6531: an address beyond ASCII, in the envelope or the headers, goes only to a server
    /// offering SMTPUTF8.
    fn mail_from(&self, envelope: &Envelope<'_>, extensions: &Extensions) -> Result<String> {
        let from = envelope.from.email().as_str();
        if !is_international(envelope) {
            return Ok(format!("MAIL FROM:<{from}>"));
        }
        // Retrying asks the same server again, so the mail is buried at once.
        if !extensions.smtputf8 {
            return Err(Error::Rule(format!(
                "the SMTP server {} does not offer SMTPUTF8, which an address of this mail needs",
                self.settings.host
            )));
        }
        Ok(format!("MAIL FROM:<{from}> SMTPUTF8"))
    }

    async fn connect(&self) -> Result<TcpStream> {
        let address = (self.settings.host.as_str(), self.settings.port);
        let connected = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(address)).await;
        let stream = connected
            .map_err(|_| self.unreachable("it did not answer within 5 s"))?
            .map_err(|error| self.unreachable(&error.to_string()))?;
        stream
            .set_nodelay(true)
            .map_err(|error| self.unreachable(&error.to_string()))?;
        Ok(stream)
    }

    async fn handshake(&self, stream: TcpStream) -> Result<TlsStream<TcpStream>> {
        let name = ServerName::try_from(self.settings.host.clone())
            .map_err(|error| self.unreachable(&error.to_string()))?;
        self.tls
            .connect(name, stream)
            .await
            .map_err(|error| self.unreachable(&format!("TLS failed: {error}")))
    }

    fn unreachable(&self, problem: &str) -> Error {
        Error::Unavailable(format!(
            "connecting to the SMTP server {}:{}: {problem}",
            self.settings.host, self.settings.port
        ))
    }
}

fn is_international(envelope: &Envelope<'_>) -> bool {
    let mail = envelope.mail;
    std::iter::once(envelope.from)
        .chain(mail.to())
        .chain(mail.reply_to())
        .any(|address| !address.email().as_str().is_ascii())
}

impl Transport for Smtp {
    fn name(&self) -> &'static str {
        "smtp"
    }

    fn server(&self) -> Option<(&str, u16)> {
        Some((&self.settings.host, self.settings.port))
    }

    fn send<'a>(&'a self, envelope: &'a Envelope<'a>) -> BoxFuture<'a, Result<Vec<Refused>>> {
        Box::pin(self.transmit(envelope))
    }
}
