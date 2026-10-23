#![cfg(feature = "oauth")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use ring::rand::SystemRandom;
use ring::signature::{
    ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair, RSA_PKCS1_SHA256, RsaKeyPair,
    RsaPublicKeyComponents,
};
use serde_json::{Value, json};
use sofie::core::Result;
use sofie::core::encoding::base64::{self, URL_SAFE};
use sofie::db::{Context, Db, Outcome, ReadOnly, Reply, Resolve};
use sofie::prelude::*;
use sofie::values::Secret;
use sofie_identity::Memberships;
use sofie_identity::oauth::{OAuth, Provider};
use sofie_identity::testing::signed_up;
use sofie_testing::{FakeDriver, TestApp, TestResponse, TestServer};
use support::Member;

mod support;

const CLIENT: &str = "the-client";
const EC_KEY: &str = "ec-1";
const RSA_KEY: &str = "rsa-1";

struct Signer {
    ec: EcdsaKeyPair,
    rsa: RsaKeyPair,
    random: SystemRandom,
}

static SIGNER: LazyLock<Signer> = LazyLock::new(|| {
    let random = SystemRandom::new();
    let ec = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &random).unwrap();
    let ec =
        EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, ec.as_ref(), &random).unwrap();
    let rsa = RsaKeyPair::from_pkcs8(include_bytes!("fixtures/rsa_test_key.pk8")).unwrap();
    Signer { ec, rsa, random }
});

impl Signer {
    fn sign(&self, kid: &str, message: &[u8]) -> Vec<u8> {
        if kid == EC_KEY {
            return self
                .ec
                .sign(&self.random, message)
                .unwrap()
                .as_ref()
                .to_vec();
        }
        let mut signature = vec![0; self.rsa.public().modulus_len()];
        self.rsa
            .sign(&RSA_PKCS1_SHA256, &self.random, message, &mut signature)
            .unwrap();
        signature
    }

    fn jwk(&self, kid: &str) -> Value {
        let encoded = |bytes: &[u8]| base64::encode(&URL_SAFE, bytes);
        if kid == EC_KEY {
            let point = self.ec.public_key().as_ref();
            return json!({
                "kty": "EC", "crv": "P-256", "kid": kid, "use": "sig", "alg": "ES256",
                "x": encoded(&point[1..33]), "y": encoded(&point[33..]),
            });
        }
        let public = RsaPublicKeyComponents::<Vec<u8>>::from(self.rsa.public());
        json!({ "kty": "RSA", "kid": kid, "n": encoded(&public.n), "e": encoded(&public.e) })
    }
}

/// What the fake provider publishes; its origin is its issuer.
struct Published {
    issuer: String,
    kids: Mutex<Vec<&'static str>>,
    key_fetches: AtomicUsize,
}

struct Provided(Arc<Published>);

impl<C: Context> Resolve<C> for Provided {
    fn resolve(context: &mut C) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(Ok(Self(sofie::db::provided(context.db()).unwrap())))
    }
}

struct Json(Value);

impl Outcome for Json {
    fn reply(self) -> Result<Reply> {
        Ok(Reply::Json(self.0))
    }
}

impl ReadOnly for Json {}

async fn discovery(provided: Provided) -> Result<Json> {
    let issuer = &provided.0.issuer;
    Ok(Json(
        json!({ "issuer": issuer, "jwks_uri": format!("{issuer}/keys") }),
    ))
}

async fn keys(provided: Provided) -> Result<Json> {
    provided.0.key_fetches.fetch_add(1, Ordering::SeqCst);
    let kids = provided.0.kids.lock().unwrap().clone();
    let keys: Vec<Value> = kids.into_iter().map(|kid| SIGNER.jwk(kid)).collect();
    Ok(Json(json!({ "keys": keys })))
}

#[derive(Input)]
struct TokenRequest {
    grant_type: String,
    code: String,
    redirect_uri: String,
    client_id: String,
    client_secret: String,
    code_verifier: String,
}

/// The provider's token endpoint: the code is the ID token it answers.
async fn token(asked: TokenRequest) -> Result<Json> {
    let expected = (
        asked.grant_type.as_str(),
        asked.client_id.as_str(),
        asked.client_secret.as_str(),
    );
    if expected != ("authorization_code", CLIENT, "shh") || asked.code_verifier.len() < 43 {
        return Err(Error::Rule("not this client's code".to_owned()));
    }
    assert!(asked.redirect_uri.ends_with("/account/oauth/callback"));
    Ok(Json(
        json!({ "id_token": asked.code, "access_token": "unused" }),
    ))
}

struct FakeProvider {
    _server: TestServer,
    published: Arc<Published>,
}

impl FakeProvider {
    fn issuer(&self) -> &str {
        &self.published.issuer
    }

    fn key_fetches(&self) -> usize {
        self.published.key_fetches.load(Ordering::SeqCst)
    }
}

async fn provider() -> FakeProvider {
    let db = Db::new(FakeDriver::default());
    let server = TestServer::start(App::new(db.clone()).area(
        "provider",
        |routes: &mut Routes<Web>| {
            routes.get("/.well-known/openid-configuration", discovery);
            routes.get("/keys", keys);
            routes.post("/token", token);
        },
    ))
    .await;
    let published = Arc::new(Published {
        issuer: server.origin().to_owned(),
        kids: Mutex::new(vec![EC_KEY]),
        key_fetches: AtomicUsize::new(0),
    });
    sofie::db::provide(&db, Arc::clone(&published));
    FakeProvider {
        _server: server,
        published,
    }
}

async fn started(provider: &FakeProvider) -> TestApp {
    let issuer = provider.issuer().to_owned();
    let token_url = format!("{issuer}/token");
    sofie_testing::start(
        move |db: Db| async move {
            db.schema().migrate(&[sofie_identity::migrations()]).await?;
            let http = sofie_client::for_vendor(
                "oauth",
                sofie_client::Vendor {
                    hosts: Some("127.0.0.1".to_owned()),
                    private: Some(true),
                    ..sofie_client::Vendor::default()
                },
            )?;
            let client = (CLIENT.to_owned(), Secret::new("shh".to_owned()));
            let example = Provider::openid(
                "example",
                "Example",
                &issuer,
                "https://issuer.example/authorize",
                &token_url,
                client,
            )?;
            sofie_identity::oauth::install(
                &db,
                OAuth::new("http://localhost", http, vec![example])?,
            );
            let mut screens = sofie::http::Screens::default();
            screens.add_builtin(sofie_identity::module(&[]).screens)?;
            screens.describe_forms(sofie_identity::FORMS)?;
            Ok(App::new(db)
                .screens(screens)
                .membership(Memberships)
                .area::<Web>("sofie_identity", sofie_identity::routes::<Member>)
                .area::<Member>("sofie_identity", sofie_identity::oauth::routes::<Member>)
                .area::<Member>(
                    "sofie_identity",
                    sofie_identity::account_data::routes::<Member>,
                ))
        },
        sofie::worker::Worker::new,
    )
    .await
}

fn decoded(text: &str) -> String {
    String::from_utf8(sofie::core::encoding::percent::decode_form(text.as_bytes()).unwrap())
        .unwrap()
}

/// The provider's redirect, its sealed state and the browser's flow cookie.
struct Begun {
    state: String,
    nonce: String,
    flow: String,
}

fn begun(answer: &TestResponse) -> Begun {
    answer.assert_ok();
    let location = answer.json()["location"].as_str().unwrap().to_owned();
    assert!(
        location.starts_with("https://issuer.example/authorize?"),
        "{location}"
    );
    let parameter = |name: &str| {
        let query = location.split_once('?').unwrap().1;
        let raw = query
            .split('&')
            .find_map(|pair| pair.strip_prefix(&format!("{name}=")))
            .unwrap();
        decoded(raw)
    };
    assert_eq!(parameter("code_challenge_method"), "S256");
    assert_eq!(
        parameter("redirect_uri"),
        "http://localhost/account/oauth/callback"
    );
    let flow = answer
        .header("set-cookie")
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    assert!(flow.contains("-flow="), "{flow}");
    Begun {
        state: parameter("state"),
        nonce: parameter("nonce"),
        flow,
    }
}

fn claims(provider: &FakeProvider, begun: &Begun, subject: &str, email: &str) -> Value {
    json!({
        "iss": provider.issuer(),
        "aud": CLIENT,
        "exp": 4_000_000_000_i64,
        "iat": 1_000_000_000_i64,
        "nonce": begun.nonce,
        "sub": subject,
        "email": email,
        "email_verified": true,
    })
}

fn unsigned(header: &Value, claims: &Value) -> String {
    let encoded = |value: &Value| base64::encode(&URL_SAFE, value.to_string().as_bytes());
    format!("{}.{}", encoded(header), encoded(claims))
}

fn signed_by(kid: &str, header: &Value, claims: &Value) -> String {
    let message = unsigned(header, claims);
    let signature = SIGNER.sign(kid, message.as_bytes());
    format!("{message}.{}", base64::encode(&URL_SAFE, &signature))
}

fn code(provider: &FakeProvider, begun: &Begun, subject: &str, email: &str) -> String {
    let header = json!({ "alg": "ES256", "kid": EC_KEY });
    signed_by(EC_KEY, &header, &claims(provider, begun, subject, email))
}

async fn returned(app: &TestApp, begun: &Begun, code: &str, flow: &str) -> TestResponse {
    app.post("/account/oauth/callback")
        .header("cookie", flow)
        .header("sec-fetch-site", "same-origin")
        .accept_json()
        .json(&json!({ "code": code, "state": begun.state }))
        .await
}

#[tokio::test]
async fn a_provider_signs_up_a_new_address_and_only_the_browser_that_began() {
    let provider = provider().await;
    let app = started(&provider).await;
    let page = app.get("/sign-in").accept_json().await.json();
    assert_eq!(page["oauth"]["providers"][0]["label"], "Example");

    let begun = begun(
        &app.post("/account/oauth")
            .json(&json!({ "provider": "example" }))
            .await,
    );
    let code = code(&provider, &begun, "subject-1", "ada@example.com");
    let callback = app
        .get(&format!(
            "/account/oauth/callback?code={code}&state={}",
            sofie::http::path_segment(&begun.state)
        ))
        .await;
    assert!(
        callback.body.contains("data-sofie-autosubmit"),
        "{}",
        callback.body
    );
    let elsewhere = returned(&app, &begun, &code, "__Host-other-flow=x").await;
    let signed_up = returned(&app, &begun, &code, &begun.flow).await;

    assert_eq!(elsewhere.status, 422, "{}", elsewhere.body);
    signed_up.assert_status(204);
    let session = signed_up.session().expect("signed up and in");
    let connected = app
        .get("/account/connections")
        .session(&session)
        .accept_json()
        .await
        .json();
    assert_eq!(connected["connected"][0]["provider"], "example");
}

#[tokio::test]
async fn an_existing_address_is_connected_only_from_its_signed_in_account() {
    let provider = provider().await;
    let app = started(&provider).await;
    let session = signed_up::<Member>(&app, "ada@example.com", "correct horse battery").await;

    let begun_out = begun(
        &app.post("/account/oauth")
            .json(&json!({ "provider": "example" }))
            .await,
    );
    let code_out = code(&provider, &begun_out, "subject-2", "ada@example.com");
    let taken = returned(&app, &begun_out, &code_out, &begun_out.flow).await;
    assert_eq!(taken.status, 422, "{}", taken.body);

    let connecting = app
        .post("/account/connections")
        .session(&session)
        .accept_json()
        .json(&json!({ "provider": "example" }))
        .await;
    let begun_in = begun(&connecting);
    let linked = returned(
        &app,
        &begun_in,
        &code(&provider, &begun_in, "subject-2", "ada@example.com"),
        &begun_in.flow,
    )
    .await;
    linked.assert_ok();
    assert_eq!(linked.json()["location"], "/account/connections");

    let again = begun(
        &app.post("/account/oauth")
            .json(&json!({ "provider": "example" }))
            .await,
    );
    let signed_in = returned(
        &app,
        &again,
        &code(&provider, &again, "subject-2", "other@example.com"),
        &again.flow,
    )
    .await;
    let session = signed_in
        .session()
        .expect("the connected provider signs in");
    assert!(app.principal_of::<Member>(&session).is_some());
}

#[tokio::test]
async fn an_account_closed_while_connecting_gets_no_provider() {
    let provider = provider().await;
    let app = started(&provider).await;
    let session = signed_up::<Member>(&app, "ada@example.com", "correct horse battery").await;
    let connecting = app
        .post("/account/connections")
        .session(&session)
        .accept_json()
        .json(&json!({ "provider": "example" }))
        .await;
    let begun = begun(&connecting);
    app.post("/account/account-data/close")
        .session(&session)
        .accept_json()
        .json(&json!({ "email": "ada@example.com" }))
        .await
        .assert_ok();

    let linked = returned(
        &app,
        &begun,
        &code(&provider, &begun, "subject-3", "ada@example.com"),
        &begun.flow,
    )
    .await;

    assert_eq!(linked.status, 422, "{}", linked.body);
    let identities = app
        .query(
            "SELECT subject FROM sofie_identity.account_identity WHERE subject = 'subject-3'",
            &[],
        )
        .await;
    assert!(identities.is_empty(), "{identities:?}");
}

#[tokio::test]
async fn connecting_a_provider_needs_a_sign_in_from_the_last_ten_minutes() {
    let provider = provider().await;
    let app = started(&provider).await;
    let session = signed_up::<Member>(&app, "ada@example.com", "correct horse battery").await;
    app.clock()
        .advance(std::time::Duration::from_mins(11))
        .unwrap();

    let refused = app
        .post("/account/connections")
        .session(&session)
        .accept_json()
        .json(&json!({ "provider": "example" }))
        .await;

    assert_eq!(refused.status, 422, "{}", refused.body);
    assert!(refused.body.contains("Sign in again"), "{}", refused.body);
}

async fn begin_signing_in(app: &TestApp) -> Begun {
    begun(
        &app.post("/account/oauth")
            .json(&json!({ "provider": "example" }))
            .await,
    )
}

#[tokio::test]
async fn a_key_new_to_the_set_is_fetched_once_and_signs_with_rs256() {
    let provider = provider().await;
    let app = started(&provider).await;
    let first = begin_signing_in(&app).await;
    let code_first = code(&provider, &first, "subject-4", "ada@example.com");
    returned(&app, &first, &code_first, &first.flow)
        .await
        .assert_status(204);
    assert_eq!(provider.key_fetches(), 1);

    provider.published.kids.lock().unwrap().push(RSA_KEY);
    app.clock()
        .advance(std::time::Duration::from_mins(2))
        .unwrap();
    let rotated = begin_signing_in(&app).await;
    let header = json!({ "alg": "RS256", "kid": RSA_KEY });
    let claims_rotated = claims(&provider, &rotated, "subject-4", "ada@example.com");
    let code_rotated = signed_by(RSA_KEY, &header, &claims_rotated);
    let signed_in = returned(&app, &rotated, &code_rotated, &rotated.flow).await;
    assert!(signed_in.session().is_some(), "{}", signed_in.body);
    assert_eq!(provider.key_fetches(), 2);

    let made_up = begin_signing_in(&app).await;
    let header = json!({ "alg": "RS256", "kid": "made-up" });
    let claims_made_up = claims(&provider, &made_up, "subject-4", "ada@example.com");
    let code_made_up = signed_by(RSA_KEY, &header, &claims_made_up);
    let refused = returned(&app, &made_up, &code_made_up, &made_up.flow).await;
    assert_eq!(refused.status, 422, "{}", refused.body);
    assert_eq!(provider.key_fetches(), 2);
}

#[tokio::test]
async fn a_token_not_signed_by_the_providers_key_for_this_sign_in_is_refused() {
    let provider = provider().await;
    let app = started(&provider).await;
    let es256 = json!({ "alg": "ES256", "kid": EC_KEY });
    let tokens: [fn(&FakeProvider, &Begun, &Value) -> String; 5] = [
        |provider, begun, _| {
            let none = json!({ "alg": "none" });
            format!(
                "{}.",
                unsigned(&none, &claims(provider, begun, "s", "a@example.com"))
            )
        },
        |provider, begun, _| {
            let hmac = json!({ "alg": "HS256", "kid": EC_KEY });
            let claims = claims(provider, begun, "s", "a@example.com");
            format!("{}.c2ln", unsigned(&hmac, &claims))
        },
        |provider, begun, _| {
            let mismatched = json!({ "alg": "RS256", "kid": EC_KEY });
            signed_by(
                EC_KEY,
                &mismatched,
                &claims(provider, begun, "s", "a@example.com"),
            )
        },
        |provider, begun, es256| {
            let signed = signed_by(
                EC_KEY,
                es256,
                &claims(provider, begun, "s", "a@example.com"),
            );
            let other = unsigned(es256, &claims(provider, begun, "t", "b@example.com"));
            let signature = signed.rsplit_once('.').unwrap().1;
            format!("{other}.{signature}")
        },
        |provider, begun, es256| {
            let mut claims = claims(provider, begun, "s", "a@example.com");
            claims["nonce"] = json!("another sign-in");
            signed_by(EC_KEY, es256, &claims)
        },
    ];
    for token in tokens {
        let begun = begin_signing_in(&app).await;
        let code = token(&provider, &begun, &es256);
        let refused = returned(&app, &begun, &code, &begun.flow).await;
        assert_eq!(refused.status, 422, "{}", refused.body);
        assert!(refused.session().is_none());
    }
}
