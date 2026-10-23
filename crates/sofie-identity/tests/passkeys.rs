#![cfg(feature = "passkeys")]

use ring::digest::{SHA256, digest};
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::{Value, json};
use sofie::core::encoding::base64::{self, URL_SAFE};
use sofie::db::Db;
use sofie::prelude::*;
use sofie_identity::Memberships;
use sofie_identity::testing::signed_up;
use sofie_testing::{TestApp, TestResponse, TestSession};
use support::Member;

mod support;

/// An area admitting only confirmed addresses, for an account a member of both.
struct Verified;

impl Area for Verified {
    const NAME: &'static str = "verified";
    const PREFIX: &'static str = "/verified";
    const SIGN_IN: &'static str = "/verified/sign-in";
    type Principal = sofie_identity::Account;
}

impl sofie_identity::MemberArea for Verified {
    const HOME: &'static str = "/verified";
    const VERIFIED_EMAIL: bool = true;
}

async fn started() -> TestApp {
    sofie_testing::start(
        |db: Db| async move {
            db.schema().migrate(&[sofie_identity::migrations()]).await?;
            let mut screens = sofie::http::Screens::default();
            screens.add_builtin(sofie_identity::module(&[]).screens)?;
            screens.describe_forms(sofie_identity::FORMS)?;
            Ok(App::new(db)
                .screens(screens)
                .membership(Memberships)
                .area::<Web>("sofie_identity", sofie_identity::routes::<Member>)
                .area::<Web>("sofie_identity", sofie_identity::routes::<Verified>)
                .area::<Member>("sofie_identity", sofie_identity::passkeys::routes::<Member>))
        },
        sofie::worker::Worker::new,
    )
    .await
}

/// A software authenticator holding one Ed25519 passkey.
struct Authenticator {
    pair: Ed25519KeyPair,
    credential: Vec<u8>,
    count: u32,
}

impl Authenticator {
    fn new() -> Self {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        Self {
            pair: Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap(),
            credential: vec![7; 16],
            count: 0,
        }
    }

    fn data(&self, rp_id: &str, flags: u8) -> Vec<u8> {
        let rp = digest(&SHA256, rp_id.as_bytes());
        [rp.as_ref(), &[flags], &self.count.to_be_bytes()].concat()
    }

    fn create(&self, origin: &str, challenge: &str) -> Value {
        let client = client_data("webauthn.create", origin, challenge);
        let cose = [
            &[0xa4, 0x01, 0x01, 0x03, 0x27, 0x20, 0x06, 0x21, 0x58, 0x20][..],
            self.pair.public_key().as_ref(),
        ]
        .concat();
        let data = [
            &self.data(&rp_id(origin), 0x45)[..],
            &[0; 16],
            &[0x00, 0x10],
            &self.credential,
            &cose,
        ]
        .concat();
        let mut object = vec![0xa3, 0x63, b'f', b'm', b't', 0x64, b'n', b'o', b'n', b'e'];
        object.extend([0x67, b'a', b't', b't', b'S', b't', b'm', b't', 0xa0]);
        object.extend([0x68, b'a', b'u', b't', b'h', b'D', b'a', b't', b'a', 0x58]);
        object.push(u8::try_from(data.len()).unwrap());
        object.extend(&data);
        json!({
            "id": encoded(&self.credential),
            "client_data": encoded(client.as_bytes()),
            "attestation_object": encoded(&object),
        })
    }

    fn get(&mut self, origin: &str, challenge: &str) -> Value {
        self.get_flagged(origin, challenge, 0x05)
    }

    fn get_flagged(&mut self, origin: &str, challenge: &str, flags: u8) -> Value {
        self.count += 1;
        let client = client_data("webauthn.get", origin, challenge);
        let data = self.data(&rp_id(origin), flags);
        let signed = [&data[..], digest(&SHA256, client.as_bytes()).as_ref()].concat();
        json!({
            "id": encoded(&self.credential),
            "client_data": encoded(client.as_bytes()),
            "authenticator_data": encoded(&data),
            "signature": encoded(self.pair.sign(&signed).as_ref()),
        })
    }
}

fn encoded(bytes: &[u8]) -> String {
    base64::encode(&URL_SAFE, bytes)
}

fn client_data(kind: &str, origin: &str, challenge: &str) -> String {
    json!({ "type": kind, "challenge": challenge, "origin": origin }).to_string()
}

fn rp_id(origin: &str) -> String {
    let host = origin.split_once("://").unwrap().1;
    host.split(':').next().unwrap().to_owned()
}

fn options(json: &Value) -> Value {
    serde_json::from_str(json.as_str().unwrap()).unwrap()
}

async fn posted(
    app: &TestApp,
    path: &str,
    session: Option<&TestSession>,
    body: &Value,
) -> TestResponse {
    let request = app
        .post(path)
        .header("origin", app.origin())
        .header("sec-fetch-site", "same-origin")
        .accept_json()
        .json(body);
    match session {
        Some(session) => request.session(session).await,
        None => request.await,
    }
}

async fn sign_in_challenge(app: &TestApp) -> String {
    let page = app.get("/sign-in").accept_json().await.json();
    assert_eq!(page["passkey"]["post"], "/account/passkey");
    options(&page["passkey"]["options"])["challenge"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn a_registered_passkey_signs_in_once_per_challenge() {
    let app = started().await;
    let session = signed_up::<Member>(&app, "ada@example.com", "correct horse battery").await;
    let mut authenticator = Authenticator::new();

    let page = app
        .get("/account/passkeys")
        .session(&session)
        .accept_json()
        .await
        .json();
    let created = options(&page["options"]);
    assert_eq!(created["rp"]["name"], "sofie", "the app, not the area");
    assert_eq!(created["user"]["name"], "ada@example.com");
    let registration = authenticator.create(app.origin(), created["challenge"].as_str().unwrap());
    posted(&app, "/account/passkeys", Some(&session), &registration)
        .await
        .assert_ok();
    let twice = posted(&app, "/account/passkeys", Some(&session), &registration).await;
    assert_eq!(twice.status, 422, "{}", twice.body);

    let challenge = sign_in_challenge(&app).await;
    let assertion = authenticator.get(app.origin(), &challenge);
    let signed_in = posted(&app, "/account/passkey", None, &assertion).await;
    let replayed = posted(&app, "/account/passkey", None, &assertion).await;

    signed_in.assert_ok();
    assert_eq!(signed_in.json()["location"], "/account");
    let session = signed_in.session().expect("a passkey signs in");
    assert!(app.principal_of::<Member>(&session).is_some());
    assert_eq!(replayed.status, 422, "{}", replayed.body);
    let elsewhere = authenticator.get("https://evil.example", &sign_in_challenge(&app).await);
    let phished = posted(&app, "/account/passkey", None, &elsewhere).await;
    assert_eq!(phished.status, 422, "{}", phished.body);
}

#[tokio::test]
async fn a_passkey_that_did_not_verify_its_user_signs_nobody_in() {
    let app = started().await;
    let session = signed_up::<Member>(&app, "ada@example.com", "correct horse battery").await;
    let mut authenticator = Authenticator::new();
    let page = app
        .get("/account/passkeys")
        .session(&session)
        .accept_json()
        .await
        .json();
    let registration = authenticator.create(
        app.origin(),
        options(&page["options"])["challenge"].as_str().unwrap(),
    );
    posted(&app, "/account/passkeys", Some(&session), &registration)
        .await
        .assert_ok();
    let sign_in_page = app.get("/sign-in").accept_json().await.json();
    assert_eq!(
        options(&sign_in_page["passkey"]["options"])["userVerification"],
        "required"
    );

    let present_only =
        authenticator.get_flagged(app.origin(), &sign_in_challenge(&app).await, 0x01);
    let refused = posted(&app, "/account/passkey", None, &present_only).await;

    assert_eq!(refused.status, 422, "{}", refused.body);
    assert!(refused.session().is_none());
}

#[tokio::test]
async fn a_removed_passkey_is_added_again() {
    let app = started().await;
    let session = signed_up::<Member>(&app, "ada@example.com", "correct horse battery").await;
    let authenticator = Authenticator::new();
    let page = |app: &TestApp| app.get("/account/passkeys").session(&session).accept_json();
    let registered = |page: Value| {
        authenticator.create(
            app.origin(),
            options(&page["options"])["challenge"].as_str().unwrap(),
        )
    };
    let first = registered(page(&app).await.json());
    posted(&app, "/account/passkeys", Some(&session), &first)
        .await
        .assert_ok();
    let listed = page(&app).await.json();
    let id = listed["passkeys"][0]["id"].as_str().unwrap();
    posted(
        &app,
        &format!("/account/passkeys/{id}/remove"),
        Some(&session),
        &json!({}),
    )
    .await
    .assert_ok();

    let again = registered(page(&app).await.json());
    posted(&app, "/account/passkeys", Some(&session), &again)
        .await
        .assert_ok();
}

#[tokio::test]
async fn a_passkey_signs_in_where_a_confirmed_address_is_needed_only_once_confirmed() {
    let app = started().await;
    let session = signed_up::<Member>(&app, "ada@example.com", "correct horse battery").await;
    app.query(
        "INSERT INTO sofie_identity.membership (id, version, account, area, created_at, updated_at) \
         SELECT gen_random_uuid(), 1, account, 'verified', created_at, updated_at \
         FROM sofie_identity.membership WHERE area = 'member'",
        &[],
    )
    .await;
    let mut authenticator = Authenticator::new();
    let page = app
        .get("/account/passkeys")
        .session(&session)
        .accept_json()
        .await
        .json();
    let created = options(&page["options"]);
    let registration = authenticator.create(app.origin(), created["challenge"].as_str().unwrap());
    posted(&app, "/account/passkeys", Some(&session), &registration)
        .await
        .assert_ok();
    let assertion = authenticator.get(app.origin(), &sign_in_challenge(&app).await);
    let unconfirmed = posted(&app, "/verified/passkey", None, &assertion).await;
    app.query("UPDATE sofie_identity.account SET verified_at = now()", &[])
        .await;
    let assertion = authenticator.get(app.origin(), &sign_in_challenge(&app).await);
    let confirmed = posted(&app, "/verified/passkey", None, &assertion).await;

    assert_eq!(unconfirmed.status, 422, "{}", unconfirmed.body);
    assert!(unconfirmed.session().is_none());
    confirmed.assert_ok();
    assert!(confirmed.session().is_some());
}

#[tokio::test]
async fn adding_a_passkey_needs_a_sign_in_from_the_last_ten_minutes() {
    let app = started().await;
    let session = signed_up::<Member>(&app, "ada@example.com", "correct horse battery").await;
    app.clock()
        .advance(std::time::Duration::from_mins(11))
        .unwrap();
    let page = app
        .get("/account/passkeys")
        .session(&session)
        .accept_json()
        .await
        .json();
    let created = options(&page["options"]);
    let registration =
        Authenticator::new().create(app.origin(), created["challenge"].as_str().unwrap());

    let refused = posted(&app, "/account/passkeys", Some(&session), &registration).await;

    assert_eq!(refused.status, 422, "{}", refused.body);
    assert!(refused.body.contains("Sign in again"), "{}", refused.body);
}
