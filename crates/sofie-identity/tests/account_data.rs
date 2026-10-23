#![cfg(feature = "account-data")]

use std::sync::Arc;

use serde_json::{Value, json};
use sofie::core::SqlValue;
use sofie::db::{BoxFuture, Db, Uuid};
use sofie::prelude::*;
use sofie_identity::Memberships;
use sofie_identity::account_data::{AccountClosed, PersonalData};
use sofie_identity::testing::signed_up;
use sofie_testing::{TestApp, assert_raised, sent_mails};
use support::Member;

mod support;

struct Notes;

impl PersonalData for Notes {
    fn name(&self) -> &'static str {
        "notes"
    }

    fn export<'a>(&'a self, _read: &'a Read, account: Uuid) -> BoxFuture<'a, Result<Value>> {
        Box::pin(async move { Ok(json!([{ "owner": account.to_string(), "text": "milk" }])) })
    }
}

async fn started() -> TestApp {
    sofie_testing::start(
        |db: Db| async move {
            db.schema().migrate(&[sofie_identity::migrations()]).await?;
            sofie_identity::account_data::register(&db, Arc::new(Notes));
            #[cfg(feature = "notify")]
            sofie_identity::account_data::register(
                &db,
                Arc::new(sofie_identity::account_data::Notifications::new(&db)),
            );
            let mut screens = sofie::http::Screens::default();
            screens.add_builtin(sofie_identity::module(&[]).screens)?;
            screens.describe_forms(sofie_identity::FORMS)?;
            Ok(App::new(db)
                .screens(screens)
                .membership(Memberships)
                .area::<Web>("sofie_identity", sofie_identity::routes::<Member>)
                .area::<Member>(
                    "sofie_identity",
                    sofie_identity::account_data::routes::<Member>,
                ))
        },
        |db| sofie_identity::worker(sofie::worker::Worker::new(db)),
    )
    .await
}

#[tokio::test]
async fn a_member_exports_its_data_and_closes_its_account() {
    let app = started().await;
    let credentials = json!({ "email": "ada@example.com", "password": "correct horse battery" });
    let session = signed_up::<Member>(&app, "ada@example.com", "correct horse battery").await;
    app.get("/account/account-data")
        .session(&session)
        .await
        .assert_ok();

    let exported = app
        .post("/account/account-data/export")
        .session(&session)
        .accept_json()
        .await;
    exported.assert_ok();
    let exported = exported.json();
    assert_eq!(
        exported["sofie_identity"]["account"]["email"],
        "ada@example.com"
    );
    assert_eq!(
        exported["sofie_identity"]["memberships"][0]["area"],
        "member"
    );
    assert_eq!(exported["notes"][0]["text"], "milk");

    let close = |email: &'static str| {
        app.post("/account/account-data/close")
            .session(&session)
            .accept_json()
            .json(&json!({ "email": email }))
    };
    assert_eq!(close("eve@example.com").await.status, 422);
    let closed = close("ada@example.com").await;
    closed.assert_ok();

    assert!(
        closed.header("set-cookie").contains("Max-Age=0"),
        "signed out"
    );
    assert_eq!(app.post("/sign-in").json(&credentials).await.status, 422);
    let addresses = app
        .query("SELECT email FROM sofie_identity.account", &[])
        .await;
    let SqlValue::Text(address) = &addresses[0][0] else {
        panic!("{addresses:?}");
    };
    assert!(address.ends_with("@closed.invalid"), "{address}");
    assert_raised::<AccountClosed>(&app);
}

#[tokio::test]
async fn closing_an_account_forgets_its_product_events() {
    let app = started().await;
    let session = signed_up::<Member>(&app, "ada@example.com", "correct horse battery").await;
    let account = app.principal_of::<Member>(&session).unwrap().id().uuid();
    let other = Uuid::from_u128(7);
    for principal in [account, other] {
        app.query(
            "INSERT INTO sofie.product_events (at, name, principal) VALUES (now(), 'ordered', $1)",
            &[SqlValue::Uuid(principal)],
        )
        .await;
    }

    app.post("/account/account-data/close")
        .session(&session)
        .accept_json()
        .json(&json!({ "email": "ada@example.com" }))
        .await
        .assert_ok();
    let kept = app
        .query("SELECT principal FROM sofie.product_events", &[])
        .await;

    assert_eq!(kept, vec![vec![SqlValue::Uuid(other)]]);
}

#[cfg(feature = "notify")]
#[tokio::test]
async fn closing_an_account_erases_its_notifications() {
    let app = started().await;
    let session = signed_up::<Member>(&app, "ada@example.com", "correct horse battery").await;
    let account = app.principal_of::<Member>(&session).unwrap().id().uuid();
    let other = Uuid::from_u128(7);
    for (id, principal) in [(Uuid::from_u128(1), account), (Uuid::from_u128(2), other)] {
        app.query(
            "INSERT INTO sofie.inbox (id, version, principal, topic, title, body, created_at) \
             VALUES ($1, 1, $2, 'shop.shipped', 'Shipped', '', now())",
            &[SqlValue::Uuid(id), SqlValue::Uuid(principal)],
        )
        .await;
    }
    let exported = app
        .post("/account/account-data/export")
        .session(&session)
        .accept_json()
        .await
        .json();

    app.post("/account/account-data/close")
        .session(&session)
        .accept_json()
        .json(&json!({ "email": "ada@example.com" }))
        .await
        .assert_ok();
    let kept = app.query("SELECT principal FROM sofie.inbox", &[]).await;

    assert_eq!(exported["sofie_notify"]["inbox"][0]["title"], "Shipped");
    assert_eq!(kept, vec![vec![SqlValue::Uuid(other)]]);
}

#[cfg(feature = "sign-in-links")]
#[tokio::test]
async fn closing_an_account_uses_up_the_links_mailed_to_it() {
    let app = started().await;
    let session = signed_up::<Member>(&app, "ada@example.com", "correct horse battery").await;
    let asked = app
        .post("/account/sign-in-link")
        .json(&json!({ "email": "ada@example.com" }))
        .await;
    assert_eq!(asked.status, 204, "{}", asked.body);
    app.drain().await;
    let mails = sent_mails(&app);
    let text = mails[0].body().text();
    let start = text.find("http://localhost").unwrap() + "http://localhost".len();
    let link = &text[start..start + text[start..].find(')').unwrap()];

    app.post("/account/account-data/close")
        .session(&session)
        .accept_json()
        .json(&json!({ "email": "ada@example.com" }))
        .await
        .assert_ok();
    let followed = app.post(link).json(&json!({})).await;

    assert_eq!(followed.status, 422, "{}", followed.body);
    assert!(followed.session().is_none());
}

#[tokio::test]
async fn the_export_lists_tokens_the_second_factor_and_impersonations_without_secrets() {
    let app = started().await;
    let session = signed_up::<Member>(&app, "ada@example.com", "correct horse battery").await;
    let account = app.principal_of::<Member>(&session).unwrap();
    let read = app.db().read();
    let in_a_day = app
        .db()
        .clock()
        .now()
        .checked_add(std::time::Duration::from_hours(24))
        .unwrap();
    let mut issued = sofie_identity::tokens::issue::<Member>(
        &read,
        sofie_identity::AccountId::from_uuid(account.id().uuid()),
        &["shop.order.*.read"],
        in_a_day,
    )
    .await
    .unwrap();
    app.db().commit(&mut issued).await.unwrap();
    let secret = issued.secret().expose().to_owned();

    let exported = app
        .post("/account/account-data/export")
        .session(&session)
        .accept_json()
        .await;

    exported.assert_ok();
    let own = &exported.json()["sofie_identity"];
    assert_eq!(own["tokens"][0]["area"], "member", "{own}");
    assert_eq!(own["tokens"][0]["scopes"], json!(["shop.order.*.read"]));
    assert!(own["second_factor"].is_null(), "{own}");
    assert_eq!(own["impersonations"], json!([]));
    assert!(own["account"].get("verified_at").is_some(), "{own}");
    assert!(!exported.body.contains(&secret));
}
