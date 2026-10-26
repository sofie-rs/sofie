#![cfg(all(feature = "password-reset", feature = "email-verification"))]

use std::time::Duration;

use serde_json::{Value, json};
use sofie::db::Db;
use sofie::http::Screens;
use sofie::prelude::*;
use sofie::worker::Worker;
use sofie_identity::{Account, MemberArea, Memberships};
use sofie_testing::{TestApp, TestResponse, sent_mails};

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

struct Confirmed;

impl Area for Confirmed {
    const NAME: &'static str = "confirmed";
    const PREFIX: &'static str = "/confirmed";
    const SIGN_IN: &'static str = "/confirmed/sign-in";
    type Principal = Account;
}

impl MemberArea for Confirmed {
    const HOME: &'static str = "/confirmed/home";
    const SIGN_UP: Option<&'static str> = Some("/confirmed/sign-up");
    const VERIFIED_EMAIL: bool = true;
}

struct Closed;

impl Area for Closed {
    const NAME: &'static str = "closed";
    const PREFIX: &'static str = "/closed";
    const SIGN_IN: &'static str = "/closed/sign-in";
    type Principal = Account;
}

impl MemberArea for Closed {
    const HOME: &'static str = "/closed/home";
}

const PASSWORD: &str = "correct horse battery";
const NEW_PASSWORD: &str = "a whole new passphrase";
const RESET_SUBJECT: &str = "Reset your password";
const VERIFY_SUBJECT: &str = "Confirm your email address";
const TAKEN_SUBJECT: &str = "You already have an account";
const JOIN_SUBJECT: &str = "Sign in to join";
const BASE_URL: &str = "http://localhost";

async fn identity_app(db: Db) -> Result<App> {
    db.schema().migrate(&[sofie_identity::migrations()]).await?;
    let mut screens = Screens::default();
    screens.add_builtin(sofie_identity::module(&[]).screens)?;
    screens.describe_forms(sofie_identity::FORMS)?;
    Ok(App::new(db)
        .screens(screens)
        .membership(Memberships)
        .area::<Web>("sofie_identity", sofie_identity::routes::<Members>)
        .area::<Web>("sofie_identity", sofie_identity::routes::<Confirmed>)
        .area::<Web>("sofie_identity", sofie_identity::routes::<Closed>))
}

async fn started() -> TestApp {
    sofie_testing::start(identity_app, |db| sofie_identity::worker(Worker::new(db))).await
}

fn credentials(email: &str, password: &str) -> Value {
    json!({ "email": email, "password": password })
}

async fn sign_up(app: &TestApp, area: &str) -> TestResponse {
    app.post(&format!("/{area}/sign-up"))
        .json(&credentials("ada@example.com", PASSWORD))
        .await
}

async fn sign_in(app: &TestApp, area: &str, password: &str) -> TestResponse {
    let path = format!("/{area}/sign-in");
    app.post(&path)
        .json(&credentials("ada@example.com", password))
        .await
}

async fn mailed_links(app: &TestApp, subject: &str) -> Vec<String> {
    app.drain().await;
    sent_mails(app)
        .iter()
        .filter(|mail| mail.subject() == subject)
        .map(|mail| {
            let text = mail.body().text();
            let start = text.find(BASE_URL).unwrap() + BASE_URL.len();
            let end = start + text[start..].find(')').unwrap();
            text[start..end].to_owned()
        })
        .collect()
}

async fn ask_reset(app: &TestApp, email: &str) -> TestResponse {
    app.post("/members/forgot-password")
        .json(&json!({ "email": email }))
        .await
}

async fn reset(app: &TestApp, link: &str, password: &str) -> TestResponse {
    app.post(link)
        .json(&json!({ "new_password": password }))
        .await
}

#[tokio::test]
async fn a_reset_link_sets_a_new_password_once_and_retires_older_links() {
    let app = started().await;
    let signed_up = sign_up(&app, "members").await;
    assert!(signed_up.session().is_none(), "{}", signed_up.body);
    assert_eq!(ask_reset(&app, "ada@example.com").await.status, 204);
    assert_eq!(ask_reset(&app, "ada@example.com").await.status, 204);
    let links = mailed_links(&app, RESET_SUBJECT).await;
    assert_eq!(links.len(), 2);

    let page = app.get(&links[1]).accept_json().await;
    assert_eq!(page.status, 200, "{}", page.body);
    assert_eq!(page.json()["action"], links[1].as_str());
    assert_eq!(reset(&app, &links[1], "short").await.status, 422);
    let changed = reset(&app, &links[1], NEW_PASSWORD).await;
    assert!(changed.status.is_success(), "{}", changed.body);

    assert!(
        sign_in(&app, "members", NEW_PASSWORD)
            .await
            .session()
            .is_some()
    );
    assert_eq!(sign_in(&app, "members", PASSWORD).await.status, 422);
    for used in &links {
        let again = reset(&app, used, "yet another passphrase").await;
        assert_eq!(again.status, 422, "{}", again.body);
        assert!(again.body.contains("invalid"), "{}", again.body);
    }
}

fn mailed_to(app: &TestApp, subject: &str) -> Vec<String> {
    sent_mails(app)
        .iter()
        .filter(|mail| mail.subject() == subject)
        .flat_map(|mail| mail.to().iter().map(|to| to.email().as_str().to_owned()))
        .collect()
}

#[tokio::test]
async fn signing_up_answers_alike_for_a_new_and_a_taken_address_and_mails_each_owner() {
    let app = started().await;
    let first = sign_up(&app, "members").await;
    first.assert_ok();

    let taken = app
        .post("/members/sign-up")
        .json(&credentials("Ada@Example.com", NEW_PASSWORD))
        .await;
    let new = app
        .post("/members/sign-up")
        .json(&credentials("grace@example.com", NEW_PASSWORD))
        .await;
    let posted_taken = app
        .post("/members/sign-up")
        .form(&[("email", "ada@example.com"), ("password", NEW_PASSWORD)])
        .htmx("sign-up")
        .trigger("sign-up")
        .await;
    let posted_new = app
        .post("/members/sign-up")
        .form(&[("email", "linus@example.com"), ("password", NEW_PASSWORD)])
        .htmx("sign-up")
        .trigger("sign-up")
        .await;

    assert_eq!((taken.status, &taken.body), (new.status, &new.body));
    assert_eq!(taken.status, first.status);
    for answer in [&first, &taken, &new, &posted_taken, &posted_new] {
        assert!(answer.session().is_none(), "signing up signs nobody in");
    }
    assert_eq!(posted_taken.header("hx-redirect"), "/members/link-sent");
    assert_eq!(posted_new.header("hx-redirect"), "/members/link-sent");

    app.drain().await;
    let mut confirming = mailed_to(&app, VERIFY_SUBJECT);
    confirming.sort();
    assert_eq!(
        confirming,
        ["ada@example.com", "grace@example.com", "linus@example.com"]
    );
    assert_eq!(
        mailed_to(&app, TAKEN_SUBJECT),
        ["ada@example.com", "ada@example.com"]
    );
    assert_eq!(
        sign_in(&app, "members", NEW_PASSWORD).await.status,
        422,
        "a taken address keeps its password"
    );
    assert!(sign_in(&app, "members", PASSWORD).await.session().is_some());

    let links = mailed_links(&app, TAKEN_SUBJECT).await;
    assert_eq!(links.len(), 2);
    assert!(
        reset(&app, &links[0], NEW_PASSWORD)
            .await
            .status
            .is_success()
    );
    assert!(
        sign_in(&app, "members", NEW_PASSWORD)
            .await
            .session()
            .is_some()
    );
}

#[tokio::test]
async fn a_member_elsewhere_signing_up_is_answered_alike_and_mailed_to_sign_in_and_join() {
    let app = started().await;
    sign_up(&app, "members").await.assert_ok();
    app.drain().await;

    let taken = app
        .post("/confirmed/sign-up")
        .json(&credentials("ada@example.com", NEW_PASSWORD))
        .await;
    let new = app
        .post("/confirmed/sign-up")
        .json(&credentials("grace@example.com", NEW_PASSWORD))
        .await;

    assert_eq!((taken.status, &taken.body), (new.status, &new.body));
    assert!(taken.session().is_none());
    app.drain().await;
    assert_eq!(mailed_to(&app, JOIN_SUBJECT), ["ada@example.com"]);
    assert_eq!(mailed_to(&app, TAKEN_SUBJECT), Vec::<String>::new());
    assert_eq!(
        mailed_to(&app, VERIFY_SUBJECT),
        ["ada@example.com", "grace@example.com"],
        "a stranger's address signs up as usual"
    );
    assert_eq!(sign_in(&app, "confirmed", PASSWORD).await.status, 422);
    assert_eq!(
        sign_in(&app, "confirmed", NEW_PASSWORD).await.status,
        422,
        "the taken address keeps its password and gains nothing yet"
    );
}

#[tokio::test]
async fn signing_in_through_the_join_link_adds_the_membership_once() {
    let app = started().await;
    sign_up(&app, "members").await.assert_ok();
    app.post("/members/sign-up")
        .json(&credentials("grace@example.com", NEW_PASSWORD))
        .await
        .assert_ok();
    sign_up(&app, "confirmed").await.assert_ok();
    let link = mailed_links(&app, JOIN_SUBJECT).await.remove(0);
    let page = app.get(&link).accept_json().await;
    assert_eq!(page.json()["action"], link.as_str(), "{}", page.body);

    let wrong = app
        .post(&link)
        .json(&credentials("ada@example.com", NEW_PASSWORD))
        .await;
    let other_account = app
        .post(&link)
        .json(&credentials("grace@example.com", NEW_PASSWORD))
        .await;
    assert_eq!(
        (wrong.status, &wrong.body),
        (other_account.status, &other_account.body)
    );
    assert_eq!(wrong.status, 422);
    assert_eq!(sign_in(&app, "confirmed", PASSWORD).await.status, 422);

    let joined = app
        .post(&link)
        .json(&credentials("ada@example.com", PASSWORD))
        .await;
    let session = joined.session().expect("joining signs in");
    assert!(app.principal_of::<Confirmed>(&session).is_some());
    let again = app
        .post(&link)
        .json(&credentials("ada@example.com", PASSWORD))
        .await;
    assert_eq!(again.status, 422, "{}", again.body);
    assert!(
        sign_in(&app, "confirmed", PASSWORD)
            .await
            .session()
            .is_some(),
        "the link confirmed the address the area asks for"
    );
    assert!(sign_in(&app, "members", PASSWORD).await.session().is_some());
}

#[tokio::test]
async fn an_area_without_a_sign_up_mails_no_join_link_and_adds_no_one() {
    let app = started().await;
    sign_up(&app, "members").await.assert_ok();

    let refused = app
        .post("/closed/sign-up")
        .json(&credentials("ada@example.com", PASSWORD))
        .await;
    app.drain().await;

    assert!(!refused.status.is_success(), "{}", refused.body);
    assert_eq!(mailed_to(&app, JOIN_SUBJECT), Vec::<String>::new());
    assert_eq!(sign_in(&app, "closed", PASSWORD).await.status, 422);
    sign_up(&app, "confirmed").await.assert_ok();
    let elsewhere = mailed_links(&app, JOIN_SUBJECT).await.remove(0);
    let moved = app
        .post(&elsewhere.replace("/confirmed/", "/closed/"))
        .json(&credentials("ada@example.com", PASSWORD))
        .await;
    assert!(!moved.status.is_success(), "{}", moved.body);
    assert_eq!(sign_in(&app, "closed", PASSWORD).await.status, 422);
}

#[tokio::test]
async fn asking_answers_the_same_for_any_address_and_mails_only_members() {
    let app = started().await;
    sign_up(&app, "members").await;

    let known = ask_reset(&app, "ada@example.com").await;
    let unknown = ask_reset(&app, "nobody@example.com").await;
    let elsewhere = app
        .post("/confirmed/forgot-password")
        .json(&json!({ "email": "ada@example.com" }))
        .await;

    assert_eq!((known.status, &known.body), (unknown.status, &unknown.body));
    assert_eq!(elsewhere.status, 204);
    assert_eq!(mailed_links(&app, RESET_SUBJECT).await.len(), 1);
}

#[tokio::test]
async fn an_account_gets_at_most_three_links_an_hour() {
    let app = started().await;
    sign_up(&app, "members").await;

    for _ in 0..4 {
        assert_eq!(ask_reset(&app, "ada@example.com").await.status, 204);
        app.drain().await;
    }

    assert_eq!(mailed_links(&app, RESET_SUBJECT).await.len(), 3);
}

#[tokio::test]
async fn a_changed_or_expired_link_opens_nothing() {
    let app = started().await;
    sign_up(&app, "members").await;
    ask_reset(&app, "ada@example.com").await;
    let link = mailed_links(&app, RESET_SUBJECT).await.remove(0);

    let changed = link.replace("expires=", "expires=9");
    assert_eq!(app.get(&changed).accept_json().await.status, 403);
    app.clock().advance(Duration::from_mins(61)).unwrap();

    assert_eq!(app.get(&link).accept_json().await.status, 403);
    assert_eq!(reset(&app, &link, NEW_PASSWORD).await.status, 403);
}

#[tokio::test]
async fn links_expired_over_an_hour_ago_are_pruned() {
    let app = started().await;
    sign_up(&app, "members").await;
    ask_reset(&app, "ada@example.com").await;
    app.drain().await;

    app.clock().advance(Duration::from_mins(121)).unwrap();
    app.drain().await;

    let links = app
        .query("SELECT count(*) FROM sofie_identity.account_link", &[])
        .await;
    assert_eq!(links, [vec![sofie::core::SqlValue::Int(0)]]);
}

#[tokio::test]
async fn an_area_requiring_a_confirmed_address_signs_in_only_after_its_link() {
    let app = started().await;

    let signed_up = sign_up(&app, "confirmed").await;
    assert!(signed_up.status.is_success(), "{}", signed_up.body);
    assert!(signed_up.session().is_none());
    let refused = sign_in(&app, "confirmed", PASSWORD).await;
    let wrong = sign_in(&app, "confirmed", "not the password").await;
    assert_eq!(refused.status, 422);
    assert_eq!(
        (refused.status, &refused.body),
        (wrong.status, &wrong.body),
        "the password one signed up with tells nothing before the address is confirmed"
    );
    assert!(
        refused.body.contains("not confirmed yet"),
        "{}",
        refused.body
    );

    let links = mailed_links(&app, VERIFY_SUBJECT).await;
    assert_eq!(links.len(), 1);
    assert_eq!(app.get(&links[0]).accept_json().await.status, 200);
    let confirmed = app.post(&links[0]).form(&[] as &[(&str, &str)]).await;
    assert_eq!(confirmed.status, 303, "{}", confirmed.body);
    assert_eq!(
        confirmed.header("location"),
        "/confirmed/sign-in",
        "confirming signs nobody in, so it leads to signing in"
    );

    assert!(
        sign_in(&app, "confirmed", PASSWORD)
            .await
            .session()
            .is_some()
    );
    let asked = app
        .post("/confirmed/verify-email")
        .json(&json!({ "email": "ada@example.com" }))
        .await;
    assert_eq!(asked.status, 204);
    assert_eq!(
        mailed_links(&app, VERIFY_SUBJECT).await.len(),
        1,
        "a confirmed address gets no new link"
    );
}

#[cfg(feature = "passkeys")]
#[tokio::test]
async fn confirming_an_address_removes_the_passkeys_added_before_but_keeps_the_password() {
    let app = started().await;
    sign_up(&app, "confirmed").await.assert_ok();
    app.query(
        "INSERT INTO sofie_identity.passkey (id, version, account, credential, public_key, \
         algorithm, sign_count, challenged_at, created_at, updated_at) \
         SELECT $1, 1, id, 'credential', 'key', -7, 0, 0, now(), now() FROM sofie_identity.account",
        &[sofie::core::SqlValue::Uuid(sofie::db::Uuid::from_u128(1))],
    )
    .await;
    let link = mailed_links(&app, VERIFY_SUBJECT).await.remove(0);

    let confirmed = app.post(&link).form(&[] as &[(&str, &str)]).await;

    assert_eq!(confirmed.status, 303, "{}", confirmed.body);
    let live = app
        .query(
            "SELECT count(*) FROM sofie_identity.passkey WHERE deleted_at IS NULL",
            &[],
        )
        .await;
    assert_eq!(live, [vec![sofie::core::SqlValue::Int(0)]]);
    assert!(
        sign_in(&app, "confirmed", PASSWORD)
            .await
            .session()
            .is_some()
    );
}

#[cfg(feature = "shield")]
#[tokio::test]
async fn the_right_password_before_confirming_does_not_lock_the_account() {
    use sofie::http::{Shield, ShieldMode};
    let app = sofie_testing::start(
        |db: Db| async move {
            Ok(identity_app(db)
                .await?
                .serving(|serving| serving.shield(Shield::new(ShieldMode::Enforce))))
        },
        |db| sofie_identity::worker(Worker::new(db)),
    )
    .await;
    sign_up(&app, "confirmed").await;

    for _ in 0..10 {
        app.clock().advance(Duration::from_secs(6)).unwrap();
        assert_eq!(sign_in(&app, "confirmed", PASSWORD).await.status, 422);
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    let links = mailed_links(&app, VERIFY_SUBJECT).await;
    app.post(&links[0]).json(&json!({})).await;
    app.clock().advance(Duration::from_secs(6)).unwrap();

    let signed_in = sign_in(&app, "confirmed", PASSWORD).await;
    assert!(signed_in.session().is_some(), "{}", signed_in.body);
}

#[cfg(feature = "sign-in-links")]
#[tokio::test]
async fn a_sign_in_link_signs_in_once_and_confirms_the_address() {
    const SIGN_IN_SUBJECT: &str = "Sign in";
    let app = started().await;
    sign_up(&app, "confirmed").await.assert_ok();
    let ask = |email: &'static str| {
        app.post("/confirmed/sign-in-link")
            .json(&json!({ "email": email }))
    };
    assert_eq!(ask("ada@example.com").await.status, 204);
    assert_eq!(ask("nobody@example.com").await.status, 204);
    let links = mailed_links(&app, SIGN_IN_SUBJECT).await;
    assert_eq!(links.len(), 1, "only an address with an account gets one");
    let page = app.get(&links[0]).accept_json().await;
    assert_eq!(page.json()["action"], links[0].as_str(), "{}", page.body);

    let followed = app.post(&links[0]).json(&json!({})).await;
    let again = app.post(&links[0]).json(&json!({})).await;

    let session = followed.session().expect("following the link signs in");
    assert!(app.principal_of::<Confirmed>(&session).is_some());
    assert_eq!(again.status, 422, "{}", again.body);
}

async fn live_sessions(app: &TestApp) -> Vec<Vec<sofie::core::SqlValue>> {
    app.query(
        "SELECT count(*) FROM sofie_identity.account_session WHERE ended_at IS NULL",
        &[],
    )
    .await
}

#[cfg(feature = "sign-in-links")]
#[tokio::test]
async fn a_sign_in_link_confirming_an_address_ends_the_password_and_sessions_before_it() {
    let app = started().await;
    sign_up(&app, "members").await.assert_ok();
    sign_in(&app, "members", PASSWORD).await.assert_ok();
    app.post("/members/sign-in-link")
        .json(&json!({ "email": "ada@example.com" }))
        .await
        .assert_ok();
    let link = mailed_links(&app, "Sign in").await.remove(0);

    let followed = app.post(&link).json(&json!({})).await;

    assert!(followed.session().is_some(), "{}", followed.body);
    assert_eq!(
        live_sessions(&app).await,
        [vec![sofie::core::SqlValue::Int(1)]]
    );
    assert_eq!(sign_in(&app, "members", PASSWORD).await.status, 422);
}

#[tokio::test]
async fn adding_an_unconfirmed_account_to_an_area_ends_its_password_and_sessions() {
    let app = started().await;
    sign_up(&app, "members").await.assert_ok();
    sign_in(&app, "members", PASSWORD).await.assert_ok();
    let read = app.db().read();
    let email = sofie::values::Email::parse("ada@example.com").unwrap();

    let mut added = sofie_identity::add_member(&read, &app.db().clock(), email, "confirmed")
        .await
        .unwrap();
    app.db().commit(&mut added).await.unwrap();

    assert_eq!(
        live_sessions(&app).await,
        [vec![sofie::core::SqlValue::Int(0)]]
    );
    assert_eq!(sign_in(&app, "members", PASSWORD).await.status, 422);
}

#[tokio::test]
async fn adding_a_confirmed_account_to_an_area_keeps_its_password() {
    let app = started().await;
    sign_up(&app, "members").await.assert_ok();
    let link = mailed_links(&app, VERIFY_SUBJECT).await.remove(0);
    app.post(&link).json(&json!({})).await;
    let read = app.db().read();
    let email = sofie::values::Email::parse("ada@example.com").unwrap();

    let mut added = sofie_identity::add_member(&read, &app.db().clock(), email, "confirmed")
        .await
        .unwrap();
    app.db().commit(&mut added).await.unwrap();

    sign_in(&app, "confirmed", PASSWORD).await.assert_ok();
}

#[tokio::test]
async fn a_reset_confirms_the_address_so_adding_the_account_keeps_its_new_password() {
    let app = started().await;
    sign_up(&app, "members").await.assert_ok();
    ask_reset(&app, "ada@example.com").await.assert_ok();
    let link = mailed_links(&app, RESET_SUBJECT).await.remove(0);
    reset(&app, &link, NEW_PASSWORD).await.assert_ok();
    let read = app.db().read();
    let email = sofie::values::Email::parse("ada@example.com").unwrap();

    let mut added = sofie_identity::add_member(&read, &app.db().clock(), email, "confirmed")
        .await
        .unwrap();
    app.db().commit(&mut added).await.unwrap();

    sign_in(&app, "confirmed", NEW_PASSWORD).await.assert_ok();
}

#[cfg(feature = "sign-in-links")]
#[tokio::test]
async fn a_password_reset_retires_a_sign_in_link_mailed_before_it() {
    const SIGN_IN_SUBJECT: &str = "Sign in";
    let app = started().await;
    sign_up(&app, "members").await.assert_ok();
    app.post("/members/sign-in-link")
        .json(&json!({ "email": "ada@example.com" }))
        .await
        .assert_ok();
    let sign_in_link = mailed_links(&app, SIGN_IN_SUBJECT).await.remove(0);
    ask_reset(&app, "ada@example.com").await.assert_ok();
    let reset_link = mailed_links(&app, RESET_SUBJECT).await.remove(0);

    reset(&app, &reset_link, NEW_PASSWORD).await.assert_ok();
    let followed = app.post(&sign_in_link).json(&json!({})).await;

    assert_eq!(followed.status, 422, "{}", followed.body);
    assert!(followed.session().is_none());
}

#[tokio::test]
async fn the_sign_up_fixture_confirms_the_address_an_area_asks_for() {
    let app = started().await;

    let session =
        sofie_identity::testing::signed_up::<Confirmed>(&app, "ada@example.com", PASSWORD).await;

    assert!(app.principal_of::<Confirmed>(&session).is_some());
}

#[cfg(feature = "tokens")]
#[tokio::test]
async fn a_password_reset_revokes_the_account_s_access_tokens() {
    let app = started().await;
    sign_up(&app, "members").await;
    let rows = app
        .query("SELECT id FROM sofie_identity.account", &[])
        .await;
    let sofie::core::SqlValue::Uuid(account) = rows[0][0] else {
        panic!("{rows:?}");
    };
    let read = app.db().read();
    let in_a_day = app
        .db()
        .clock()
        .now()
        .checked_add(Duration::from_hours(24))
        .unwrap();
    let mut issued = sofie_identity::tokens::issue::<Members>(
        &read,
        sofie_identity::AccountId::from_uuid(account),
        &["shop.order.*.read"],
        in_a_day,
    )
    .await
    .unwrap();
    app.db().commit(&mut issued).await.unwrap();
    ask_reset(&app, "ada@example.com").await;
    let link = mailed_links(&app, RESET_SUBJECT).await.remove(0);

    reset(&app, &link, NEW_PASSWORD).await;

    let live = app
        .query(
            "SELECT count(*) FROM sofie_identity.access_token WHERE deleted_at IS NULL",
            &[],
        )
        .await;
    assert_eq!(live, [vec![sofie::core::SqlValue::Int(0)]]);
}
