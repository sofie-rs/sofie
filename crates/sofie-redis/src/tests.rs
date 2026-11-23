use std::sync::Arc;

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{RootCertStore, ServerConfig};
use tokio_rustls::TlsAcceptor;

use super::*;
use crate::fake::FakeRedis;

const ROOT: &[u8] = include_bytes!("../../sofie-client/tests/certificates/root.der");
const SERVER_CERTIFICATE: &[u8] =
    include_bytes!("../../sofie-client/tests/certificates/server.der");
const SERVER_KEY: &[u8] = include_bytes!("../../sofie-client/tests/certificates/server.key.der");

fn url(port: u16, rest: &str) -> RedisUrl {
    RedisUrl::parse(&format!("redis://{rest}127.0.0.1:{port}")).unwrap()
}

async fn client(fake: &FakeRedis) -> Redis {
    Redis::connect(url(fake.port, "")).await.unwrap()
}

#[tokio::test]
async fn a_value_is_read_back_until_it_expires_or_is_deleted() {
    let fake = FakeRedis::start(None, None).await;
    let redis = client(&fake).await;

    redis
        .set("visits", "3", Duration::from_millis(150))
        .await
        .unwrap();
    redis
        .set("other", b"4".to_vec(), Duration::from_secs(60))
        .await
        .unwrap();

    assert_eq!(redis.get("visits").await.unwrap(), Some(b"3".to_vec()));
    redis.delete(&["other"]).await.unwrap();
    assert_eq!(redis.get("other").await.unwrap(), None);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(redis.get("visits").await.unwrap(), None);
}

#[tokio::test]
async fn a_refused_command_fails_naming_it_without_its_arguments() {
    let fake = FakeRedis::start(None, None).await;
    let redis = client(&fake).await;

    let error = redis
        .call(&[Command::new("NOSUCHCOMMAND").argument("secret-argument")])
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains("NOSUCHCOMMAND"), "{error}");
    assert!(!error.contains("secret-argument"), "{error}");
}

struct Handler(Db);

impl Context for Handler {
    fn db(&self) -> &Db {
        &self.0
    }

    fn reach(&self) -> sofie_db::Reach {
        sofie_db::Reach::All
    }
}

#[tokio::test]
async fn a_handler_takes_the_redis_installed_on_its_database() {
    let fake = FakeRedis::start(None, None).await;
    let db = Db::new(sofie_sqlite::Sqlite::connect("sqlite::memory:").unwrap());
    let mut handler = Handler(db.clone());
    let missing = Redis::resolve(&mut handler).await.err().unwrap();
    assert!(missing.to_string().contains("SOFIE_REDIS_URL"), "{missing}");

    client(&fake).await.install(&db);
    let redis = Redis::resolve(&mut handler).await.unwrap();
    redis.set("k", "v", Duration::from_secs(5)).await.unwrap();

    assert!(fake.holds("k"));
}

#[tokio::test]
async fn every_connection_signs_in_and_selects_its_database() {
    let fake = FakeRedis::start(Some("p@ss"), None).await;
    let refused = Redis::connect(url(fake.port, ":wrong@")).await;
    let Err(error) = refused else {
        panic!("a wrong password was accepted");
    };
    assert!(error.to_string().contains("WRONGPASS"), "{error}");
    assert!(!error.to_string().contains("wrong"), "{error}");

    let address = format!("redis://app:p%40ss@127.0.0.1:{}/3", fake.port);
    let client = Redis::connect(RedisUrl::parse(&address).unwrap())
        .await
        .unwrap();
    client.publish("anyone", "x").await.unwrap();

    assert_eq!(fake.selected(), vec!["3".to_owned()]);
}

#[tokio::test]
async fn a_connection_redis_closed_is_replaced_by_a_new_one() {
    let fake = FakeRedis::start(None, None).await;
    let client = client(&fake).await;

    fake.close_connections();
    tokio::time::sleep(Duration::from_millis(20)).await;
    client.publish("anyone", "x").await.unwrap();

    assert_eq!(fake.connections(), 2);
}

#[tokio::test]
async fn an_unreachable_redis_is_dialled_again_only_after_a_backoff() {
    let fake = FakeRedis::start(None, None).await;
    let client = client(&fake).await;
    fake.stop();
    tokio::time::sleep(Duration::from_millis(20)).await;

    let first = client.publish("anyone", "x").await.unwrap_err().to_string();
    let second = client.publish("anyone", "x").await.unwrap_err().to_string();

    assert!(first.contains("connecting to Redis"), "{first}");
    assert!(second.contains("connecting again in"), "{second}");
}

#[tokio::test]
async fn a_subscriber_receives_what_another_connection_publishes() {
    let fake = FakeRedis::start(None, None).await;
    let client = client(&fake).await;
    let mut subscriber = client.subscribe("sofie_cache").await.unwrap();

    client
        .publish("sofie_cache", "{\"from\":\"a\"}")
        .await
        .unwrap();

    let received = timeout(Duration::from_secs(2), subscriber.next())
        .await
        .unwrap();
    assert_eq!(received.unwrap(), vec!["{\"from\":\"a\"}".to_owned()]);
}

#[tokio::test]
async fn rediss_speaks_tls_and_checks_the_server_s_certificate() {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(SERVER_KEY.to_vec()));
    let config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![CertificateDer::from(SERVER_CERTIFICATE.to_vec())], key)
        .unwrap();
    let fake = FakeRedis::start(None, Some(TlsAcceptor::from(Arc::new(config)))).await;
    let address = RedisUrl::parse(&format!("rediss://localhost:{}", fake.port)).unwrap();
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from(ROOT.to_vec())).unwrap();

    let untrusted = Redis::connect(address.clone()).await;
    let client = Redis::reaching(Target::trusting(address, roots))
        .await
        .unwrap();
    client.set("k", "v", Duration::from_secs(5)).await.unwrap();

    assert!(untrusted.is_err());
    assert_eq!(client.get("k").await.unwrap(), Some(b"v".to_vec()));
}
