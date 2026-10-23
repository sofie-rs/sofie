#![cfg(feature = "token-screens")]

use std::time::Duration;

use serde_json::json;
use sofie::db::Db;
use sofie::prelude::*;
use sofie_identity::tokens::{self, AccessTokenId};
use sofie_identity::{AccountId, Memberships};
use sofie_testing::{TestApp, TestSession};
use support::Console;

mod support;

async fn started() -> TestApp {
    sofie_testing::start(
        |db: Db| async move {
            db.schema().migrate(&[sofie_identity::migrations()]).await?;
            let mut screens = sofie::http::Screens::default();
            screens.add_builtin(sofie_identity::module(&[]).screens)?;
            Ok(App::new(db)
                .screens(screens)
                .membership(Memberships)
                .area::<Web>("sofie_identity", sofie_identity::routes::<Console>)
                .area::<Console>("sofie_identity", tokens::screen::routes::<Console>))
        },
        sofie::worker::Worker::new,
    )
    .await
}

async fn member(app: &TestApp, email: &str) -> (AccountId, TestSession) {
    let (account, session) = support::joined::<Console>(app, email).await;
    support::grant_role(app, account, email, &["sofie_identity.access_token.*.*"]).await;
    (account, session)
}

async fn issued(app: &TestApp, account: AccountId) -> AccessTokenId {
    let expires_at = app
        .db()
        .clock()
        .now()
        .checked_add(Duration::from_hours(24))
        .unwrap();
    let mut issued = tokens::issue::<Console>(&app.db().read(), account, &[], expires_at)
        .await
        .unwrap();
    app.db().commit(&mut issued).await.unwrap();
    issued.token().id()
}

#[tokio::test]
async fn a_principal_lists_and_revokes_only_its_own_tokens() {
    let app = started().await;
    let (ada, session) = member(&app, "ada@example.com").await;
    let (grace, _) = member(&app, "grace@example.com").await;
    let kept = issued(&app, ada).await;
    let revoked = issued(&app, ada).await;
    let foreign = issued(&app, grace).await;

    let page = app.get("/console/tokens").session(&session).await;
    assert_eq!(page.status, 200, "{}", page.body);
    for own in [kept, revoked] {
        assert!(page.body.contains(&own.to_string()), "{}", page.body);
    }
    assert!(!page.body.contains(&foreign.to_string()), "{}", page.body);

    let revoke = |token: AccessTokenId| {
        app.post("/console/tokens")
            .session(&session)
            .json(&json!({ "token": token.to_string() }))
    };
    revoke(foreign).await.assert_status(404);
    revoke(revoked).await.assert_ok();

    let listed = app
        .get("/console/tokens")
        .session(&session)
        .accept_json()
        .await
        .json();
    let ids: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|token| token["id"].as_str())
        .collect();
    assert_eq!(ids, [kept.to_string()]);
    let held = tokens::list(&app.db().read(), grace).await.unwrap();
    assert_eq!(held.len(), 1);
}
