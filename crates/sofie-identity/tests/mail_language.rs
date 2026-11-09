#![cfg(all(feature = "i18n", feature = "password-reset"))]

use std::sync::Arc;

use serde_json::json;
use sofie::db::Db;
use sofie::prelude::*;
use sofie::worker::Worker;
use sofie_identity::{Account, MemberArea, Memberships};
use sofie_testing::{TestApp, sent_mails};

struct Members;

impl Area for Members {
    const NAME: &'static str = "members";
    const PREFIX: &'static str = "/members";
    const SIGN_IN: &'static str = "/members/sign-in";
    type Principal = Account;
}

impl MemberArea for Members {
    const HOME: &'static str = "/members/home";
    const SIGN_UP: Option<&'static str> = Some("/members/sign-up");
}

const GERMAN_SUBJECT: &str = "Passwort zurücksetzen";

async fn started() -> TestApp {
    sofie_testing::start(
        |db: Db| async move {
            db.schema().migrate(&[sofie_identity::migrations()]).await?;
            let builtin = [("sofie_identity", sofie_identity::TRANSLATIONS)];
            let translations = sofie::http::translations(&["en", "de"], &[], &builtin, &[])?;
            sofie::mail::translations(&db, Arc::new(translations));
            let mut screens = sofie::http::Screens::default();
            screens.add_builtin(sofie_identity::module(&[]).screens)?;
            screens.describe_forms(sofie_identity::FORMS)?;
            Ok(App::new(db)
                .screens(screens)
                .membership(Memberships)
                .area::<Web>("sofie_identity", sofie_identity::routes::<Members>)
                .area::<Members>(
                    "sofie_identity",
                    sofie_identity::sessions::language_routes::<Members>,
                ))
        },
        |db| sofie_identity::worker(Worker::new(db)),
    )
    .await
}

async fn signed_up(app: &TestApp, email: &str) {
    let credentials = json!({ "email": email, "password": "correct horse battery" });
    app.post("/members/sign-up")
        .json(&credentials)
        .await
        .assert_ok();
}

/// The subjects of the mails sent since `before`, so the sign-up's own mail is left out.
async fn reset_subjects(app: &TestApp, email: &str, accept_language: &str) -> Vec<String> {
    app.drain().await;
    let before = sent_mails(app).len();
    app.post("/members/forgot-password")
        .header("accept-language", accept_language)
        .json(&json!({ "email": email }))
        .await
        .assert_ok();
    app.drain().await;
    sent_mails(app)[before..]
        .iter()
        .map(|mail| mail.subject().to_owned())
        .collect()
}

#[tokio::test]
async fn a_link_is_mailed_in_the_language_the_asker_s_browser_names_first() {
    let app = started().await;
    signed_up(&app, "ada@example.com").await;

    let subjects = reset_subjects(&app, "ada@example.com", "de-DE, en;q=0.5").await;

    assert_eq!(subjects, [GERMAN_SUBJECT]);
    let mails = sent_mails(&app);
    let mail = mails.last().unwrap();
    assert!(
        mail.body().text().contains("Neues Passwort wählen"),
        "{}",
        mail.body().text()
    );
}

#[tokio::test]
async fn the_account_s_chosen_language_wins_over_the_asker_s() {
    let app = started().await;
    signed_up(&app, "ada@example.com").await;
    let credentials = json!({ "email": "ada@example.com", "password": "correct horse battery" });
    let session = app
        .post("/members/sign-in")
        .json(&credentials)
        .await
        .session()
        .unwrap();
    app.post("/members/language")
        .session(&session)
        .accept_json()
        .json(&json!({ "locale": "de" }))
        .await
        .assert_ok();

    let subjects = reset_subjects(&app, "ada@example.com", "en-US").await;

    assert_eq!(subjects, [GERMAN_SUBJECT]);
}
