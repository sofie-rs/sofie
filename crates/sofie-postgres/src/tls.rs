//! TLS through rustls, as `sslmode` asks: `disable` never encrypts, `prefer` (the default)
//! encrypts when the server can, `require` always encrypts, and like libpq neither checks the
//! certificate; `verify-ca` also checks it is signed by a public root, or by one of the PEM
//! certificates in `sslrootcert=<path>`, `verify-full` also that it names the host.

use std::borrow::Cow;
use std::io;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, InvalidDnsNameError, ServerName, UnixTime};
use rustls::{
    CertificateError, ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_postgres::Socket;
use tokio_postgres::tls::{ChannelBinding, MakeTlsConnect, TlsConnect, TlsStream};

use sofie_core::encoding::percent;
use sofie_core::{Error, Result};

use crate::BoxFuture;

const ROOT_FILE: &str = "sslrootcert=";

pub(crate) type Tls = Rustls;
pub(crate) type Stream = Encrypted<Socket>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Check {
    /// Encrypted against an eavesdropper, not against a server in the middle.
    Nothing,
    SignedByRoot,
    SignedByRootForHost,
}

/// What the driver is handed in place of `url`, and how its certificate is checked.
pub(crate) fn configure(url: &str) -> Result<(String, Rustls)> {
    let (url, root_file) = take_root_file(url)?;
    let (url, check) = split_mode(&url);
    let roots = match root_file {
        None => public_roots(),
        Some(_) if check == Check::Nothing => {
            return Err(Error::internal_message(
                "`sslrootcert` is read only with `sslmode=verify-ca` or `sslmode=verify-full`",
            ));
        }
        Some(path) => roots_from_file(Path::new(&path))?,
    };
    Ok((url.into_owned(), Rustls::checking(check, roots)?))
}

/// tokio-postgres knows only `disable`, `prefer` and `require`, so a verifying mode is handed
/// to it as `require` and the check is kept here.
fn split_mode(url: &str) -> (Cow<'_, str>, Check) {
    let verifying = [
        ("sslmode=verify-full", Check::SignedByRootForHost),
        ("sslmode=verify-ca", Check::SignedByRoot),
    ];
    for (mode, check) in verifying {
        if url.contains(mode) {
            return (Cow::Owned(url.replace(mode, "sslmode=require")), check);
        }
    }
    (Cow::Borrowed(url), Check::Nothing)
}

/// tokio-postgres refuses an option it does not know, so `sslrootcert` is taken out of the url,
/// percent-decoded in the URL form, ending at whitespace in the keyword form.
fn take_root_file(url: &str) -> Result<(Cow<'_, str>, Option<String>)> {
    let is_url = url.starts_with("postgres://") || url.starts_with("postgresql://");
    let Some(start) = option_start(url, is_url) else {
        return Ok((Cow::Borrowed(url), None));
    };
    let value_start = start + ROOT_FILE.len();
    let value_end = url[value_start..]
        .find(|character: char| {
            if is_url {
                character == '&' || character == '#'
            } else {
                character.is_whitespace()
            }
        })
        .map_or(url.len(), |length| value_start + length);
    let value = &url[value_start..value_end];
    let path = if is_url {
        percent::decode(value.as_bytes())
            .and_then(|decoded| String::from_utf8(decoded).ok())
            .ok_or_else(|| {
                Error::internal_message(format!("`sslrootcert={value}` is not a valid path"))
            })?
    } else {
        value.to_owned()
    };
    if path == "system" {
        return Err(Error::internal_message(
            "`sslrootcert=system` is not supported: leave it out to trust the public roots, or \
             name a PEM file",
        ));
    }
    let (cut_start, cut_end) = match url[value_end..].chars().next() {
        Some('&') => (start, value_end + 1),
        _ if is_url => (start - 1, value_end),
        _ => (start, value_end),
    };
    let rest = format!("{}{}", &url[..cut_start], &url[cut_end..]);
    Ok((Cow::Owned(rest), Some(path)))
}

fn option_start(url: &str, is_url: bool) -> Option<usize> {
    url.match_indices(ROOT_FILE)
        .map(|(index, _)| index)
        .find(|&index| match url[..index].chars().next_back() {
            None => !is_url,
            Some(before) if is_url => before == '?' || before == '&',
            Some(before) => before.is_whitespace(),
        })
}

fn public_roots() -> RootCertStore {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    roots
}

fn roots_from_file(path: &Path) -> Result<RootCertStore> {
    let shown = path.display();
    let pem = std::fs::read(path)
        .map_err(|error| Error::internal(format!("reading `sslrootcert` {shown}"), error))?;
    let mut roots = RootCertStore::empty();
    for certificate in CertificateDer::pem_slice_iter(&pem) {
        let certificate = certificate.map_err(|error| {
            Error::internal(format!("reading a PEM certificate in {shown}"), error)
        })?;
        roots.add(certificate).map_err(|error| {
            Error::internal(format!("trusting a certificate in {shown}"), error)
        })?;
    }
    if roots.is_empty() {
        return Err(Error::internal_message(format!(
            "`sslrootcert` {shown} holds no PEM certificate"
        )));
    }
    Ok(roots)
}

#[derive(Clone)]
pub(crate) struct Rustls {
    config: Arc<ClientConfig>,
}

impl Rustls {
    fn checking(check: Check, roots: RootCertStore) -> Result<Self> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = Verifier::new(check, roots, Arc::clone(&provider))?;
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("ring's default provider supports the safe default protocol versions")
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_no_client_auth();
        Ok(Self {
            config: Arc::new(config),
        })
    }
}

impl<S> MakeTlsConnect<S> for Rustls
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    type Stream = Encrypted<S>;
    type TlsConnect = Connector;
    type Error = InvalidDnsNameError;

    fn make_tls_connect(&mut self, host: &str) -> Result<Connector, InvalidDnsNameError> {
        Ok(Connector {
            config: Arc::clone(&self.config),
            name: ServerName::try_from(host.to_owned())?,
        })
    }
}

pub(crate) struct Connector {
    config: Arc<ClientConfig>,
    name: ServerName<'static>,
}

impl<S> TlsConnect<S> for Connector
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    type Stream = Encrypted<S>;
    type Error = io::Error;
    type Future = BoxFuture<'static, io::Result<Encrypted<S>>>;

    fn connect(self, stream: S) -> Self::Future {
        let connector = tokio_rustls::TlsConnector::from(self.config);
        Box::pin(async move { connector.connect(self.name, stream).await.map(Encrypted) })
    }
}

pub(crate) struct Encrypted<S>(tokio_rustls::client::TlsStream<S>);

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncRead for Encrypted<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_read(context, buffer)
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncWrite for Encrypted<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(context, bytes)
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(context)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(context)
    }
}

/// Without channel binding, SCRAM authenticates as it does over a plain connection.
impl<S: AsyncRead + AsyncWrite + Unpin> TlsStream for Encrypted<S> {
    fn channel_binding(&self) -> ChannelBinding {
        ChannelBinding::none()
    }
}

#[derive(Debug)]
struct Verifier {
    webpki: Arc<WebPkiServerVerifier>,
    check: Check,
}

impl Verifier {
    fn new(
        check: Check,
        roots: RootCertStore,
        provider: Arc<rustls::crypto::CryptoProvider>,
    ) -> Result<Self> {
        let webpki = WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider)
            .build()
            .map_err(|error| Error::internal("building the Postgres certificate check", error))?;
        Ok(Self { webpki, check })
    }
}

impl ServerCertVerifier for Verifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if self.check == Check::Nothing {
            return Ok(ServerCertVerified::assertion());
        }
        let verified = self.webpki.verify_server_cert(
            end_entity,
            intermediates,
            server_name,
            ocsp_response,
            now,
        );
        match verified {
            Err(rustls::Error::InvalidCertificate(
                CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. },
            )) if self.check == Check::SignedByRoot => Ok(ServerCertVerified::assertion()),
            other => other,
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signed: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.webpki
            .verify_tls12_signature(message, certificate, signed)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signed: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.webpki
            .verify_tls13_signature(message, certificate, signed)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.webpki.supported_verify_schemes()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// Self-signed for `db.test` until 2126, made with `openssl req -x509 -newkey ec`.
    const SELF_SIGNED: &str = "-----BEGIN CERTIFICATE-----
MIIBjTCCATKgAwIBAgIUdwB8tJw7LVfHUT6MaOy3KNuvJbIwCgYIKoZIzj0EAwIw
EjEQMA4GA1UEAwwHZGIudGVzdDAgFw0yNjEwMTQyMzE1NTVaGA8yMTI2MDkyMDIz
MTU1NVowEjEQMA4GA1UEAwwHZGIudGVzdDBZMBMGByqGSM49AgEGCCqGSM49AwEH
A0IABEIC6cyGkSWkhZG2G4g3/OmThuKAWom/y//TNDlXlmiQnKh33NqO1v/QRlqQ
7WetoY2qgYsWlHAA5FZRASCW7wWjZDBiMB0GA1UdDgQWBBSO6dLCwqkBd5l2Kb5v
7pcVoLSWDDAfBgNVHSMEGDAWgBSO6dLCwqkBd5l2Kb5v7pcVoLSWDDASBgNVHREE
CzAJggdkYi50ZXN0MAwGA1UdEwEB/wQCMAAwCgYIKoZIzj0EAwIDSQAwRgIhAMAK
rchuwmVWg9sd5H63/YEb0wyf7JHWp3WlwrNx0dAmAiEA58fdvdtpPKXTr6SRPqbG
ZCVekNhoUeniti6NwS1BvAI=
-----END CERTIFICATE-----
";

    fn verifies(check: Check, roots: RootCertStore, host: &str) -> bool {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = Verifier::new(check, roots, provider).unwrap();
        let certificate = CertificateDer::from_pem_slice(SELF_SIGNED.as_bytes()).unwrap();
        let in_2027 = UnixTime::since_unix_epoch(Duration::from_hours(500_000));
        let name = ServerName::try_from(host.to_owned()).unwrap();
        verifier
            .verify_server_cert(&certificate, &[], &name, &[], in_2027)
            .is_ok()
    }

    #[test]
    fn a_root_file_is_trusted_in_place_of_the_public_roots() {
        let path = std::env::temp_dir().join(format!("sofie-root-{}.pem", std::process::id()));
        std::fs::write(&path, SELF_SIGNED).unwrap();
        let url = format!(
            "postgres://app@db.test/shop?sslmode=verify-full&sslrootcert={}",
            path.display()
        );

        let (taken, root_file) = take_root_file(&url).unwrap();
        let roots = roots_from_file(&path);
        std::fs::remove_file(&path).unwrap();

        assert_eq!(taken, "postgres://app@db.test/shop?sslmode=verify-full");
        assert_eq!(root_file, Some(path.display().to_string()));
        let roots = roots.unwrap();
        assert!(verifies(
            Check::SignedByRootForHost,
            roots.clone(),
            "db.test"
        ));
        assert!(!verifies(
            Check::SignedByRootForHost,
            roots.clone(),
            "other.test"
        ));
        assert!(verifies(Check::SignedByRoot, roots, "other.test"));
        assert!(!verifies(Check::SignedByRoot, public_roots(), "db.test"));
    }

    #[test]
    fn sslrootcert_is_taken_out_of_either_url_form() {
        let cases = [
            (
                "postgres://db.test/shop?sslrootcert=%2Fetc%2Fca.pem&sslmode=verify-ca",
                "postgres://db.test/shop?sslmode=verify-ca",
            ),
            (
                "host=db.test sslrootcert=/etc/ca.pem sslmode=verify-ca",
                "host=db.test  sslmode=verify-ca",
            ),
        ];
        for (url, rest) in cases {
            let (taken, root_file) = take_root_file(url).unwrap();

            assert_eq!(taken, rest);
            assert_eq!(root_file.as_deref(), Some("/etc/ca.pem"));
        }
    }

    #[test]
    fn a_root_file_that_cannot_be_used_is_a_boot_error_naming_it() {
        let missing = "/nonexistent/sofie-root.pem";
        let refused = [
            format!("postgres://db.test/shop?sslmode=verify-full&sslrootcert={missing}"),
            "postgres://db.test/shop?sslmode=verify-ca&sslrootcert=system".to_owned(),
            format!("postgres://db.test/shop?sslmode=require&sslrootcert={missing}"),
        ];
        let messages: Vec<String> = refused
            .iter()
            .map(|url| crate::Postgres::connect(url).err().unwrap().to_string())
            .collect();

        assert!(messages[0].contains(missing), "{}", messages[0]);
        assert!(
            messages[1].contains("sslrootcert=system"),
            "{}",
            messages[1]
        );
        assert!(messages[2].contains("verify-ca"), "{}", messages[2]);
    }

    #[test]
    fn a_verifying_mode_is_required_of_the_driver_and_checked_here() {
        let (url, check) = split_mode("postgres://app@db.example.com/shop?sslmode=verify-full");

        assert_eq!(url, "postgres://app@db.example.com/shop?sslmode=require");
        assert_eq!(check, Check::SignedByRootForHost);
        let (url, check) = split_mode("host=db.example.com sslmode=verify-ca dbname=shop");
        assert_eq!(url, "host=db.example.com sslmode=require dbname=shop");
        assert_eq!(check, Check::SignedByRoot);
    }

    #[test]
    fn the_driver_s_own_modes_pass_unchanged_and_check_no_certificate() {
        for url in [
            "postgres://localhost/shop",
            "postgres://localhost/shop?sslmode=disable",
            "postgres://localhost/shop?sslmode=require",
        ] {
            let (passed, check) = split_mode(url);

            assert_eq!(passed, url);
            assert_eq!(check, Check::Nothing);
        }
    }

    #[test]
    fn every_mode_libpq_knows_is_accepted() {
        for mode in ["disable", "prefer", "require", "verify-ca", "verify-full"] {
            let url = format!("postgres://nobody@localhost/nothing?sslmode={mode}");

            assert!(crate::Postgres::connect(&url).is_ok(), "{mode}");
        }
    }
}
