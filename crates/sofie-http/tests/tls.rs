use std::path::Path;
use std::sync::Arc;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, RootCertStore};
use sofie_db::Db;
use sofie_http::{App, SigningKeys, Tls};
use sofie_testing::FakeDriver;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio_rustls::TlsConnector;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(FIXTURES).join(name)
}

fn connector(alpn: &[&[u8]]) -> TlsConnector {
    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from_pem_file(fixture("localhost.crt")).unwrap())
        .unwrap();
    let mut config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
    config.alpn_protocols = alpn.iter().map(|protocol| protocol.to_vec()).collect();
    TlsConnector::from(Arc::new(config))
}

async fn serving() -> (std::net::SocketAddr, oneshot::Sender<()>) {
    let tls = Tls::from_pem_files(&fixture("localhost.crt"), &fixture("localhost.key")).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel();
    let app = App::new(Db::new(FakeDriver::default())).serving(|serving| {
        serving
            .signing_keys(SigningKeys::generate().unwrap())
            .tls(tls)
    });
    tokio::spawn(app.serve(listener, async { drop(stopped.await) }));
    (address, stop)
}

#[tokio::test]
async fn answers_https_and_negotiates_http2() {
    let (address, _stop) = serving().await;
    let name = ServerName::try_from("localhost").unwrap();

    let stream = TcpStream::connect(address).await.unwrap();
    let mut https = connector(&[b"http/1.1"])
        .connect(name.clone(), stream)
        .await
        .unwrap();
    https
        .write_all(b"GET /live HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut answer = String::new();
    https.read_to_string(&mut answer).await.unwrap();

    let stream = TcpStream::connect(address).await.unwrap();
    let h2 = connector(&[b"h2", b"http/1.1"])
        .connect(name, stream)
        .await
        .unwrap();

    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert_eq!(h2.get_ref().1.alpn_protocol(), Some(&b"h2"[..]));
}

#[tokio::test]
async fn plain_http_gets_no_answer() {
    let (address, _stop) = serving().await;

    let mut stream = TcpStream::connect(address).await.unwrap();
    stream
        .write_all(b"GET /live HTTP/1.1\r\nhost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut answer = Vec::new();
    let read = stream.read_to_end(&mut answer).await;

    assert!(
        !String::from_utf8_lossy(&answer).contains("200"),
        "{read:?}"
    );
}

#[test]
fn a_key_that_is_not_the_certificates_is_refused() {
    let refused = Tls::from_pem_files(&fixture("localhost.crt"), &fixture("other.key"));

    let error = refused
        .err()
        .expect("a mismatched key is refused")
        .to_string();
    assert!(error.contains("localhost.crt"), "{error}");
}
