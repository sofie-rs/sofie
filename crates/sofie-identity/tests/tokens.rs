#![cfg(feature = "tokens")]

use std::time::Duration;

use sofie::core::{Instant, SqlValue};
use sofie::db::Db;
use sofie::http::{Gates, Permission, RoutePermission};
use sofie::prelude::*;
use sofie_identity::tokens::{self, AccessToken};
use sofie_identity::{AccountId, Memberships};
use sofie_testing::{TestApp, TestResponse};
use support::Console;

mod support;

const READ_TOKENS: &str = "sofie_identity.access_token.*.read";
const CANCEL_ORDERS: &str = "shop.order.*.cancel";

const ROUTE_PERMISSIONS: &[RoutePermission] = &[
    RoutePermission {
        method: "GET",
        path: "/console/tokens",
        permission: Permission::new(READ_TOKENS),
    },
    RoutePermission {
        method: "POST",
        path: "/console/orders/cancel",
        permission: Permission::new(CANCEL_ORDERS),
    },
];

#[derive(ReadModel)]
#[read(from = AccessToken)]
struct TokenRow {
    id: Id<AccessToken>,
}

async fn own_tokens(console: Principal<Console>, read: Read) -> Result<Vec<TokenRow>> {
    let account = console.id();
    TokenRow::query()
        .where_(AccessToken::ACCOUNT.eq(account))
        .all(&read)
        .await
}

async fn cancel_orders(_console: Principal<Console>) -> Result<()> {
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
                .gates(Gates::default().route_permissions(ROUTE_PERMISSIONS))
                .area::<Web>("sofie_identity", sofie_identity::routes::<Console>)
                .area::<Console>("shop", |routes| {
                    routes.get("/tokens", own_tokens);
                    routes.post("/orders/cancel", cancel_orders);
                }))
        },
        sofie::worker::Worker::new,
    )
    .await
}

/// A member whose role grants everything, so only a token's scopes narrow it.
async fn member(app: &TestApp) -> AccountId {
    let (account, _) = support::joined::<Console>(app, "ada@example.com").await;
    support::grant_role(app, account, "operator", &["*.*.*.*"]).await;
    account
}

async fn issued(app: &TestApp, account: AccountId, scopes: &[&str], expires_at: Instant) -> String {
    let mut issued = tokens::issue::<Console>(&app.db().read(), account, scopes, expires_at)
        .await
        .unwrap();
    app.db().commit(&mut issued).await.unwrap();
    issued.secret().expose().to_owned()
}

fn in_a_day(app: &TestApp) -> Instant {
    app.db()
        .clock()
        .now()
        .checked_add(Duration::from_hours(24))
        .unwrap()
}

async fn get(app: &TestApp, token: &str) -> TestResponse {
    app.get("/console/tokens")
        .header("authorization", &format!("Bearer {token}"))
        .accept_json()
        .await
}

fn assert_refused_as_bearer(response: &TestResponse) {
    response.assert_status(401);
    let challenge = response.headers.get("www-authenticate").unwrap();
    assert!(challenge.to_str().unwrap().starts_with("Bearer"));
}

#[tokio::test]
async fn a_token_opens_its_area_within_its_scopes_and_never_a_session() {
    let app = started().await;
    let account = member(&app).await;
    let reading = issued(&app, account, &[READ_TOKENS], in_a_day(&app)).await;

    let listed = get(&app, &reading).await;
    listed.assert_status(200);
    assert_eq!(listed.json().as_array().map(Vec::len), Some(1));
    assert!(listed.headers.get("set-cookie").is_none());
    let used = app
        .query("SELECT last_used_at FROM sofie_identity.access_token", &[])
        .await;
    assert!(matches!(used[0][0], SqlValue::Timestamp(_)));

    let cancel = |token: &str| {
        app.post("/console/orders/cancel")
            .header("authorization", &format!("Bearer {token}"))
            .header("sec-fetch-site", "cross-site")
            .accept_json()
    };
    cancel(&reading).await.assert_status(403);
    let cancelling = issued(&app, account, &[CANCEL_ORDERS], in_a_day(&app)).await;
    let cancelled = cancel(&cancelling).await;
    cancelled.assert_status(204);
    assert!(cancelled.headers.get("set-cookie").is_none());
}

#[tokio::test]
async fn an_unknown_expired_or_revoked_token_is_refused() {
    let app = started().await;
    let account = member(&app).await;

    let unknown = sofie::auth::BearerToken::generate().unwrap();
    assert_refused_as_bearer(&get(&app, unknown.expose()).await);
    assert_refused_as_bearer(&get(&app, "not-a-token").await);

    let yesterday = app
        .db()
        .clock()
        .now()
        .checked_sub(Duration::from_hours(24))
        .unwrap();
    let expired = issued(&app, account, &[READ_TOKENS], yesterday).await;
    assert_refused_as_bearer(&get(&app, &expired).await);

    let live = issued(&app, account, &[READ_TOKENS], in_a_day(&app)).await;
    get(&app, &live).await.assert_status(200);
    let read = app.db().read();
    let held = tokens::list(&read, account).await.unwrap();
    let newest = held.first().unwrap().id();
    let mut revoked = tokens::revoke(&read, account, newest).await.unwrap();
    app.db().commit(&mut revoked).await.unwrap();
    assert_refused_as_bearer(&get(&app, &live).await);
    assert_eq!(tokens::list(&read, account).await.unwrap().len(), 1);
}

#[tokio::test]
async fn failed_lookups_are_throttled_and_scopes_must_be_permissions() {
    let app = started().await;
    let account = member(&app).await;
    for _ in 0..10 {
        get(&app, "not-a-token").await.assert_status(401);
    }
    get(&app, "not-a-token").await.assert_status(429);

    let read = app.db().read();
    let refused = tokens::issue::<Console>(&read, account, &["orders"], in_a_day(&app)).await;
    assert!(refused.is_err());
}

#[tokio::test]
async fn a_removed_members_token_stops_working_at_once() {
    let app = started().await;
    let account = member(&app).await;
    let live = issued(&app, account, &[READ_TOKENS], in_a_day(&app)).await;
    get(&app, &live).await.assert_status(200);

    app.query("DELETE FROM sofie_identity.membership", &[])
        .await;

    assert_refused_as_bearer(&get(&app, &live).await);
}
