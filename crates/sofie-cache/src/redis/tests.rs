use std::path::PathBuf;
use std::process::{Child, Command as Process, Stdio};
use std::sync::Arc;

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{RootCertStore, ServerConfig};
use sofie_db::Db;
use sofie_postgres::Postgres;
use sofie_testing::TestDatabase;
use tokio_rustls::TlsAcceptor;

use super::fake::FakeRedis;
use super::*;
use crate::{Cache, Key, Store};

const ROOT: &[u8] = include_bytes!("../../../sofie-client/tests/certificates/root.der");
const SERVER_CERTIFICATE: &[u8] =
    include_bytes!("../../../sofie-client/tests/certificates/server.der");
const SERVER_KEY: &[u8] = include_bytes!("../../../sofie-client/tests/certificates/server.key.der");

fn url(port: u16, rest: &str) -> RedisUrl {
    RedisUrl::parse(&format!("redis://{rest}127.0.0.1:{port}")).unwrap()
}

async fn client(fake: &FakeRedis) -> Client {
    Client::connect(url(fake.port, "")).await.unwrap()
}

fn stored(value: &str, tags: &[&str], expires_in: Duration) -> Stored {
    Stored {
        value: value.into(),
        tags: tags.iter().map(ToString::to_string).collect(),
        expires_in,
    }
}

#[tokio::test]
async fn an_entry_is_read_back_with_its_tags_until_it_expires() {
    let fake = FakeRedis::start(None, None).await;
    let client = client(&fake).await;
    let entry = stored(
        "{\"total\":3}",
        &["order.orders"],
        Duration::from_millis(150),
    );

    client.set("order:totals", &entry).await.unwrap();
    let read = client.get("order:totals").await.unwrap().unwrap();

    assert_eq!(read.value, entry.value);
    assert_eq!(read.tags, entry.tags);
    assert!(read.expires_in <= entry.expires_in && !read.expires_in.is_zero());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(client.get("order:totals").await.unwrap(), None);
}

#[tokio::test]
async fn deleting_and_dropping_a_tag_remove_entries_and_the_tag_s_set() {
    let fake = FakeRedis::start(None, None).await;
    let client = client(&fake).await;
    let hour = Duration::from_secs(3600);
    client
        .set("a", &stored("1", &["order.orders"], hour))
        .await
        .unwrap();
    client
        .set("b", &stored("2", &["order.orders", "order.orders/7"], hour))
        .await
        .unwrap();
    client
        .set("c", &stored("3", &["catalog.products"], hour))
        .await
        .unwrap();
    client.set("d", &stored("4", &[], hour)).await.unwrap();

    client
        .drop_tagged(&["order.orders".to_owned()])
        .await
        .unwrap();
    client.delete(&["d".to_owned()]).await.unwrap();

    assert!(!fake.holds("a") && !fake.holds("b") && !fake.holds("d"));
    assert!(!fake.holds("sofie:tags:order.orders"));
    assert!(fake.holds("c"));
}

#[tokio::test]
async fn writing_under_a_tag_prunes_members_whose_entries_expired() {
    let fake = FakeRedis::start(None, None).await;
    let client = client(&fake).await;
    let old = Command::new("ZADD")
        .argument("sofie:tags:t")
        .argument("1")
        .argument("old");
    client.call(&[old]).await.unwrap();

    client
        .set("new", &stored("1", &["t"], Duration::from_secs(3600)))
        .await
        .unwrap();

    assert_eq!(fake.members("sofie:tags:t"), ["new"]);
}

#[tokio::test]
async fn dropping_a_tag_with_more_members_than_one_pop_removes_them_all() {
    let fake = FakeRedis::start(None, None).await;
    let client = client(&fake).await;
    let hour = Duration::from_secs(3600);
    for index in 0..1_100 {
        let key = format!("k{index}");
        client.set(&key, &stored("1", &["t"], hour)).await.unwrap();
    }

    client.drop_tagged(&["t".to_owned()]).await.unwrap();

    assert!(!fake.holds("k0") && !fake.holds("k1099"));
    assert!(!fake.holds("sofie:tags:t"));
}

#[tokio::test]
async fn a_drop_redis_fails_midway_leaves_every_key_in_its_tag_set() {
    let fake = FakeRedis::start(None, None).await;
    let client = client(&fake).await;
    let hour = Duration::from_secs(3600);
    for key in ["a", "b"] {
        client.set(key, &stored("1", &["t"], hour)).await.unwrap();
    }

    fake.refuse_after(1);
    let failed = client.drop_tagged(&["t".to_owned()]).await;
    fake.answer_all();

    assert!(failed.is_err());
    assert_eq!(fake.members("sofie:tags:t"), ["a", "b"]);
    client.drop_tagged(&["t".to_owned()]).await.unwrap();
    assert!(!fake.holds("a") && !fake.holds("b"));
}

#[tokio::test]
async fn an_entry_and_its_tags_are_written_in_one_transaction() {
    let fake = FakeRedis::start(None, None).await;
    let client = client(&fake).await;
    let hour = Duration::from_secs(3600);

    client
        .set("a", &stored("1", &["order.orders"], hour))
        .await
        .unwrap();

    assert_eq!(
        fake.transactions(),
        [["SET", "ZADD", "ZREMRANGEBYSCORE", "PEXPIRE"]]
    );
}

#[tokio::test]
async fn every_connection_signs_in_and_selects_its_database() {
    let fake = FakeRedis::start(Some("p@ss"), None).await;
    let refused = Client::connect(url(fake.port, ":wrong@")).await;
    let Err(error) = refused else {
        panic!("a wrong password was accepted");
    };
    assert!(error.to_string().contains("WRONGPASS"), "{error}");
    assert!(!error.to_string().contains("wrong"), "{error}");

    let address = format!("redis://app:p%40ss@127.0.0.1:{}/3", fake.port);
    let client = Client::connect(RedisUrl::parse(&address).unwrap())
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
async fn a_drop_longer_than_a_database_notification_reaches_peers_whole() {
    let fake = FakeRedis::start(None, None).await;
    let mut subscriber = client(&fake)
        .await
        .subscribe(crate::peers::CHANNEL)
        .await
        .unwrap();
    let peers = crate::peers::Peers::Redis(Arc::new(client(&fake).await));
    let dropped = crate::peers::Dropped {
        from: "a".to_owned(),
        tags: (0..200)
            .map(|record| format!("shop.order/{record:048}"))
            .collect(),
        ..crate::peers::Dropped::default()
    };

    peers.publish(&dropped).await.unwrap();

    let received = timeout(Duration::from_secs(2), subscriber.next()).await;
    let received: crate::peers::Dropped =
        serde_json::from_str(&received.unwrap().unwrap()[0]).unwrap();
    assert!(!received.everything);
    assert_eq!(received.tags, dropped.tags);
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

    let untrusted = Client::connect(address.clone()).await;
    let client = Client::reaching(Target::trusting(address, roots))
        .await
        .unwrap();
    client
        .set("k", &stored("v", &[], Duration::from_secs(5)))
        .await
        .unwrap();

    assert!(untrusted.is_err());
    assert_eq!(client.get("k").await.unwrap().unwrap().value.as_ref(), "v");
}

const TOTAL: Key<u32> = crate::key!("total");

async fn instance(database: &TestDatabase, port: u16) -> (Db, Cache) {
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    let cache = Cache::start(&db, Store::Redis(url(port, "")))
        .await
        .unwrap();
    (db, cache)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_value_set_in_one_instance_replaces_the_other_s_memory_copy() {
    let database = TestDatabase::create().await;
    let fake = FakeRedis::start(None, None).await;
    let (_first_db, first) = instance(&database, fake.port).await;
    let (_second_db, second) = instance(&database, fake.port).await;
    let hour = Duration::from_secs(3600);
    tokio::time::sleep(Duration::from_millis(100)).await;
    first.entry(&TOTAL).expires_in(hour).set(&1).await.unwrap();
    assert_eq!(second.entry(&TOTAL).get().await.unwrap(), Some(1));

    first.entry(&TOTAL).expires_in(hour).set(&2).await.unwrap();

    for _ in 0..50 {
        if second.entry(&TOTAL).get().await.unwrap() == Some(2) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the second instance kept its copy");
}

fn redis_server() -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|directory| directory.join("redis-server"))
        .find(|path| path.is_file())
}

struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn a_real_redis_server_answers_like_the_fake() {
    let Some(binary) = redis_server() else {
        eprintln!("no redis-server on PATH; skipped");
        return;
    };
    let port = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let _server = Running(
        Process::new(binary)
            .args([
                "--port",
                &port.to_string(),
                "--save",
                "",
                "--appendonly",
                "no",
            ])
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let mut connected = None;
    for _ in 0..50 {
        if let Ok(client) = Client::connect(url(port, "")).await {
            connected = Some(client);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let client = connected.expect("redis-server started");
    let hour = Duration::from_secs(3600);

    client.set("a", &stored("1", &["t"], hour)).await.unwrap();
    assert_eq!(client.get("a").await.unwrap().unwrap().value.as_ref(), "1");
    client.drop_tagged(&["t".to_owned()]).await.unwrap();
    assert_eq!(client.get("a").await.unwrap(), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn with_redis_down_waiters_take_the_leader_s_value_and_memory_keeps_it() {
    let database = TestDatabase::create().await;
    let fake = FakeRedis::start(None, None).await;
    let (_db, cache) = instance(&database, fake.port).await;
    fake.stop();
    let computed = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let callers: Vec<_> = (0..4)
        .map(|_| {
            let (cache, computed) = (cache.clone(), Arc::clone(&computed));
            tokio::spawn(async move {
                let entry = cache.entry(&TOTAL).expires_in(Duration::from_secs(3600));
                entry
                    .get_or_compute(|| async {
                        computed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        Ok(7)
                    })
                    .await
                    .unwrap()
            })
        })
        .collect();
    for caller in callers {
        assert_eq!(caller.await.unwrap(), 7);
    }

    assert_eq!(computed.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(cache.entry(&TOTAL).get().await.unwrap(), Some(7));
}
