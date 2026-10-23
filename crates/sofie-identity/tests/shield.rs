#![cfg(feature = "permitted-shield")]

use std::time::Duration;

use serde_json::{Value, json};
use sofie::db::Db;
use sofie::http::{Shield, ShieldMode};
use sofie::prelude::*;
use sofie_identity::{Memberships, SignInLocked, permitted_routes};
use sofie_testing::{TestApp, TestResponse, TestSession};
use support::{Console, Member, PASSWORD, account_id};

mod support;

const DECIDED_WITHIN: Duration = Duration::from_secs(5);

async fn started() -> TestApp {
    sofie_testing::start(
        |db: Db| async move {
            db.schema().migrate(&[sofie_identity::migrations()]).await?;
            let mut screens = sofie::http::Screens::default();
            screens.add_builtin(sofie_identity::module(&[]).screens)?;
            Ok(App::new(db)
                .screens(screens)
                .membership(Memberships)
                .serving(|serving| serving.shield(Shield::new(ShieldMode::Enforce)))
                .area::<Web>("sofie_identity", sofie_identity::routes::<Console>)
                .area::<Web>("sofie_identity", sofie_identity::routes::<Member>)
                .area::<Console>("sofie_identity", permitted_routes::<Console>))
        },
        sofie::worker::Worker::new,
    )
    .await
}

fn credentials(email: &str, password: &str) -> Value {
    json!({ "email": email, "password": password })
}

async fn console(app: &TestApp) -> TestSession {
    let (account, session) = support::joined::<Console>(app, "ada@example.com").await;
    support::grant_role(app, account, "security", &["sofie_identity.shield.*.*"]).await;
    session
}

/// The clock moves between tries so the route's own rate limit refills.
async fn refused_while_locked(app: &TestApp, email: &str) -> TestResponse {
    let started = std::time::Instant::now();
    loop {
        app.clock().advance(Duration::from_secs(6)).unwrap();
        let answer = app
            .post("/console/sign-in")
            .json(&credentials(email, PASSWORD))
            .await;
        if answer.status == 422 || started.elapsed() > DECIDED_WITHIN {
            return answer;
        }
    }
}

async fn owner_notices(app: &TestApp) -> Vec<SignInLocked> {
    let started = std::time::Instant::now();
    loop {
        let notices = sofie_testing::raised::<SignInLocked>(app);
        if !notices.is_empty() || started.elapsed() > DECIDED_WITHIN {
            return notices;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn a_guessed_account_is_locked_its_owner_told_and_a_console_principal_lifts_it() {
    let app = started().await;
    let session = console(&app).await;
    let victim = credentials("victim@example.com", PASSWORD);
    assert_eq!(app.post("/console/sign-up").json(&victim).await.status, 204);
    let account = account_id(&app, "victim@example.com").await;
    app.clock().advance(Duration::from_mins(1)).unwrap();

    let guess = credentials("victim@example.com", "not the password");
    for _ in 0..10 {
        assert_eq!(app.post("/console/sign-in").json(&guess).await.status, 422);
    }
    let refused = refused_while_locked(&app, "victim@example.com").await;
    app.clock().advance(Duration::from_secs(6)).unwrap();
    let wrong = app.post("/console/sign-in").json(&guess).await;

    assert_eq!(refused.status, 422, "{}", refused.body);
    assert_eq!(
        (refused.status, &refused.body),
        (wrong.status, &wrong.body),
        "a lock tells no guess right"
    );
    let notices = owner_notices(&app).await;
    assert_eq!(notices.len(), 1, "the owner is told once");
    assert_eq!(notices[0].account, account.uuid());

    let screen = app
        .get("/console/shield")
        .session(&session)
        .accept_json()
        .await;
    assert_eq!(screen.status, 200, "{}", screen.body);
    let blocks = screen.json();
    let caller = blocks[0]["caller"].as_str().unwrap();
    assert!(
        caller.starts_with(&format!("account {} from ", account.uuid())),
        "{caller}"
    );
    assert_eq!(blocks[0]["grade"], "lock");
    let lifted = app
        .post("/console/shield")
        .session(&session)
        .json(&json!({ "block": blocks[0]["id"].to_string() }))
        .await;
    assert_eq!(lifted.status, 204, "{}", lifted.body);
    app.clock().advance(Duration::from_secs(6)).unwrap();
    let signed_in = app.post("/console/sign-in").json(&victim).await;
    assert!(signed_in.session().is_some(), "{}", signed_in.body);
}

#[tokio::test]
async fn a_wrong_password_reads_the_same_whether_or_not_the_account_is_locked() {
    let app = started().await;
    let victim = credentials("victim@example.com", PASSWORD);
    assert_eq!(app.post("/console/sign-up").json(&victim).await.status, 204);
    let guess = credentials("victim@example.com", "not the password");
    let before = app.post("/console/sign-in").json(&guess).await;
    for _ in 0..9 {
        app.post("/console/sign-in").json(&guess).await;
    }
    refused_while_locked(&app, "victim@example.com").await;

    app.clock().advance(Duration::from_mins(1)).unwrap();
    let after = app.post("/console/sign-in").json(&guess).await;

    assert_eq!((after.status, &after.body), (before.status, &before.body));
}

#[tokio::test]
async fn a_right_password_in_an_area_the_account_is_not_in_locks_nothing() {
    let app = started().await;
    let ada = credentials("ada@example.com", PASSWORD);
    assert_eq!(app.post("/sign-up").json(&ada).await.status, 204);
    app.clock().advance(Duration::from_mins(1)).unwrap();
    for _ in 0..10 {
        assert_eq!(app.post("/console/sign-in").json(&ada).await.status, 422);
    }

    let started = std::time::Instant::now();
    while started.elapsed() < DECIDED_WITHIN {
        app.clock().advance(Duration::from_secs(6)).unwrap();
        let signed_in = app.post("/sign-in").json(&ada).await;
        assert!(signed_in.session().is_some(), "{}", signed_in.body);
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}
