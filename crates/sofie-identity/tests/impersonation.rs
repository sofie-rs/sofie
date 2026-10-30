#![cfg(feature = "impersonation")]

use std::time::Duration;

use serde_json::{Value, json};
use sofie::db::{Db, Outcome, ReadOnly, Reply};
use sofie::http::{Gates, Permission, RoutePermission};
use sofie::prelude::*;
use sofie_identity::impersonation::{Impersonating, ImpersonationEnded, ImpersonationStarted};
use sofie_identity::{AccountId, Memberships};
use sofie_testing::{TestApp, TestResponse, TestSession, assert_raised};
use support::{Console, Member};

mod support;

const OWN_SESSIONS: Permission = Permission::new("sofie_identity.account_session.*.update");
const OWN_ACCOUNT: Permission = Permission::new("sofie_identity.account.*.update");
const ROUTE_PERMISSIONS: &[RoutePermission] = &[
    RoutePermission {
        method: "GET",
        path: "/console/sessions",
        permission: OWN_SESSIONS,
    },
    RoutePermission {
        method: "POST",
        path: "/console/sessions/{account_session}/end",
        permission: OWN_SESSIONS,
    },
    RoutePermission {
        method: "POST",
        path: "/console/sessions/others",
        permission: OWN_SESSIONS,
    },
    RoutePermission {
        method: "GET",
        path: "/console/account-data",
        permission: OWN_ACCOUNT,
    },
    RoutePermission {
        method: "POST",
        path: "/console/account-data/export",
        permission: OWN_ACCOUNT,
    },
    RoutePermission {
        method: "POST",
        path: "/console/account-data/close",
        permission: OWN_ACCOUNT,
    },
    RoutePermission {
        method: "GET",
        path: "/console/two-factor",
        permission: OWN_ACCOUNT,
    },
    RoutePermission {
        method: "POST",
        path: "/console/two-factor/start",
        permission: OWN_ACCOUNT,
    },
    RoutePermission {
        method: "POST",
        path: "/console/two-factor/confirm",
        permission: OWN_ACCOUNT,
    },
    RoutePermission {
        method: "POST",
        path: "/console/two-factor/disable",
        permission: OWN_ACCOUNT,
    },
];

struct Who(Option<String>);

impl Outcome for Who {
    fn reply(self) -> Result<Reply> {
        Ok(Reply::Json(json!({ "impersonated_by": self.0 })))
    }
}

impl ReadOnly for Who {}

async fn who(_member: Principal<Member>, impersonating: Impersonating) -> Result<Who> {
    Ok(Who(impersonating.by().map(str::to_owned)))
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
                .area::<Web>("sofie_identity", sofie_identity::routes::<Member>)
                .area::<Console>(
                    "sofie_identity",
                    sofie_identity::impersonation::routes::<Console, Member>,
                )
                .area::<Member>(
                    "sofie_identity",
                    sofie_identity::impersonation::stop_routes::<Member, Console>,
                )
                .area::<Member>("sofie_identity", sofie_identity::sessions::routes::<Member>)
                .area::<Console>(
                    "sofie_identity",
                    sofie_identity::sessions::routes::<Console>,
                )
                .area::<Member>("sofie_identity", sofie_identity::passkeys::routes::<Member>)
                .area::<Member>("sofie_identity", sofie_identity::oauth::routes::<Member>)
                .area::<Member>(
                    "sofie_identity",
                    sofie_identity::two_factor::routes::<Member>,
                )
                .area::<Console>(
                    "sofie_identity",
                    sofie_identity::two_factor::routes::<Console>,
                )
                .area::<Member>(
                    "sofie_identity",
                    sofie_identity::account_data::routes::<Member>,
                )
                .area::<Console>(
                    "sofie_identity",
                    sofie_identity::account_data::routes::<Console>,
                )
                .area::<Member>("shop", |routes: &mut Routes<Member>| {
                    routes.get("/who", who);
                }))
        },
        sofie::worker::Worker::new,
    )
    .await
}

async fn grant_impersonation(app: &TestApp, account: AccountId) {
    support::grant_role(
        app,
        account,
        "support",
        &["sofie_identity.impersonation.*.*"],
    )
    .await;
}

async fn impersonate(app: &TestApp, staff: &TestSession, email: &str) -> TestResponse {
    app.post("/console/impersonation")
        .session(staff)
        .accept_json()
        .json(&json!({ "email": email, "reason": "ticket 42: cart is empty" }))
        .await
}

async fn json(app: &TestApp, path: &str, session: &TestSession) -> Value {
    let answer = app.get(path).session(session).accept_json().await;
    answer.assert_ok();
    answer.json()
}

#[tokio::test]
async fn a_permitted_principal_signs_in_as_a_member_and_back_on_the_record() {
    let app = started().await;
    let (grace, staff) = support::joined::<Console>(&app, "grace@example.com").await;
    support::joined::<Member>(&app, "ada@example.com").await;
    assert_eq!(
        impersonate(&app, &staff, "ada@example.com").await.status,
        403
    );
    grant_impersonation(&app, grace).await;

    let started = impersonate(&app, &staff, "ada@example.com").await;
    started.assert_status(204);
    let ada = started.session().expect("a session as the member");

    assert!(app.principal_of::<Member>(&ada).is_some());
    assert_eq!(
        json(&app, "/account/who", &ada).await["impersonated_by"],
        "grace@example.com"
    );
    let sessions = json(&app, "/account/sessions", &ada).await;
    let devices: Vec<&str> = sessions
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|session| session["device"].as_str())
        .collect();
    assert!(
        devices.contains(&"Impersonated by grace@example.com"),
        "{sessions}"
    );
    let stopped = app
        .post("/account/impersonation/stop")
        .session(&ada)
        .accept_json()
        .await;
    stopped.assert_status(204);
    let back = stopped.session().expect("the impersonator signed back in");
    assert!(app.principal_of::<Console>(&back).is_some());
    app.get("/account/who")
        .session(&ada)
        .accept_json()
        .await
        .assert_status(401);

    let record = json(&app, "/console/impersonation", &back).await;
    let first = &record["impersonations"][0];
    assert_eq!(
        (first["impersonator"].clone(), first["member"].clone()),
        (json!("grace@example.com"), json!("ada@example.com"))
    );
    assert_eq!(first["reason"], "ticket 42: cart is empty");
    assert!(!first["ended_at"].is_null(), "{record}");
    let raised = assert_raised::<ImpersonationStarted>(&app);
    assert_eq!(raised[0].reason, "ticket 42: cart is empty");
    assert_raised::<ImpersonationEnded>(&app);
}

async fn start_two_factor(app: &TestApp, session: &TestSession) -> TestResponse {
    app.post("/console/two-factor/start")
        .session(session)
        .accept_json()
        .await
}

async fn stopped(app: &TestApp, member: &TestSession) -> TestSession {
    let stopped = app
        .post("/account/impersonation/stop")
        .session(member)
        .accept_json()
        .await;
    stopped.assert_status(204);
    stopped.session().expect("the impersonator signed back in")
}

#[tokio::test]
async fn stopping_resumes_the_impersonators_own_session_as_old_as_it_was() {
    let app = started().await;
    let (grace, staff) = support::joined::<Console>(&app, "grace@example.com").await;
    support::grant_role(
        &app,
        grace,
        "support",
        &[
            "sofie_identity.impersonation.*.*",
            "sofie_identity.account.*.update",
        ],
    )
    .await;
    support::joined::<Member>(&app, "ada@example.com").await;

    let ada = impersonate(&app, &staff, "ada@example.com")
        .await
        .session()
        .unwrap();
    let back = stopped(&app, &ada).await;
    start_two_factor(&app, &back).await.assert_ok();

    app.clock().advance(Duration::from_mins(11)).unwrap();
    let ada = impersonate(&app, &back, "ada@example.com")
        .await
        .session()
        .unwrap();
    let back = stopped(&app, &ada).await;
    let refused = start_two_factor(&app, &back).await;

    assert_eq!(refused.status, 422, "{}", refused.body);
    assert!(refused.body.contains("Sign in again"), "{}", refused.body);
    json(&app, "/console/impersonation", &staff).await;
    let live = app
        .query(
            "SELECT count(*) FROM sofie_identity.account_session \
             WHERE account = $1 AND ended_at IS NULL",
            &[sofie::core::SqlValue::Uuid(grace.uuid())],
        )
        .await;
    assert_eq!(
        live,
        [vec![sofie::core::SqlValue::Int(1)]],
        "no session left behind"
    );
}

#[tokio::test]
async fn each_request_made_while_impersonating_is_logged_with_both_accounts() {
    let app = started().await;
    let (grace, staff) = support::joined::<Console>(&app, "grace@example.com").await;
    let (ada, _) = support::joined::<Member>(&app, "ada@example.com").await;
    grant_impersonation(&app, grace).await;
    let telemetry = sofie::telemetry::capture();
    json(&app, "/console/impersonation", &staff).await;
    let started = impersonate(&app, &staff, "ada@example.com").await;
    started.assert_status(204);
    let as_ada = started.session().expect("a session as the member");

    json(&app, "/account/who", &as_ada).await;
    json(&app, "/account/who", &as_ada).await;

    let logged: Vec<_> = telemetry
        .events()
        .into_iter()
        .filter(|event| event.fields.contains_key("impersonation.impersonator"))
        .map(|event| {
            let field = |name: &str| event.fields[name].clone();
            (
                event.level.as_str(),
                field("impersonation.account"),
                field("impersonation.impersonator"),
            )
        })
        .collect();
    let expected = ("INFO", ada.uuid().to_string(), grace.uuid().to_string());
    assert_eq!(logged, [expected.clone(), expected]);
}

#[tokio::test]
async fn only_a_member_holding_no_more_than_the_impersonator_is_impersonated() {
    let app = started().await;
    let (grace, staff) = support::joined::<Console>(&app, "grace@example.com").await;
    support::grant_role(
        &app,
        grace,
        "support",
        &["sofie_identity.impersonation.*.*", "shop.order.*.read"],
    )
    .await;
    support::joined::<Member>(&app, "ada@example.com").await;
    let (bob, _) = support::joined::<Member>(&app, "bob@example.com").await;
    support::grant_role_in(&app, "member", bob, "reader", &["shop.order.*.read"]).await;
    let (carol, _) = support::joined::<Member>(&app, "carol@example.com").await;
    support::grant_role_in(
        &app,
        "member",
        carol,
        "refunder",
        &["shop.order.*.read", "shop.order.*.refund"],
    )
    .await;

    impersonate(&app, &staff, "ada@example.com")
        .await
        .assert_status(204);
    impersonate(&app, &staff, "bob@example.com")
        .await
        .assert_status(204);
    let refused = impersonate(&app, &staff, "carol@example.com").await;
    assert_eq!(refused.status, 422, "{}", refused.body);
    assert!(
        refused.body.contains("shop.order.*.refund"),
        "{}",
        refused.body
    );

    let record = json(&app, "/console/impersonation", &staff).await;
    assert_eq!(
        record["impersonations"].as_array().unwrap().len(),
        2,
        "{record}"
    );
}

#[tokio::test]
async fn an_impersonation_ends_once_its_member_is_granted_more() {
    let app = started().await;
    let (grace, staff) = support::joined::<Console>(&app, "grace@example.com").await;
    support::grant_role(
        &app,
        grace,
        "support",
        &["sofie_identity.impersonation.*.*", "shop.order.*.read"],
    )
    .await;
    let (bob, _) = support::joined::<Member>(&app, "bob@example.com").await;
    let reader =
        support::grant_role_in(&app, "member", bob, "reader", &["shop.order.*.read"]).await;
    let bob = impersonate(&app, &staff, "bob@example.com")
        .await
        .session()
        .unwrap();
    json(&app, "/account/who", &bob).await;

    let mut refunding = sofie_identity::grant(&app.db().read(), reader, "shop.order.*.refund")
        .await
        .unwrap();
    app.db().commit(&mut refunding).await.unwrap();
    // Past the session cache's minute, as nothing a grant writes drops its entry.
    app.clock().advance(Duration::from_mins(2)).unwrap();

    app.get("/account/who")
        .session(&bob)
        .accept_json()
        .await
        .assert_status(401);
    let record = json(&app, "/console/impersonation", &staff).await;
    assert!(
        !record["impersonations"][0]["ended_at"].is_null(),
        "{record}"
    );
    assert_raised::<ImpersonationEnded>(&app);
}

#[tokio::test]
async fn an_impersonation_ends_by_itself_and_never_reaches_staff() {
    let app = started().await;
    let (grace, staff) = support::joined::<Console>(&app, "grace@example.com").await;
    support::joined::<Console>(&app, "linus@example.com").await;
    support::joined::<Member>(&app, "ada@example.com").await;
    grant_impersonation(&app, grace).await;

    let other_staff = impersonate(&app, &staff, "linus@example.com").await;
    assert_eq!(other_staff.status, 422, "{}", other_staff.body);
    let ada = impersonate(&app, &staff, "ada@example.com")
        .await
        .session()
        .unwrap();
    json(&app, "/account/who", &ada).await;
    app.clock().advance(Duration::from_mins(31)).unwrap();

    app.get("/account/who")
        .session(&ada)
        .accept_json()
        .await
        .assert_status(401);
    let record = json(&app, "/console/impersonation", &staff).await;
    let ended = &record["impersonations"][0]["ended_at"];
    assert!(!ended.is_null(), "an expired one reads as ended: {record}");
}

#[tokio::test]
async fn an_impersonation_changes_no_way_of_signing_in_and_closes_nothing() {
    let app = started().await;
    let (grace, staff) = support::joined::<Console>(&app, "grace@example.com").await;
    support::joined::<Member>(&app, "ada@example.com").await;
    grant_impersonation(&app, grace).await;
    let ada = impersonate(&app, &staff, "ada@example.com")
        .await
        .session()
        .unwrap();
    let unknown = sofie::db::Uuid::now_v7().unwrap();
    let changes = [
        ("/account/passkeys".to_owned(), json!({})),
        (format!("/account/passkeys/{unknown}/remove"), json!({})),
        (
            "/account/connections".to_owned(),
            json!({ "provider": "google" }),
        ),
        (format!("/account/connections/{unknown}/remove"), json!({})),
        ("/account/two-factor/start".to_owned(), json!({})),
        (
            "/account/two-factor/confirm".to_owned(),
            json!({ "code": "000000" }),
        ),
        (
            "/account/two-factor/disable".to_owned(),
            json!({ "code": "000000" }),
        ),
        (
            "/account/account-data/close".to_owned(),
            json!({ "email": "ada@example.com" }),
        ),
        ("/account/account-data/export".to_owned(), json!({})),
        ("/account/sessions/others".to_owned(), json!({})),
        (format!("/account/sessions/{unknown}/end"), json!({})),
    ];
    for (path, body) in changes {
        let refused = app
            .post(&path)
            .session(&ada)
            .accept_json()
            .json(&body)
            .await;
        assert_eq!(refused.status, 422, "{path}: {}", refused.body);
        assert!(
            refused.body.contains("impersonation"),
            "{path}: {}",
            refused.body
        );
    }
    json(&app, "/account/who", &ada).await;
}

#[tokio::test]
async fn an_impersonation_never_sees_a_second_factor_being_set_up() {
    let app = started().await;
    let (grace, staff) = support::joined::<Console>(&app, "grace@example.com").await;
    let (_, own) = support::joined::<Member>(&app, "ada@example.com").await;
    grant_impersonation(&app, grace).await;
    app.post("/account/two-factor/start")
        .session(&own)
        .accept_json()
        .await
        .assert_ok();
    let ada = impersonate(&app, &staff, "ada@example.com")
        .await
        .session()
        .unwrap();

    let seen = app
        .get("/account/two-factor")
        .session(&ada)
        .accept_json()
        .await;

    assert_eq!(seen.status, 422, "{}", seen.body);
    assert!(!seen.body.contains("recovery"), "{}", seen.body);
}

#[tokio::test]
async fn a_closed_account_or_a_blank_reason_is_not_impersonated() {
    let app = started().await;
    let (grace, staff) = support::joined::<Console>(&app, "grace@example.com").await;
    let (ada, own) = support::joined::<Member>(&app, "ada@example.com").await;
    grant_impersonation(&app, grace).await;
    let blank = app
        .post("/console/impersonation")
        .session(&staff)
        .accept_json()
        .json(&json!({ "email": "ada@example.com", "reason": "    " }))
        .await;
    app.post("/account/account-data/close")
        .session(&own)
        .accept_json()
        .json(&json!({ "email": "ada@example.com" }))
        .await
        .assert_ok();

    let closed = impersonate(
        &app,
        &staff,
        &format!("closed-{}@closed.invalid", ada.uuid()),
    )
    .await;

    assert_eq!(blank.status, 422, "{}", blank.body);
    assert_eq!(closed.status, 422, "{}", closed.body);
    assert!(closed.session().is_none());
}

#[tokio::test]
async fn signing_out_of_an_impersonation_ends_it_on_the_record() {
    let app = started().await;
    let (grace, staff) = support::joined::<Console>(&app, "grace@example.com").await;
    support::joined::<Member>(&app, "ada@example.com").await;
    grant_impersonation(&app, grace).await;
    let ada = impersonate(&app, &staff, "ada@example.com")
        .await
        .session()
        .unwrap();

    app.post("/account/sign-out")
        .session(&ada)
        .accept_json()
        .await
        .assert_ok();

    let open = app
        .query(
            "SELECT count(*) FROM sofie_identity.impersonation WHERE ended_at IS NULL",
            &[],
        )
        .await;
    assert_eq!(open, [vec![sofie::core::SqlValue::Int(0)]]);
    assert_eq!(assert_raised::<ImpersonationEnded>(&app).len(), 1);
}

#[tokio::test]
async fn a_member_ending_its_other_sessions_ends_an_impersonation_on_the_record() {
    let app = started().await;
    let (grace, staff) = support::joined::<Console>(&app, "grace@example.com").await;
    let (_, own) = support::joined::<Member>(&app, "ada@example.com").await;
    grant_impersonation(&app, grace).await;
    impersonate(&app, &staff, "ada@example.com")
        .await
        .assert_ok();

    app.post("/account/sessions/others")
        .session(&own)
        .accept_json()
        .await
        .assert_ok();

    assert_eq!(assert_raised::<ImpersonationEnded>(&app).len(), 1);
}

#[tokio::test]
async fn an_impersonator_ending_its_other_sessions_ends_the_impersonations_it_started() {
    let app = started().await;
    let (grace, staff) = support::joined::<Console>(&app, "grace@example.com").await;
    support::joined::<Member>(&app, "ada@example.com").await;
    support::grant_role(
        &app,
        grace,
        "support",
        &[
            "sofie_identity.impersonation.*.*",
            "sofie_identity.account_session.*.update",
        ],
    )
    .await;
    let ada = impersonate(&app, &staff, "ada@example.com")
        .await
        .session()
        .unwrap();

    app.post("/console/sessions/others")
        .session(&staff)
        .accept_json()
        .await
        .assert_ok();

    app.get("/account/who")
        .session(&ada)
        .accept_json()
        .await
        .assert_status(401);
    assert_eq!(assert_raised::<ImpersonationEnded>(&app).len(), 1);
}

#[tokio::test]
async fn removing_a_membership_ends_an_impersonation_in_its_area_on_the_record() {
    let app = started().await;
    let (grace, staff) = support::joined::<Console>(&app, "grace@example.com").await;
    let (ada, _) = support::joined::<Member>(&app, "ada@example.com").await;
    grant_impersonation(&app, grace).await;
    impersonate(&app, &staff, "ada@example.com")
        .await
        .assert_ok();

    let mut removed = sofie_identity::remove_membership(
        &app.db().read(),
        ada,
        Member::NAME,
        app.db().clock().now(),
    )
    .await
    .unwrap();
    app.db().commit(&mut removed).await.unwrap();

    assert_eq!(assert_raised::<ImpersonationEnded>(&app).len(), 1);
}

#[tokio::test]
async fn stopping_after_the_impersonators_own_session_ended_only_signs_out() {
    let app = started().await;
    let (grace, staff) = support::joined::<Console>(&app, "grace@example.com").await;
    support::joined::<Member>(&app, "ada@example.com").await;
    grant_impersonation(&app, grace).await;
    let started = impersonate(&app, &staff, "ada@example.com").await;
    started.assert_status(204);
    let ada = started.session().expect("a session as the member");

    app.query(
        "UPDATE sofie_identity.account_session SET ended_at = now() WHERE account = $1",
        &[sofie::core::SqlValue::Uuid(grace.uuid())],
    )
    .await;
    let stopped = app
        .post("/account/impersonation/stop")
        .session(&ada)
        .accept_json()
        .await;
    stopped.assert_status(204);

    assert!(stopped.session().is_none(), "no staff session is opened");
    app.get("/account/who")
        .session(&ada)
        .accept_json()
        .await
        .assert_status(401);
    assert_raised::<ImpersonationEnded>(&app);
}

#[tokio::test]
async fn removing_the_impersonators_membership_ends_the_impersonations_it_started() {
    let app = started().await;
    let (grace, staff) = support::joined::<Console>(&app, "grace@example.com").await;
    support::joined::<Member>(&app, "ada@example.com").await;
    grant_impersonation(&app, grace).await;
    let ada = impersonate(&app, &staff, "ada@example.com")
        .await
        .session()
        .unwrap();

    let mut removed = sofie_identity::remove_membership(
        &app.db().read(),
        grace,
        Console::NAME,
        app.db().clock().now(),
    )
    .await
    .unwrap();
    app.db().commit(&mut removed).await.unwrap();

    app.get("/account/who")
        .session(&ada)
        .accept_json()
        .await
        .assert_status(401);
    assert_eq!(assert_raised::<ImpersonationEnded>(&app).len(), 1);
}

#[tokio::test]
async fn closing_an_impersonated_account_ends_the_impersonation_on_the_record() {
    let app = started().await;
    let (grace, staff) = support::joined::<Console>(&app, "grace@example.com").await;
    let (_, own) = support::joined::<Member>(&app, "ada@example.com").await;
    grant_impersonation(&app, grace).await;
    let ada = impersonate(&app, &staff, "ada@example.com")
        .await
        .session()
        .unwrap();

    app.post("/account/account-data/close")
        .session(&own)
        .accept_json()
        .json(&json!({ "email": "ada@example.com" }))
        .await
        .assert_ok();

    app.get("/account/who")
        .session(&ada)
        .accept_json()
        .await
        .assert_status(401);
    let open = app
        .query(
            "SELECT count(*) FROM sofie_identity.impersonation WHERE ended_at IS NULL",
            &[],
        )
        .await;
    assert_eq!(open, [vec![sofie::core::SqlValue::Int(0)]]);
    assert_eq!(assert_raised::<ImpersonationEnded>(&app).len(), 1);
}

#[tokio::test]
async fn closing_the_impersonators_account_ends_the_impersonations_it_started() {
    let app = started().await;
    let (grace, staff) = support::joined::<Console>(&app, "grace@example.com").await;
    support::grant_role(
        &app,
        grace,
        "support",
        &[
            "sofie_identity.impersonation.*.*",
            "sofie_identity.account.*.update",
        ],
    )
    .await;
    support::joined::<Member>(&app, "ada@example.com").await;
    let ada = impersonate(&app, &staff, "ada@example.com")
        .await
        .session()
        .unwrap();

    app.post("/console/account-data/close")
        .session(&staff)
        .accept_json()
        .json(&json!({ "email": "grace@example.com" }))
        .await
        .assert_ok();

    app.get("/account/who")
        .session(&ada)
        .accept_json()
        .await
        .assert_status(401);
    let kept_devices = app
        .query(
            "SELECT count(*) FROM sofie_identity.account_session \
             WHERE account = $1 AND (device IS NOT NULL OR ended_at IS NULL)",
            &[sofie::core::SqlValue::Uuid(grace.uuid())],
        )
        .await;
    assert_eq!(kept_devices, [vec![sofie::core::SqlValue::Int(0)]]);
    assert_eq!(assert_raised::<ImpersonationEnded>(&app).len(), 1);
}
