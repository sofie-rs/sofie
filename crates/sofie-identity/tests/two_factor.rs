#![cfg(feature = "totp")]

use std::time::Duration;

use ring::hmac;
use serde_json::{Value, json};
use sofie::db::Db;
use sofie::prelude::*;
use sofie_identity::Memberships;
use sofie_identity::testing::signed_up;
use sofie_testing::{TestApp, TestSession};
use support::Member;

mod support;

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
                .area::<Member>(
                    "sofie_identity",
                    sofie_identity::two_factor::routes::<Member>,
                ))
        },
        |db| sofie_identity::worker(sofie::worker::Worker::new(db)),
    )
    .await
}

fn credentials() -> Value {
    json!({ "email": "ada@example.com", "password": "correct horse battery" })
}

fn base32_decoded(text: &str) -> Vec<u8> {
    let (mut bytes, mut buffer, mut bits) = (Vec::new(), 0u32, 0);
    for symbol in text.bytes() {
        let value = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"
            .iter()
            .position(|known| *known == symbol)
            .unwrap();
        buffer = (buffer << 5) | u32::try_from(value).unwrap();
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            bytes.push(u8::try_from((buffer >> bits) & 0xff).unwrap());
        }
    }
    bytes
}

fn code_now(app: &TestApp, key: &[u8]) -> String {
    let step = app.clock().clock().now().timestamp().as_second() / 30;
    let tag = hmac::sign(
        &hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, key),
        &step.to_be_bytes(),
    );
    let digest = tag.as_ref();
    let offset = usize::from(digest[19] & 0x0f);
    let code = u32::from_be_bytes([
        digest[offset] & 0x7f,
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]) % 1_000_000;
    format!("{code:06}")
}

async fn page(app: &TestApp, session: &TestSession) -> Value {
    let answer = app
        .get("/account/two-factor")
        .session(session)
        .accept_json()
        .await;
    answer.assert_ok();
    answer.json()
}

async fn post(app: &TestApp, session: &TestSession, path: &str, code: &str) -> u16 {
    app.post(path)
        .session(session)
        .accept_json()
        .json(&json!({ "code": code }))
        .await
        .status
        .as_u16()
}

#[tokio::test]
async fn a_second_factor_is_asked_after_the_password_once_confirmed() {
    let app = started().await;
    let session = signed_up::<Member>(&app, "ada@example.com", "correct horse battery").await;
    assert_eq!(page(&app, &session).await["off"], true);

    app.post("/account/two-factor/start")
        .session(&session)
        .accept_json()
        .await
        .assert_ok();
    let pending = page(&app, &session).await["pending"].clone();
    let key = base32_decoded(pending["key"].as_str().unwrap());
    let recovery: Vec<String> = serde_json::from_value(pending["recovery"].clone()).unwrap();
    assert_eq!(recovery.len(), 10);
    let uri = pending["uri"].as_str().unwrap();
    assert!(
        uri.starts_with("otpauth://totp/sofie:ada%40example.com?secret="),
        "{uri}"
    );
    assert!(uri.ends_with("&issuer=sofie"), "{uri}");
    let shown = app.get("/account/two-factor").session(&session).await.body;
    assert!(shown.contains(&recovery[0]), "{shown}");
    assert!(
        shown.contains("aria-label=\"QR code of the key\""),
        "{shown}"
    );
    assert!(shown.contains("<path d=\"M16 16h28v4h-28z"), "{shown}");
    assert!(shown.contains(pending["key"].as_str().unwrap()), "{shown}");

    assert_eq!(
        post(&app, &session, "/account/two-factor/confirm", "000000").await,
        422
    );
    let code = code_now(&app, &key);
    assert_eq!(
        post(&app, &session, "/account/two-factor/confirm", &code).await,
        204
    );
    assert_eq!(page(&app, &session).await["enabled"], true);

    let signed_in = app.post("/sign-in").json(&credentials()).await;
    signed_in.assert_ok();
    assert!(
        signed_in.session().is_none(),
        "only after the second factor"
    );
    assert_eq!(signed_in.json()["second_factor"], true);
    let challenge = signed_in.json()["location"].as_str().unwrap().to_owned();
    assert!(challenge.starts_with("/account/two-factor/"), "{challenge}");
    app.get(&challenge).accept_json().await.assert_ok();

    let reused = app.post(&challenge).json(&json!({ "code": code })).await;
    assert_eq!(reused.status, 422, "a code works once: {}", reused.body);
    let proved = app
        .post(&challenge)
        .json(&json!({ "code": recovery[0].to_uppercase() }))
        .await;
    let again = app
        .post(&challenge)
        .json(&json!({ "code": recovery[1] }))
        .await;

    let session = proved.session().expect("a recovery code signs in");
    assert!(app.principal_of::<Member>(&session).is_some());
    assert_eq!(
        again.status, 422,
        "the pending sign-in is used up: {}",
        again.body
    );
}

#[tokio::test]
async fn wrong_codes_lock_the_second_factor_for_a_while() {
    let app = started().await;
    let session = signed_up::<Member>(&app, "ada@example.com", "correct horse battery").await;
    app.post("/account/two-factor/start")
        .session(&session)
        .accept_json()
        .await
        .assert_ok();
    let pending = page(&app, &session).await["pending"].clone();
    let key = base32_decoded(pending["key"].as_str().unwrap());

    for _ in 0..5 {
        assert_eq!(
            post(&app, &session, "/account/two-factor/confirm", "000000").await,
            422
        );
    }
    let locked = post(
        &app,
        &session,
        "/account/two-factor/confirm",
        &code_now(&app, &key),
    )
    .await;
    app.clock().advance(Duration::from_mins(16)).unwrap();
    let signed_in_again = app.post("/sign-in").json(&credentials()).await;
    let later = post(
        &app,
        &signed_in_again.session().unwrap(),
        "/account/two-factor/confirm",
        &code_now(&app, &key),
    )
    .await;

    assert_eq!(locked, 422);
    assert_eq!(later, 204);
}

#[tokio::test]
async fn the_page_asked_for_before_signing_in_is_kept_through_the_second_factor() {
    let app = started().await;
    let session = signed_up::<Member>(&app, "ada@example.com", "correct horse battery").await;
    app.post("/account/two-factor/start")
        .session(&session)
        .accept_json()
        .await
        .assert_ok();
    let key = base32_decoded(
        page(&app, &session).await["pending"]["key"]
            .as_str()
            .unwrap(),
    );
    let code = code_now(&app, &key);
    assert_eq!(
        post(&app, &session, "/account/two-factor/confirm", &code).await,
        204
    );
    app.clock().advance(Duration::from_secs(30)).unwrap();

    let signed_in = app
        .post("/sign-in?next=/account/two-factor%3Ftab%3D1")
        .json(&credentials())
        .await;
    let challenge = signed_in.json()["location"].as_str().unwrap().to_owned();
    let shown = app.get(&challenge).accept_json().await;
    let action = shown.json()["action"].as_str().unwrap().to_owned();
    let proved = app
        .post(&action)
        .form(&[("code", &code_now(&app, &key))])
        .await;

    assert_eq!(action, challenge, "the form posts where the challenge was");
    assert_eq!(proved.status, 303, "{}", proved.body);
    assert_eq!(proved.header("location"), "/account/two-factor?tab=1");
}

#[tokio::test]
async fn a_second_factor_keeps_no_next_that_leaves_the_site() {
    let app = started().await;
    let session = signed_up::<Member>(&app, "ada@example.com", "correct horse battery").await;
    app.post("/account/two-factor/start")
        .session(&session)
        .accept_json()
        .await
        .assert_ok();
    let key = base32_decoded(
        page(&app, &session).await["pending"]["key"]
            .as_str()
            .unwrap(),
    );
    let code = code_now(&app, &key);
    post(&app, &session, "/account/two-factor/confirm", &code).await;
    app.clock().advance(Duration::from_secs(30)).unwrap();

    let signed_in = app
        .post("/sign-in?next=//evil.example/x")
        .json(&credentials())
        .await;
    let challenge = signed_in.json()["location"].as_str().unwrap().to_owned();
    let proved = app
        .post(&challenge)
        .form(&[("code", &code_now(&app, &key))])
        .await;

    assert!(!challenge.contains("evil"), "{challenge}");
    assert_eq!(proved.header("location"), "/account");
}

#[tokio::test]
async fn two_setups_begun_at_once_never_fail_as_internal() {
    let app = started().await;
    let mut statuses = Vec::new();
    for round in 0..20 {
        app.clock().advance(Duration::from_secs(10)).unwrap();
        let email = format!("ada{round}@example.com");
        let session = signed_up::<Member>(&app, &email, "correct horse battery").await;
        let start = || {
            app.post("/account/two-factor/start")
                .session(&session)
                .accept_json()
        };
        let (first, second) = tokio::join!(start(), start());
        statuses.extend([first.status.as_u16(), second.status.as_u16()]);
    }

    assert!(!statuses.contains(&500), "{statuses:?}");
}

async fn enabled(app: &TestApp) -> (Vec<u8>, Vec<String>) {
    let session = signed_up::<Member>(app, "ada@example.com", "correct horse battery").await;
    app.post("/account/two-factor/start")
        .session(&session)
        .accept_json()
        .await
        .assert_ok();
    let pending = page(app, &session).await["pending"].clone();
    let key = base32_decoded(pending["key"].as_str().unwrap());
    let recovery = serde_json::from_value(pending["recovery"].clone()).unwrap();
    let code = code_now(app, &key);
    assert_eq!(
        post(app, &session, "/account/two-factor/confirm", &code).await,
        204
    );
    app.clock().advance(Duration::from_secs(30)).unwrap();
    (key, recovery)
}

async fn challenge(app: &TestApp) -> String {
    let signed_in = app.post("/sign-in").json(&credentials()).await;
    signed_in.json()["location"].as_str().unwrap().to_owned()
}

async fn sealed_key(app: &TestApp) -> Vec<Vec<sofie::core::SqlValue>> {
    app.query("SELECT secret FROM sofie_identity.second_factor", &[])
        .await
}

#[tokio::test]
async fn a_recovery_code_signs_in_after_the_key_sealing_the_factor_was_retired() {
    let app = started().await;
    let (_, recovery) = enabled(&app).await;
    app.query(
        "UPDATE sofie_identity.second_factor SET secret = 'sealed-by-a-retired-key'",
        &[],
    )
    .await;

    let proved = app
        .post(&challenge(&app).await)
        .json(&json!({ "code": recovery[0] }))
        .await;

    assert!(proved.session().is_some(), "{}", proved.body);
}

#[tokio::test]
async fn a_proved_code_seals_the_factor_again_with_the_current_key() {
    let app = started().await;
    let (key, _) = enabled(&app).await;
    let before = sealed_key(&app).await;

    let proved = app
        .post(&challenge(&app).await)
        .json(&json!({ "code": code_now(&app, &key) }))
        .await;

    assert!(proved.session().is_some(), "{}", proved.body);
    assert_ne!(sealed_key(&app).await, before);
}

async fn mailed_link(app: &TestApp, subject: &str) -> String {
    const BASE_URL: &str = "http://localhost";
    app.drain().await;
    let mails = sofie_testing::sent_mails(app);
    let mail = mails
        .iter()
        .find(|mail| mail.subject() == subject)
        .unwrap_or_else(|| panic!("no mail `{subject}` was sent"));
    let text = mail.body().text();
    let start = text.find(BASE_URL).unwrap() + BASE_URL.len();
    let end = start + text[start..].find(')').unwrap();
    text[start..end].to_owned()
}

#[cfg(feature = "password-reset")]
#[tokio::test]
async fn a_reset_confirming_the_address_first_removes_the_second_factor_set_up_before() {
    const NEW_PASSWORD: &str = "a whole new passphrase";
    let app = started().await;
    enabled(&app).await;
    app.post("/account/forgot-password")
        .json(&json!({ "email": "ada@example.com" }))
        .await
        .assert_ok();
    let link = mailed_link(&app, "Reset your password").await;

    app.post(&link)
        .json(&json!({ "new_password": NEW_PASSWORD }))
        .await
        .assert_ok();
    let signed_in = app
        .post("/sign-in")
        .json(&json!({ "email": "ada@example.com", "password": NEW_PASSWORD }))
        .await;

    assert!(signed_in.session().is_some(), "{}", signed_in.body);
}

#[tokio::test]
async fn setting_up_a_second_factor_needs_a_sign_in_from_the_last_ten_minutes() {
    let app = started().await;
    let stale = signed_up::<Member>(&app, "ada@example.com", "correct horse battery").await;
    app.clock().advance(Duration::from_mins(11)).unwrap();
    let start = |session: TestSession| {
        app.post("/account/two-factor/start")
            .session(&session)
            .accept_json()
    };
    let refused = start(stale).await;
    let fresh = app
        .post("/sign-in")
        .json(&credentials())
        .await
        .session()
        .unwrap();
    start(fresh.clone()).await.assert_ok();
    let key = base32_decoded(page(&app, &fresh).await["pending"]["key"].as_str().unwrap());
    app.clock().advance(Duration::from_mins(11)).unwrap();

    let confirmed = post(
        &app,
        &fresh,
        "/account/two-factor/confirm",
        &code_now(&app, &key),
    )
    .await;

    assert_eq!(refused.status, 422, "{}", refused.body);
    assert!(refused.body.contains("Sign in again"), "{}", refused.body);
    assert_eq!(confirmed, 422);
}

#[tokio::test]
async fn a_pending_setup_s_key_and_codes_show_only_after_a_recent_sign_in() {
    let app = started().await;
    let session = signed_up::<Member>(&app, "ada@example.com", "correct horse battery").await;
    app.post("/account/two-factor/start")
        .session(&session)
        .accept_json()
        .await
        .assert_ok();
    app.clock().advance(Duration::from_mins(11)).unwrap();

    let shown = page(&app, &session).await;

    assert_eq!(shown["pending"], Value::Null, "{shown}");
}

#[cfg(feature = "sign-in-links")]
#[tokio::test]
async fn a_sign_in_link_confirming_the_address_first_signs_in_without_the_factor_it_removes() {
    let app = started().await;
    enabled(&app).await;
    app.post("/account/sign-in-link")
        .json(&json!({ "email": "ada@example.com" }))
        .await
        .assert_ok();
    let link = mailed_link(&app, "Sign in").await;

    let followed = app.post(&link).json(&json!({})).await;

    assert!(followed.session().is_some(), "{}", followed.body);
}
