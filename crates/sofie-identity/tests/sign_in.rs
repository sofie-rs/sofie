use serde_json::{Value, json};
use sofie::core::SqlValue;
use sofie::db::Db;
use sofie::prelude::*;
use sofie_identity::{Account, MemberArea, Memberships};
use sofie_testing::TestApp;
use support::{Member, PASSWORD};

mod support;

struct Staff;

impl Area for Staff {
    const NAME: &'static str = "staff";
    const PREFIX: &'static str = "/staff";
    const SIGN_IN: &'static str = "/staff/sign-in";
    type Principal = Account;
}

impl MemberArea for Staff {
    const HOME: &'static str = "/staff/home";
}

async fn orders() -> Result<()> {
    Ok(())
}

async fn started() -> TestApp {
    sofie_testing::start(
        |db: Db| async move {
            db.schema().migrate(&[sofie_identity::migrations()]).await?;
            let mut screens = sofie::http::Screens::default();
            screens.add_builtin(sofie_identity::module(&[]).screens)?;
            Ok(App::new(db)
                .screens(screens)
                .membership(Memberships)
                .area::<Web>("sofie_identity", sofie_identity::routes::<Member>)
                .area::<Web>("sofie_identity", sofie_identity::routes::<Staff>)
                .area::<Member>("shop", |routes: &mut Routes<Member>| {
                    routes.get("/orders", orders);
                }))
        },
        sofie::worker::Worker::new,
    )
    .await
}

/// `PASSWORD` as the PBKDF2 scheme before Argon2id stored it.
const PBKDF2_HASH: &str =
    "pbkdf2-sha256$600000$BwcHBwcHBwcHBwcHBwcHBw==$FkJR+m1A4gsPGyTioZwmxtW4nbdchEla90scJU+Ljbo=";

fn credentials(password: &str) -> Value {
    json!({ "email": "ada@example.com", "password": password })
}

async fn stored_hash(app: &TestApp) -> String {
    let rows = app
        .query(
            "SELECT password_hash FROM sofie_identity.account WHERE email = $1",
            &[SqlValue::Text("ada@example.com".to_owned())],
        )
        .await;
    let [row] = rows.as_slice() else {
        panic!("one account: {rows:?}");
    };
    let [SqlValue::Text(hash)] = row.as_slice() else {
        panic!("a text hash: {row:?}");
    };
    hash.clone()
}

#[tokio::test]
async fn signing_in_over_an_old_pbkdf2_hash_stores_an_argon2id_one() {
    let app = started().await;
    app.post("/sign-up")
        .json(&credentials(PASSWORD))
        .await
        .assert_ok();
    app.query(
        "UPDATE sofie_identity.account SET password_hash = $1 WHERE email = $2",
        &[
            SqlValue::Text(PBKDF2_HASH.to_owned()),
            SqlValue::Text("ada@example.com".to_owned()),
        ],
    )
    .await;

    let wrong = app
        .post("/sign-in")
        .json(&credentials("not the password"))
        .await;
    assert_eq!(wrong.status, 422, "{}", wrong.body);
    assert_eq!(
        stored_hash(&app).await,
        PBKDF2_HASH,
        "a wrong password rehashes nothing"
    );

    let right = app.post("/sign-in").json(&credentials(PASSWORD)).await;
    assert!(right.session().is_some(), "{}", right.body);
    let rehashed = stored_hash(&app).await;
    assert!(
        rehashed.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"),
        "{rehashed}"
    );
    let again = app.post("/sign-in").json(&credentials(PASSWORD)).await;
    assert!(again.session().is_some(), "{}", again.body);
    assert_eq!(stored_hash(&app).await, rehashed, "a current hash stays");
}

fn hash_before_normalization(typed: &str) -> String {
    use argon2::{Argon2, PasswordHasher};
    Argon2::default()
        .hash_password_with_salt(typed.as_bytes(), &[7; 16])
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn signing_in_over_a_hash_of_a_password_before_nfc_stores_its_nfc_hash() {
    let composed = "\u{E7}orrect horse battery";
    let decomposed = "c\u{327}orrect horse battery";
    let app = started().await;
    app.post("/sign-up")
        .json(&credentials(PASSWORD))
        .await
        .assert_ok();
    let before = hash_before_normalization(decomposed);
    app.query(
        "UPDATE sofie_identity.account SET password_hash = $1 WHERE email = $2",
        &[
            SqlValue::Text(before.clone()),
            SqlValue::Text("ada@example.com".to_owned()),
        ],
    )
    .await;

    let as_typed = app.post("/sign-in").json(&credentials(decomposed)).await;
    assert!(as_typed.session().is_some(), "{}", as_typed.body);
    assert_ne!(stored_hash(&app).await, before, "replaced by its NFC hash");

    let other_form = app.post("/sign-in").json(&credentials(composed)).await;
    assert!(other_form.session().is_some(), "{}", other_form.body);
}

#[tokio::test]
async fn a_password_that_nfc_shortens_under_the_policy_still_signs_in() {
    let decomposed = "u\u{308}".repeat(6);
    let app = started().await;
    app.post("/sign-up")
        .json(&credentials(PASSWORD))
        .await
        .assert_ok();
    app.query(
        "UPDATE sofie_identity.account SET password_hash = $1 WHERE email = $2",
        &[
            SqlValue::Text(hash_before_normalization(&decomposed)),
            SqlValue::Text("ada@example.com".to_owned()),
        ],
    )
    .await;

    let signed_in = app.post("/sign-in").json(&credentials(&decomposed)).await;

    assert!(signed_in.session().is_some(), "{}", signed_in.body);
}

#[tokio::test]
async fn a_right_password_outside_the_area_is_answered_as_a_wrong_one() {
    let app = started().await;
    app.post("/sign-up")
        .json(&credentials("correct horse battery"))
        .await
        .assert_ok();

    let right = app
        .post("/staff/sign-in")
        .json(&credentials("correct horse battery"))
        .await;
    let wrong = app
        .post("/staff/sign-in")
        .json(&credentials("not the password"))
        .await;

    assert_eq!(right.status, wrong.status);
    assert_eq!(right.body, wrong.body);
    assert_eq!(right.json()["error"], "The email or password is not right.");
}

#[tokio::test]
async fn sign_ups_are_rate_limited_per_address() {
    let app = started().await;

    let mut statuses = Vec::new();
    for attempt in 0..11 {
        let account = json!({
            "email": format!("visitor{attempt}@example.com"),
            "password": "correct horse battery",
        });
        statuses.push(app.post("/sign-up").json(&account).await.status.as_u16());
    }

    assert_eq!(statuses.last(), Some(&429), "{statuses:?}");
}

#[tokio::test]
async fn sign_up_carries_next_to_its_form_and_its_links_and_lands_on_checking_mail() {
    let app = started().await;
    let next = "next=/account/orders";

    let sign_up = app.get(&format!("/sign-up?{next}")).accept_json().await;
    let sign_in = app.get(&format!("/sign-in?{next}")).accept_json().await;
    let signed_up = app
        .post(&format!("/sign-up?{next}"))
        .form(&[
            ("email", "ada@example.com"),
            ("password", "correct horse battery"),
        ])
        .htmx("sign-up")
        .trigger("sign-up")
        .await;

    assert_eq!(sign_up.json()["action"], format!("/sign-up?{next}"));
    assert_eq!(sign_up.json()["other"], format!("/sign-in?{next}"));
    assert_eq!(sign_in.json()["other"], format!("/sign-up?{next}"));
    assert_eq!(
        signed_up.header("hx-redirect"),
        "/account/link-sent",
        "signing up signs nobody in, so it is told to check its mail, not sent on"
    );
    assert!(signed_up.session().is_none());
}

#[tokio::test]
async fn an_app_checks_a_current_password_chosen_under_an_older_length_policy() {
    let short = "eleven char";
    let hash =
        sofie_identity::PasswordHash::new(&sofie_identity::Password::entered(short.to_owned()))
            .await
            .unwrap();

    let current = sofie_identity::Password::entered(short.to_owned());

    assert!(sofie_identity::Password::parse(short.to_owned()).is_err());
    assert!(current.check(Some(&hash)).await.unwrap().is_right());
}
