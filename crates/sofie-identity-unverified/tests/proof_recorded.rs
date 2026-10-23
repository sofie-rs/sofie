use serde_json::json;
use sofie::core::SqlValue;
use sofie::db::Db;
use sofie::http::Screens;
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

const EMAIL: &str = "ada@example.com";
const BASE_URL: &str = "http://localhost";

async fn identity_app(db: Db) -> Result<App> {
    db.schema().migrate(&[sofie_identity::migrations()]).await?;
    let mut screens = Screens::default();
    screens.add_builtin(sofie_identity::module(&[]).screens)?;
    screens.describe_forms(sofie_identity::FORMS)?;
    Ok(App::new(db)
        .screens(screens)
        .membership(Memberships)
        .area::<Web>("sofie_identity", sofie_identity::routes::<Members>))
}

async fn signed_up() -> TestApp {
    let app =
        sofie_testing::start(identity_app, |db| sofie_identity::worker(Worker::new(db))).await;
    app.post("/members/sign-up")
        .json(&json!({ "email": EMAIL, "password": "correct horse battery" }))
        .await
        .assert_ok();
    assert_eq!(verified(&app).await, [vec![SqlValue::Bool(false)]]);
    app
}

async fn verified(app: &TestApp) -> Vec<Vec<SqlValue>> {
    app.query(
        "SELECT verified_at IS NOT NULL FROM sofie_identity.account",
        &[],
    )
    .await
}

async fn mailed_link(app: &TestApp, subject: &str) -> String {
    app.drain().await;
    let mails = sent_mails(app);
    let mail = mails
        .iter()
        .find(|mail| mail.subject() == subject)
        .unwrap_or_else(|| panic!("no mail `{subject}` was sent"));
    let text = mail.body().text();
    let start = text.find(BASE_URL).unwrap() + BASE_URL.len();
    let end = start + text[start..].find(')').unwrap();
    text[start..end].to_owned()
}

#[tokio::test]
async fn a_password_reset_records_the_address_it_proved() {
    let app = signed_up().await;
    app.post("/members/forgot-password")
        .json(&json!({ "email": EMAIL }))
        .await
        .assert_ok();
    let link = mailed_link(&app, "Reset your password").await;

    app.post(&link)
        .json(&json!({ "new_password": "a whole new passphrase" }))
        .await
        .assert_ok();

    assert_eq!(verified(&app).await, [vec![SqlValue::Bool(true)]]);
}

#[tokio::test]
async fn a_sign_in_link_records_the_address_it_proved() {
    let app = signed_up().await;
    app.post("/members/sign-in-link")
        .json(&json!({ "email": EMAIL }))
        .await
        .assert_ok();
    let link = mailed_link(&app, "Sign in").await;

    let followed = app.post(&link).json(&json!({})).await;

    assert!(followed.session().is_some(), "{}", followed.body);
    assert_eq!(verified(&app).await, [vec![SqlValue::Bool(true)]]);
}
