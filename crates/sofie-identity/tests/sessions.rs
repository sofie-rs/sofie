#![cfg(feature = "sessions")]

use std::time::Duration;

use serde_json::{Value, json};
use sofie::db::Db;
use sofie::prelude::*;
use sofie::worker::Worker;
use sofie_identity::Memberships;
use sofie_testing::{TestApp, TestSession, sent_mails};
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
                .area::<Member>("sofie_identity", sofie_identity::sessions::routes::<Member>))
        },
        |db| sofie_identity::worker(Worker::new(db)),
    )
    .await
}

const CREDENTIALS: fn() -> Value =
    || json!({ "email": "ada@example.com", "password": "correct horse battery" });

async fn signed_in(app: &TestApp) -> TestSession {
    let answer = app
        .post("/sign-in")
        .header("user-agent", "Phone\u{202e}moc.elpmaxe")
        .json(&CREDENTIALS())
        .await;
    answer.assert_ok();
    answer.session().expect("signing in sets the cookie")
}

async fn listed(app: &TestApp, session: &TestSession) -> Vec<Value> {
    let answer = app
        .get("/account/sessions")
        .session(session)
        .accept_json()
        .await;
    answer.assert_ok();
    answer.json().as_array().cloned().unwrap_or_default()
}

#[tokio::test]
async fn a_member_lists_its_sessions_and_signs_out_everywhere_else() {
    let app = started().await;
    app.post("/sign-up").json(&CREDENTIALS()).await.assert_ok();
    let phone = signed_in(&app).await;
    let laptop = signed_in(&app).await;

    let sessions = listed(&app, &laptop).await;
    assert_eq!(
        sessions.len(),
        2,
        "two sign-ins, signing up signs in none: {sessions:?}"
    );
    let devices = sessions
        .iter()
        .filter(|session| session["device"] == "Phonemoc.elpmaxe")
        .count();
    assert_eq!(
        devices, 2,
        "the device as sent, less what misleads: {sessions:?}"
    );

    app.post("/account/sessions/others")
        .session(&laptop)
        .accept_json()
        .await
        .assert_ok();

    let refused = app
        .get("/account/sessions")
        .session(&phone)
        .accept_json()
        .await;
    refused.assert_status(401);
    assert!(
        refused.header("set-cookie").contains("Max-Age=0"),
        "{refused:?}"
    );
    assert_eq!(listed(&app, &laptop).await.len(), 1);
}

async fn refused(app: &TestApp, session: &TestSession) {
    app.get("/account/sessions")
        .session(session)
        .accept_json()
        .await
        .assert_status(401);
}

#[tokio::test]
async fn signing_out_ends_the_session_for_a_kept_copy_of_its_cookie() {
    let app = started().await;
    app.post("/sign-up").json(&CREDENTIALS()).await.assert_ok();
    let session = signed_in(&app).await;
    listed(&app, &session).await;

    app.post("/account/sign-out")
        .session(&session)
        .await
        .assert_ok();

    refused(&app, &session).await;
}

/// The race of a request sent before signing out and answered after it, which reseals.
#[tokio::test]
async fn a_cookie_resealed_before_signing_out_is_refused_after_it() {
    let app = started().await;
    app.post("/sign-up").json(&CREDENTIALS()).await.assert_ok();
    let session = signed_in(&app).await;
    app.clock().advance(Duration::from_mins(5)).unwrap();
    let answer = app
        .get("/account/sessions")
        .session(&session)
        .accept_json()
        .await;
    answer.assert_ok();
    let resealed = answer.session().expect("an active session is resealed");

    app.post("/account/sign-out")
        .session(&session)
        .await
        .assert_ok();

    refused(&app, &resealed).await;
}

const BASE_URL: &str = "http://localhost";

async fn reset_link(app: &TestApp) -> String {
    app.drain().await;
    let mails = sent_mails(app);
    let mail = mails
        .iter()
        .rfind(|mail| mail.subject() == "Reset your password")
        .expect("a reset link was mailed");
    let text = mail.body().text();
    let start = text.find(BASE_URL).unwrap() + BASE_URL.len();
    let end = start + text[start..].find(')').unwrap();
    text[start..end].to_owned()
}

#[tokio::test]
async fn a_password_reset_ends_the_session_on_every_device() {
    let app = started().await;
    app.post("/sign-up").json(&CREDENTIALS()).await.assert_ok();
    let phone = signed_in(&app).await;
    listed(&app, &phone).await;

    app.post("/account/forgot-password")
        .json(&json!({ "email": "ada@example.com" }))
        .await
        .assert_ok();
    let link = reset_link(&app).await;
    app.post(&link)
        .json(&json!({ "new_password": "a whole new passphrase" }))
        .await
        .assert_ok();

    refused(&app, &phone).await;
}

#[tokio::test]
async fn removing_a_membership_ends_the_account_s_sessions_in_its_area() {
    let app = started().await;
    app.post("/sign-up").json(&CREDENTIALS()).await.assert_ok();
    let session = signed_in(&app).await;
    listed(&app, &session).await;
    let account = app.principal_of::<Member>(&session).unwrap().id();

    let mut removed = sofie_identity::remove_membership(
        &app.db().read(),
        account,
        Member::NAME,
        app.db().clock().now(),
    )
    .await
    .unwrap();
    app.db().commit(&mut removed).await.unwrap();

    refused(&app, &session).await;
    let again = app.post("/sign-in").json(&CREDENTIALS()).await;
    assert_eq!(again.status, 422, "no longer a member: {}", again.body);
}

async fn recorded_sessions(app: &TestApp) -> Vec<Vec<sofie::core::SqlValue>> {
    app.query("SELECT session FROM sofie_identity.account_session", &[])
        .await
}

#[tokio::test]
async fn sessions_no_cookie_can_outlive_are_pruned_ended_or_not() {
    let app = started().await;
    app.post("/sign-up").json(&CREDENTIALS()).await.assert_ok();
    let ended = signed_in(&app).await;
    app.post("/account/sign-out")
        .session(&ended)
        .await
        .assert_ok();
    let abandoned = signed_in(&app).await;
    listed(&app, &abandoned).await;

    app.clock()
        .advance(sofie::auth::SessionClaims::ABSOLUTE_LIFETIME + Duration::from_hours(2))
        .unwrap();
    let current = signed_in(&app).await;
    app.drain().await;

    assert_eq!(recorded_sessions(&app).await.len(), 1);
    assert_eq!(listed(&app, &current).await.len(), 1);
}
