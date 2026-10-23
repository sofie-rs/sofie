#![cfg(feature = "permissions")]

use sofie::db::Db;
use sofie::http::{Gates, Permission, Permissions, RoutePermission, SigningKeys};
use sofie::prelude::*;
use sofie_identity::{Memberships, create_role, grant, grant_as, revoke};
use sofie_testing::{TestApp, TestSession};
use support::Console;

mod support;

const READ_ORDERS: Permission = Permission::new("shop.order.*.read");
const CANCEL_ORDERS: Permission = Permission::new("shop.order.*.cancel");

const ROUTE_PERMISSIONS: &[RoutePermission] = &[
    RoutePermission {
        method: "GET",
        path: "/console/orders",
        permission: READ_ORDERS,
    },
    RoutePermission {
        method: "POST",
        path: "/console/orders/cancel",
        permission: CANCEL_ORDERS,
    },
];

async fn list_orders(_console: Principal<Console>, permissions: Permissions) -> Result<()> {
    if permissions.can(READ_ORDERS) {
        return Ok(());
    }
    Err(Error::Rule(
        "the handler ran without the route's permission".to_owned(),
    ))
}

async fn cancel_orders(_console: Principal<Console>) -> Result<()> {
    Ok(())
}

fn permitted_routes(routes: &mut Routes<Console>) {
    routes.get("/orders", list_orders);
    routes.post("/orders/cancel", cancel_orders);
}

fn app(db: Db, permissions: &'static [RoutePermission]) -> Result<App> {
    let mut screens = sofie::http::Screens::default();
    screens.add_builtin(sofie_identity::module(&[]).screens)?;
    Ok(App::new(db)
        .screens(screens)
        .membership(Memberships)
        .gates(Gates::default().route_permissions(permissions))
        .area::<Web>("sofie_identity", sofie_identity::routes::<Console>)
        .area::<Console>("shop", permitted_routes))
}

async fn started() -> TestApp {
    sofie_testing::start(
        |db| async move {
            db.schema().migrate(&[sofie_identity::migrations()]).await?;
            app(db, ROUTE_PERMISSIONS)
        },
        sofie::worker::Worker::new,
    )
    .await
}

async fn status(app: &TestApp, method: &str, path: &str, session: &TestSession) -> u16 {
    let request = match method {
        "GET" => app.get(path),
        _ => app.post(path),
    };
    request.session(session).accept_json().await.status.as_u16()
}

#[tokio::test]
async fn a_console_principal_reaches_a_route_only_while_its_role_grants_its_permission() {
    let app = started().await;
    let (account, session) = support::joined::<Console>(&app, "grace@example.com").await;
    let read = app.db().read();
    assert_eq!(status(&app, "GET", "/console/orders", &session).await, 403);

    let role = support::grant_role(&app, account, "support", &["shop.*.*.read"]).await;

    assert_eq!(status(&app, "GET", "/console/orders", &session).await, 204);
    assert_eq!(
        status(&app, "POST", "/console/orders/cancel", &session).await,
        403
    );

    let mut cancelling = grant(&read, role, "shop.order.*.cancel").await.unwrap();
    app.db().commit(&mut cancelling).await.unwrap();
    assert_eq!(
        status(&app, "POST", "/console/orders/cancel", &session).await,
        204
    );

    let mut revoked = revoke(&read, role, "shop.*.*.read").await.unwrap();
    app.db().commit(&mut revoked).await.unwrap();
    assert_eq!(status(&app, "GET", "/console/orders", &session).await, 403);
}

#[tokio::test]
async fn a_visitor_is_sent_to_sign_in_and_a_grant_must_be_a_permission() {
    let app = started().await;
    let anonymous = app.get("/console/orders").accept_json().await;
    assert_eq!(anonymous.status, 401, "{}", anonymous.body);

    let read = app.db().read();
    let mut role = create_role(&read, "support").await.unwrap();
    app.db().commit(&mut role).await.unwrap();
    assert!(grant(&read, role.id(), "shop.order.cancel").await.is_err());
    assert!(
        grant_as(
            &read,
            &Permissions::default(),
            role.id(),
            "shop.order.*.cancel"
        )
        .await
        .is_err(),
        "a granter holding nothing grants nothing"
    );
    assert!(create_role(&read, "support").await.is_err());
    assert!(
        revoke(&read, role.id(), "shop.order.*.cancel")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_permitted_route_the_wiring_names_no_permission_for_stops_the_app() {
    let database = sofie_testing::TestDatabase::create().await;
    let db = Db::new(sofie::postgres::Postgres::connect(database.url()).unwrap());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let served = app(db, &ROUTE_PERMISSIONS[..1])
        .unwrap()
        .serving(|serving| serving.signing_keys(SigningKeys::generate().unwrap()))
        .serve(listener, async {})
        .await;
    let error = served.expect_err("the app does not start");
    assert!(
        error.to_string().contains("POST /console/orders/cancel"),
        "{error}"
    );
}

async fn admin_role(app: &TestApp) {
    let read = app.db().read();
    let mut role = create_role(&read, "admin").await.unwrap();
    app.db().commit(&mut role).await.unwrap();
    let mut granted = grant(&read, role.id(), "shop.*.*.*").await.unwrap();
    app.db().commit(&mut granted).await.unwrap();
}

#[tokio::test]
async fn a_task_adds_a_new_address_as_a_member_with_a_role() {
    let app = started().await;
    admin_role(&app).await;
    let tasks =
        sofie::Tasks::new().task("sofie_identity.add_member", sofie_identity::add_member_task);
    let arguments = |email: &str| {
        serde_json::json!({ "email": email, "area": "console", "role": "admin" })
            .as_object()
            .unwrap()
            .clone()
    };

    tasks
        .run(
            app.db(),
            "sofie_identity.add_member",
            arguments("grace@example.com"),
        )
        .await
        .unwrap();
    tasks
        .run(
            app.db(),
            "sofie_identity.add_member",
            arguments("grace@example.com"),
        )
        .await
        .unwrap();
    let unknown_role = tasks
        .run(
            app.db(),
            "sofie_identity.add_member",
            serde_json::json!({ "email": "bob@example.com", "area": "console", "role": "nobody" })
                .as_object()
                .unwrap()
                .clone(),
        )
        .await;

    let members = app
        .query(
            "SELECT m.area, r.name FROM sofie_identity.membership m \
             JOIN sofie_identity.account a ON a.id = m.account \
             JOIN sofie_identity.role r ON r.id = m.role WHERE a.email = 'grace@example.com'",
            &[],
        )
        .await;
    assert_eq!(
        members,
        [vec![
            sofie::core::SqlValue::Text("console".to_owned()),
            sofie::core::SqlValue::Text("admin".to_owned())
        ]]
    );
    assert!(unknown_role.is_err());
    let bob = app
        .query(
            "SELECT count(*) FROM sofie_identity.account WHERE email = 'bob@example.com'",
            &[],
        )
        .await;
    assert_eq!(bob, [vec![sofie::core::SqlValue::Int(0)]]);
}

#[tokio::test]
async fn an_existing_account_added_with_a_role_reaches_what_the_role_grants() {
    let app = started().await;
    admin_role(&app).await;
    let (account, session) = support::joined::<Console>(&app, "ada@example.com").await;
    support::confirm(&app, account).await;
    assert_eq!(status(&app, "GET", "/console/orders", &session).await, 403);
    let read = app.db().read();
    let role = sofie_identity::role_named(&read, "admin").await.unwrap();

    let mut added = sofie_identity::add_member(
        &read,
        &app.db().clock(),
        sofie::values::Email::parse("ada@example.com").unwrap(),
        "console",
    )
    .await
    .unwrap()
    .with_role(&read, &app.db().clock(), role)
    .await
    .unwrap();
    app.db().commit(&mut added).await.unwrap();

    assert_eq!(status(&app, "GET", "/console/orders", &session).await, 204);
}

async fn signs_in(app: &TestApp) -> bool {
    let credentials =
        serde_json::json!({ "email": "ada@example.com", "password": support::PASSWORD });
    app.post("/console/sign-in")
        .json(&credentials)
        .await
        .session()
        .is_some()
}

#[tokio::test]
async fn adding_an_unconfirmed_member_again_with_a_role_ends_its_password_and_sessions() {
    let app = started().await;
    admin_role(&app).await;
    let (_, session) = support::joined::<Console>(&app, "ada@example.com").await;
    let read = app.db().read();
    let role = sofie_identity::role_named(&read, "admin").await.unwrap();

    let mut added = sofie_identity::add_member(
        &read,
        &app.db().clock(),
        sofie::values::Email::parse("ada@example.com").unwrap(),
        "console",
    )
    .await
    .unwrap()
    .with_role(&read, &app.db().clock(), role)
    .await
    .unwrap();
    app.db().commit(&mut added).await.unwrap();

    assert_eq!(status(&app, "GET", "/console/orders", &session).await, 401);
    assert!(!signs_in(&app).await);
}

#[tokio::test]
async fn assigning_a_role_to_an_unconfirmed_member_ends_its_password_and_sessions() {
    let app = started().await;
    admin_role(&app).await;
    let (account, session) = support::joined::<Console>(&app, "ada@example.com").await;
    let read = app.db().read();
    let role = sofie_identity::role_named(&read, "admin").await.unwrap();

    let mut assigned =
        sofie_identity::assign_role(&read, &app.db().clock(), account, "console", Some(role))
            .await
            .unwrap();
    app.db().commit(&mut assigned).await.unwrap();

    assert_eq!(status(&app, "GET", "/console/orders", &session).await, 401);
    assert!(!signs_in(&app).await);
}

#[tokio::test]
async fn a_test_principal_holds_exactly_the_grants_it_was_given() {
    let app = started().await;

    let reader = sofie_identity::testing::granted::<Console>(&app, &["shop.*.*.read"]).await;
    let nobody = sofie_identity::testing::granted::<Console>(&app, &[]).await;

    let read = app.as_(&reader).get("/console/orders").accept_json().await;
    let cancel = app
        .as_(&reader)
        .post("/console/orders/cancel")
        .accept_json()
        .await;
    let refused = app.as_(&nobody).get("/console/orders").accept_json().await;
    assert_eq!(read.status, 204, "{}", read.body);
    assert_eq!(cancel.status, 403, "{}", cancel.body);
    assert_eq!(refused.status, 403, "{}", refused.body);
}
