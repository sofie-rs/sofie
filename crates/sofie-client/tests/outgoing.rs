//! `Outgoing` against a local TLS server whose certificate only the test's own root signs.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde_json::{Value, json};
use sofie_client::{Outgoing, Vendor};
use sofie_core::{Error, unpoisoned};
use sofie_db::Work;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

const ROOT: &[u8] = include_bytes!("certificates/root.der");
const SERVER_CERTIFICATE: &[u8] = include_bytes!("certificates/server.der");
const SERVER_KEY: &[u8] = include_bytes!("certificates/server.key.der");

#[derive(Default)]
struct Seen {
    flaky: AtomicUsize,
    keys: Mutex<Vec<String>>,
}

struct Server {
    port: u16,
    seen: Arc<Seen>,
}

impl Server {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let acceptor = TlsAcceptor::from(Arc::new(server_config()));
        let seen = Arc::new(Seen::default());
        let serving = Arc::clone(&seen);
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let (acceptor, seen) = (acceptor.clone(), Arc::clone(&serving));
                tokio::spawn(async move {
                    let Ok(stream) = acceptor.accept(stream).await else {
                        return;
                    };
                    let service = service_fn(move |request| answer(request, Arc::clone(&seen)));
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        Self { port, seen }
    }

    fn base_url(&self) -> String {
        format!("https://localhost:{}", self.port)
    }

    fn outgoing(&self, settings: Vendor) -> Outgoing {
        outgoing(Vendor {
            base_url: Some(self.base_url()),
            private: Some(true),
            ..settings
        })
    }
}

fn outgoing(settings: Vendor) -> Outgoing {
    sofie_client::testing::outgoing("vendor", settings, ROOT).unwrap()
}

fn server_config() -> ServerConfig {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(SERVER_KEY.to_vec()));
    ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![CertificateDer::from(SERVER_CERTIFICATE.to_vec())], key)
        .unwrap()
}

async fn answer(
    request: Request<Incoming>,
    seen: Arc<Seen>,
) -> Result<Response<Full<Bytes>>, std::convert::Infallible> {
    if let Some(key) = request.headers().get("idempotency-key") {
        let key = key.to_str().unwrap().to_owned();
        unpoisoned::lock(&seen.keys).push(key);
    }
    let reply = |status: StatusCode, body: &str| {
        let mut response = Response::new(Full::new(Bytes::from(body.to_owned())));
        *response.status_mut() = status;
        response
    };
    let path = request.uri().path();
    let lookup = path.starts_with("/_sofie/lookups/");
    let still_failing =
        (path == "/flaky" || lookup) && seen.flaky.fetch_add(1, Ordering::SeqCst) < 2;
    let response = match path {
        "/flaky" if still_failing => reply(StatusCode::SERVICE_UNAVAILABLE, ""),
        _ if lookup && still_failing => reply(StatusCode::SERVICE_UNAVAILABLE, ""),
        _ if lookup => reply(StatusCode::OK, "[]"),
        "/account"
            if request
                .headers()
                .get("authorization")
                .is_some_and(|value| value == "Bearer sk_test") =>
        {
            reply(StatusCode::OK, r#"{"ok":true}"#)
        }
        "/ok" | "/flaky" => reply(StatusCode::OK, r#"{"ok":true}"#),
        "/big" => reply(StatusCode::OK, &"x".repeat(4096)),
        "/agent" => {
            let agent = request.headers()["user-agent"].to_str().unwrap();
            reply(StatusCode::OK, agent)
        }
        "/slow" => {
            tokio::time::sleep(Duration::from_secs(5)).await;
            reply(StatusCode::OK, "late")
        }
        "/down" => reply(StatusCode::SERVICE_UNAVAILABLE, ""),
        "/redirect" => {
            let mut response = reply(StatusCode::FOUND, "");
            let location = request.headers()["x-location"].clone();
            response.headers_mut().insert("location", location);
            response
        }
        _ => reply(StatusCode::NOT_FOUND, ""),
    };
    Ok(response)
}

#[tokio::test]
async fn a_request_reaches_its_vendor_over_tls_trusting_the_test_s_root_only() {
    let server = Server::start().await;

    let answer: Value = server
        .outgoing(Vendor::default())
        .get("/account")
        .bearer("sk_test")
        .send()
        .await
        .unwrap()
        .json()
        .unwrap();
    let public_roots = sofie_client::for_vendor(
        "vendor",
        Vendor {
            base_url: Some(server.base_url()),
            private: Some(true),
            ..Vendor::default()
        },
    )
    .unwrap();
    let untrusted_certificate = public_roots.get("/ok").send().await.unwrap_err();

    assert_eq!(answer, json!({ "ok": true }));
    assert!(
        matches!(&untrusted_certificate, Error::Unavailable(message) if message.contains("certificate")),
        "{untrusted_certificate:?}"
    );
}

#[tokio::test]
async fn an_answer_using_a_vendor_s_reply_has_no_fingerprint() {
    let server = Server::start().await;
    let outgoing = server.outgoing(Vendor::default());

    let (sent, fingerprint) = sofie_db::track_reads(1, outgoing.get("/ok").send()).await;

    assert!(sent.is_ok(), "{sent:?}");
    assert_eq!(fingerprint, None);
}

#[tokio::test]
async fn internal_addresses_are_refused_unless_the_vendor_allows_them() {
    let server = Server::start().await;
    let public_only = outgoing(Vendor {
        base_url: Some(server.base_url()),
        ..Vendor::default()
    });
    let allowed = server.outgoing(Vendor::default());

    let refused = public_only.get("/ok").send().await.unwrap_err();
    let plain_http = Vendor {
        base_url: Some("http://vendor.test".to_owned()),
        ..Vendor::default()
    };
    let Err(plain) = sofie_client::testing::outgoing("vendor", plain_http, ROOT) else {
        panic!("a plain HTTP base URL is refused when read");
    };

    assert!(
        matches!(&refused, Error::Internal { context, .. } if context.contains("internal address")),
        "{refused:?}"
    );
    assert!(
        matches!(&plain, Error::Internal { context, .. } if context.contains("not HTTPS")),
        "{plain:?}"
    );
    assert_eq!(allowed.get("/ok").send().await.unwrap().status(), 200);
}

#[tokio::test]
async fn untrusted_urls_get_no_exception_and_no_redirect() {
    let server = Server::start().await;
    let outgoing = server.outgoing(Vendor {
        redirects: Some(3),
        ..Vendor::default()
    });
    let untrusted = outgoing.untrusted();

    for url in [
        format!("{}/ok", server.base_url()),
        "https://169.254.169.254/latest/meta-data".to_owned(),
        "https://[::ffff:127.0.0.1]/".to_owned(),
        "http://example.com/".to_owned(),
        "https://user:secret@example.com/".to_owned(),
    ] {
        let refused = untrusted.post(&url).send().await.unwrap_err();
        assert!(
            matches!(refused, Error::BadRequest(_)),
            "{url}: {refused:?}"
        );
    }
}

#[tokio::test]
async fn an_untrusted_host_is_refused_alike_whatever_it_resolves_to() {
    let server = Server::start().await;
    let untrusted = server.outgoing(Vendor::default());

    for host in [
        "127.0.0.1",
        "10.0.0.1",
        "169.254.169.254",
        "nothing.invalid",
    ] {
        let refused = untrusted
            .untrusted()
            .get(&format!("https://{host}/"))
            .send()
            .await
            .unwrap_err();
        let expected = format!(
            "the outgoing request to `{host}` is refused: it does not resolve to a public address"
        );
        let unresolvable = host == "nothing.invalid";
        assert!(
            match &refused {
                Error::BadRequest(message) => !unresolvable && *message == expected,
                Error::Unavailable(message) => unresolvable && *message == expected,
                _ => false,
            },
            "{host}: {refused:?}"
        );
    }
}

#[tokio::test]
async fn redirects_are_not_followed_unless_allowed_and_then_only_to_the_vendor_s_hosts() {
    let server = Server::start().await;
    let off = server.outgoing(Vendor::default());
    let on = server.outgoing(Vendor {
        redirects: Some(1),
        ..Vendor::default()
    });

    let kept = redirect(&off, "localhost", server.port).await.unwrap();
    let followed = redirect(&on, "localhost", server.port).await.unwrap();
    let elsewhere = redirect(&on, "127.0.0.1", server.port).await.unwrap_err();

    assert_eq!(kept.status(), 302);
    assert_eq!(followed.status(), 200);
    assert!(
        matches!(&elsewhere, Error::Internal { context, .. } if context.contains("not one the vendor uses")),
        "{elsewhere:?}"
    );
}

#[tokio::test]
async fn a_redirect_to_another_origin_leaves_the_caller_s_headers_behind() {
    let server = Server::start().await;
    let other = Server::start().await;
    let outgoing = server.outgoing(Vendor {
        redirects: Some(1),
        ..Vendor::default()
    });
    let to = |port: u16| format!("https://localhost:{port}/account");

    let same = outgoing
        .get("/redirect")
        .bearer("sk_test")
        .header("x-location", &to(server.port))
        .send()
        .await
        .unwrap();
    let elsewhere = outgoing
        .get("/redirect")
        .bearer("sk_test")
        .header("x-location", &to(other.port))
        .send()
        .await
        .unwrap();

    assert_eq!(same.status(), 200);
    assert_eq!(elsewhere.status(), 404, "the bearer token did not travel");
}

async fn redirect(
    outgoing: &Outgoing,
    host: &str,
    port: u16,
) -> sofie_core::Result<sofie_client::Response> {
    let location = format!("https://{host}:{port}/ok");
    outgoing
        .get("/redirect")
        .header("x-location", &location)
        .send()
        .await
}

#[tokio::test]
async fn a_caller_s_user_agent_is_kept_and_a_bad_content_type_is_returned_by_send() {
    let server = Server::start().await;
    let outgoing = server.outgoing(Vendor::default());

    let agent = outgoing
        .get("/agent")
        .header("user-agent", "shop/1.0")
        .send()
        .await
        .unwrap();
    let refused = outgoing
        .post("/ok")
        .body("text/plain\n", "hello")
        .send()
        .await
        .unwrap_err();

    assert_eq!(agent.body(), b"shop/1.0");
    assert!(
        matches!(&refused, Error::Internal { context, .. } if context.contains("content type")),
        "{refused:?}"
    );
}

#[tokio::test]
async fn timeouts_and_the_response_size_cap_bound_a_request() {
    let server = Server::start().await;
    let outgoing = server.outgoing(Vendor {
        timeout_ms: Some(300),
        max_response_bytes: Some(1024),
        ..Vendor::default()
    });

    let started = Instant::now();
    let late = outgoing.get("/slow").send().await.unwrap_err();
    let elapsed = started.elapsed();
    let big = outgoing.get("/big").send().await.unwrap_err();

    assert!(matches!(late, Error::Unavailable(_)), "{late:?}");
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
    assert!(
        matches!(&big, Error::Internal { context, .. } if context.contains("larger than 1024 bytes")),
        "{big:?}"
    );
}

#[tokio::test]
async fn only_an_idempotent_request_is_tried_again_and_its_key_is_sent() {
    let server = Server::start().await;
    let outgoing = server.outgoing(Vendor {
        retries: Some(2),
        ..Vendor::default()
    });
    let key = Work::new("charge-7".to_owned(), "fingerprint".to_owned()).key();

    let once = outgoing
        .post("/flaky")
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    let tries_after_once = server.seen.flaky.load(Ordering::SeqCst);
    server.seen.flaky.store(0, Ordering::SeqCst);
    let keyed = outgoing
        .post("/flaky")
        .idempotency(&key)
        .send()
        .await
        .unwrap();

    assert_eq!((once.status(), tries_after_once), (503, 1));
    assert_eq!(keyed.status(), 200);
    assert_eq!(server.seen.flaky.load(Ordering::SeqCst), 3);
    let keys = server.seen.keys.lock().unwrap().clone();
    assert_eq!(keys, ["charge-7", "charge-7", "charge-7"]);
}

#[cfg(feature = "remote-contracts")]
#[tokio::test]
async fn a_remote_lookup_is_a_read_tried_again() {
    use sofie_db::Lookup as _;

    let server = Server::start().await;
    let outgoing = server.outgoing(Vendor {
        retries: Some(2),
        ..Vendor::default()
    });
    let keys = sofie_auth::SigningKeys::generate().unwrap();
    let lookup = sofie_client::RemoteLookup::<Value>::new(sofie_client::RemoteApp::new(
        "vendor", outgoing, keys,
    ));

    let found = lookup
        .get_many(&[sofie_core::Uuid::now_v7().unwrap()])
        .await;

    assert_eq!(found.unwrap(), Vec::<Value>::new());
    assert_eq!(server.seen.flaky.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn a_request_counts_once_against_its_circuit_and_a_refusal_not_at_all() {
    let server = Server::start().await;
    let outgoing = server.outgoing(Vendor {
        retries: Some(2),
        max_response_bytes: Some(1024),
        ..Vendor::default()
    });

    for _ in 0..4 {
        let down = outgoing.get("/down").send().await.unwrap();
        assert_eq!(down.status(), 503);
    }
    let refused = outgoing.get("/big").send().await.unwrap_err();
    let fifth = outgoing.get("/down").send().await.unwrap();
    let open = outgoing.get("/ok").send().await.unwrap_err();

    assert!(matches!(refused, Error::Internal { .. }), "{refused:?}");
    assert_eq!(fifth.status(), 503);
    assert!(
        matches!(&open, Error::Unavailable(message) if message.contains("circuit")),
        "{open:?}"
    );
}

#[tokio::test]
async fn a_circuit_is_one_port_of_a_host() {
    let failing = Server::start().await;
    let other = Server::start().await;
    let outgoing = failing.outgoing(Vendor {
        retries: Some(0),
        ..Vendor::default()
    });
    for _ in 0..5 {
        outgoing.get("/down").send().await.unwrap();
    }

    let other_port = outgoing
        .get_url(&format!("{}/ok", other.base_url()))
        .send()
        .await
        .unwrap();

    assert_eq!(other_port.status(), 200);
}

#[tokio::test]
async fn a_request_that_runs_out_of_time_counts_against_its_circuit() {
    let server = Server::start().await;
    let outgoing = server.outgoing(Vendor {
        timeout_ms: Some(100),
        ..Vendor::default()
    });

    for _ in 0..5 {
        let late = outgoing.get("/slow").send().await.unwrap_err();
        assert!(matches!(late, Error::Unavailable(_)), "{late:?}");
    }
    let open = outgoing.get("/ok").send().await.unwrap_err();

    assert!(
        matches!(&open, Error::Unavailable(message) if message.contains("circuit")),
        "{open:?}"
    );
}

#[tokio::test]
async fn an_open_circuit_is_an_operation_s_outcome_but_not_a_new_failure() {
    let server = Server::start().await;
    let outgoing = server.outgoing(Vendor {
        retries: Some(0),
        ..Vendor::default()
    });
    for _ in 0..5 {
        outgoing.get("/down").send().await.unwrap();
    }

    for _ in 0..5 {
        let host_open = outgoing
            .operation("check", async {
                outgoing.get("/ok").send().await.map(drop)
            })
            .await;
        assert!(
            matches!(host_open, Err(Error::Unavailable(_))),
            "{host_open:?}"
        );
    }
    let operation = outgoing.operation("check", async { Ok(7) }).await;

    assert_eq!(operation.unwrap(), 7);
}

#[tokio::test]
async fn an_operation_is_bounded_by_the_vendor_s_timeout_and_breaker() {
    let outgoing = outgoing(Vendor {
        timeout_ms: Some(100),
        ..Vendor::default()
    });

    let sent = outgoing.operation("send", async { Ok(7) }).await;
    for _ in 0..5 {
        let late = outgoing
            .operation("send", async {
                tokio::time::sleep(Duration::from_secs(5)).await;
                Ok(())
            })
            .await;
        assert!(matches!(late, Err(Error::Unavailable(_))), "{late:?}");
    }
    let telemetry = sofie_telemetry::capture();
    let refused = outgoing
        .operation("send", async { Ok(()) })
        .await
        .unwrap_err();
    let no_base = outgoing.get("/ok").send().await.unwrap_err();

    let spans = telemetry.spans();
    let span = spans.iter().find(|span| span.name == "outgoing").unwrap();
    assert_eq!(sent.unwrap(), 7);
    assert!(matches!(&refused, Error::Unavailable(message) if message.contains("circuit")));
    assert_eq!(
        span.fields["outcome"], "error",
        "the refusal still finishes its span"
    );
    assert!(matches!(&no_base, Error::Internal { context, .. } if context.contains("BASE_URL")));
}

#[tokio::test]
async fn a_request_is_a_client_span_inside_its_gateway_call() {
    let server = Server::start().await;
    let outgoing = server.outgoing(Vendor::default());
    let telemetry = sofie_telemetry::capture();

    let status = sofie_client::call("vendor", "check", async {
        Ok(outgoing.get("/ok").send().await?.status())
    })
    .await
    .unwrap();

    let spans = telemetry.spans();
    let gateway = spans.iter().find(|span| span.name == "gateway").unwrap();
    let client = spans
        .iter()
        .find(|span| span.name == "http.client")
        .unwrap();
    assert_eq!(status, 200);
    assert_eq!(gateway.fields["otel.name"], "vendor.check");
    assert_eq!(gateway.fields["outcome"], "ok");
    assert_eq!(client.parent, Some(gateway.id));
    assert_eq!(client.fields["server.address"], "localhost");
    assert_eq!(client.fields["http.response.status_code"], "200");
}
