#![cfg(feature = "permitted-flags")]

use sofie::db::Db;
use std::time::Duration;

use serde_json::{Value, json};
use sofie::flags::{Feature, Flags, SettingValue, Subject};
use sofie::http::{Gates, Method, RoutePath};
use sofie::prelude::*;
use sofie_identity::{Memberships, create_role, permitted_routes};
use sofie_testing::{TestApp, TestSession};
use support::Console;

mod support;

const BETA: Feature = Feature::new("shop.beta");

const SHOP: sofie::Module = sofie::Module {
    tier: sofie::Tier::Core,
    settings: &[sofie::Setting {
        name: "free_shipping_above",
        default: sofie::SettingValue::Number(5000),
    }],
    ..sofie::Module::new("shop")
};

async fn started() -> TestApp {
    sofie_testing::start(
        |db: Db| async move {
            db.schema().migrate(&[sofie_identity::migrations()]).await?;
            let mut screens = sofie::http::Screens::default();
            screens.add_builtin(sofie_identity::module(&[]).screens)?;
            let flags = Flags::start(&db, "test").await?;
            sofie::declare_settings(&flags, &[SHOP])?;
            Ok(App::new(db)
                .screens(screens)
                .membership(Memberships)
                .gates(Gates::default().features(flags))
                .area::<Web>("sofie_identity", sofie_identity::routes::<Console>)
                .area::<Console>("sofie_identity", permitted_routes::<Console>)
                .menu::<Console>(|menu| {
                    menu.entry(
                        "Roles",
                        RoutePath::new(Method::GET, "/console/roles".to_owned()),
                    );
                    menu.entry(
                        "Flags",
                        RoutePath::new(Method::GET, "/console/flags".to_owned()),
                    )
                    .feature(BETA);
                }))
        },
        sofie::worker::Worker::new,
    )
    .await
}

async fn console(app: &TestApp, email: &str, permissions: &[&str]) -> TestSession {
    let (account, session) = support::joined::<Console>(app, email).await;
    support::grant_role(app, account, email, permissions).await;
    session
}

fn items(list: &Value) -> Vec<String> {
    list["list"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["name"].as_str().unwrap_or_default().to_owned())
        .collect()
}

#[tokio::test]
async fn roles_are_listed_created_changed_and_shown_without_a_template() {
    let app = started().await;
    let session = console(&app, "ada@example.com", &["sofie_identity.*.*.*"]).await;

    let created = app
        .post("/console/roles/new")
        .session(&session)
        .form(&[("name", "support")])
        .await;
    assert_eq!(created.status, 303, "{}", created.body);
    let duplicate = app
        .post("/console/roles/new")
        .session(&session)
        .json(&json!({ "name": "support" }))
        .await;
    assert_eq!(duplicate.status, 422, "{}", duplicate.body);
    assert!(
        duplicate.body.contains("is already taken"),
        "{}",
        duplicate.body
    );

    let page = app.get("/console/roles?sort=-name").session(&session).await;
    assert_eq!(page.status, 200, "{}", page.body);
    assert!(page.body.contains("<td>support</td>"), "{}", page.body);
    assert!(page.body.contains(">New role</a>"), "{}", page.body);
    assert!(
        page.body
            .contains("<a href=\"/console/roles\" aria-current=\"page\">Roles</a>"),
        "{}",
        page.body
    );
    assert!(
        !page.body.contains(">Flags</a>"),
        "a menu entry behind an off flag is hidden"
    );

    let listed = app
        .get("/console/roles?sort=name&name.contains=sup")
        .session(&session)
        .accept_json()
        .await
        .json();
    assert_eq!(items(&listed), ["support"]);
    let id = listed["list"]["items"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let edited = app
        .post(&format!("/console/roles/{id}/edit"))
        .session(&session)
        .form(&[("name", "help desk")])
        .await;
    assert_eq!(edited.status, 303, "{}", edited.body);
    let shown = app
        .get(&format!("/console/roles/{id}"))
        .session(&session)
        .await;
    assert!(shown.body.contains("<dd>help desk</dd>"), "{}", shown.body);
    let refused = app
        .get("/console/roles?sort=secret")
        .session(&session)
        .accept_json()
        .await;
    assert_eq!(refused.status, 400, "{}", refused.body);
}

#[tokio::test]
async fn a_grant_is_reached_only_under_its_own_role() {
    let app = started().await;
    let session = console(
        &app,
        "ada@example.com",
        &["sofie_identity.*.*.*", "shop.order.*.read"],
    )
    .await;
    let read = app.db().read();
    let mut support = create_role(&read, "support").await.unwrap();
    app.db().commit(&mut support).await.unwrap();
    let mut sales = create_role(&read, "sales").await.unwrap();
    app.db().commit(&mut sales).await.unwrap();
    let (support, sales) = (support.id(), sales.id());

    let granted = app
        .post(&format!("/console/roles/{support}/role-grants/new"))
        .session(&session)
        .json(&json!({ "role": sales.to_string(), "permission": "shop.order.*.read" }))
        .await;
    assert_eq!(granted.status, 200, "{}", granted.body);
    let grant_id = granted.json()["id"].as_str().unwrap().to_owned();
    let again = app
        .post(&format!("/console/roles/{support}/role-grants/new"))
        .session(&session)
        .json(&json!({ "role": support.to_string(), "permission": "shop.order.*.read" }))
        .await;
    assert_eq!(again.status, 422, "{}", again.body);
    assert!(again.body.contains("is already taken"), "{}", again.body);
    let refused = app
        .post(&format!("/console/roles/{support}/role-grants/new"))
        .session(&session)
        .json(&json!({ "role": support.to_string(), "permission": "shop.order" }))
        .await;
    assert_eq!(refused.status, 422, "{}", refused.body);

    let list = |role| {
        let path = format!("/console/roles/{role}/role-grants");
        let request = app.get(&path).session(&session).accept_json();
        async move {
            request.await.json()["list"]["items"]
                .as_array()
                .unwrap()
                .len()
        }
    };
    assert_eq!(
        list(support).await,
        1,
        "the path's role wins over the posted one"
    );
    assert_eq!(list(sales).await, 0);

    let elsewhere = app
        .post(&format!(
            "/console/roles/{sales}/role-grants/{grant_id}/delete"
        ))
        .session(&session)
        .accept_json()
        .await;
    assert_eq!(elsewhere.status, 404, "{}", elsewhere.body);
    let deleted = app
        .post(&format!(
            "/console/roles/{support}/role-grants/{grant_id}/delete"
        ))
        .session(&session)
        .accept_json()
        .await;
    assert_eq!(deleted.status, 200, "{}", deleted.body);
    assert_eq!(list(support).await, 0);
}

#[tokio::test]
async fn a_principal_grants_no_permission_its_own_grants_do_not_cover() {
    let app = started().await;
    let held = [
        "sofie_identity.role_grant.*.create",
        "sofie_identity.role.*.read",
        "shop.order.*.read",
    ];
    let (account, session) = support::joined::<Console>(&app, "eve@example.com").await;
    let own = support::grant_role(&app, account, "support", &held).await;
    let grant = |permission: &'static str| {
        app.post(&format!("/console/roles/{own}/role-grants/new"))
            .session(&session)
            .json(&json!({ "role": own.to_string(), "permission": permission }))
    };

    let everything = grant("*.*.*.*").await;
    let impersonating = grant("sofie_identity.impersonation.*.create").await;
    let wider = grant("shop.order.*.*").await;
    let covered = grant("shop.order.*.read").await;

    for refused in [&everything, &impersonating, &wider] {
        assert_eq!(refused.status, 422, "{}", refused.body);
    }
    assert_eq!(covered.status, 422, "held already: {}", covered.body);
    assert!(
        covered.body.contains("is already taken"),
        "{}",
        covered.body
    );
}

#[tokio::test]
async fn a_console_principal_sees_only_the_links_and_entries_it_may_follow() {
    let app = started().await;
    let reader = console(&app, "ada@example.com", &["sofie_identity.*.*.read"]).await;

    let page = app.get("/console/roles").session(&reader).await;
    assert_eq!(page.status, 200, "{}", page.body);
    assert!(!page.body.contains("New role"), "{}", page.body);
    assert!(!page.body.contains("/edit"), "{}", page.body);
    let created = app
        .post("/console/roles/new")
        .session(&reader)
        .json(&json!({ "name": "support" }))
        .await;
    assert_eq!(created.status, 403, "{}", created.body);
}

#[tokio::test]
async fn a_console_principal_switches_a_flag_on_its_screen() {
    let app = started().await;
    let session = console(&app, "ada@example.com", &["sofie_identity.*.*.*"]).await;

    let switched = app
        .post("/console/flags")
        .session(&session)
        .json(&json!({ "flag": "shop.beta", "on": true }))
        .await;
    assert_eq!(switched.status, 204, "{}", switched.body);
    let page = app.get("/console/flags").session(&session).await;
    assert!(
        page.body.contains("<td>shop.beta</td><td>true</td>"),
        "{}",
        page.body
    );
    assert!(
        page.body.contains(">Flags</a>"),
        "the flag's entry shows once it is on"
    );
    let refused = app
        .post("/console/flags")
        .session(&session)
        .json(&json!({ "flag": "beta", "on": true }))
        .await;
    assert_eq!(refused.status, 422, "{}", refused.body);
}

#[tokio::test]
async fn a_flag_is_targeted_on_its_screen_and_evaluated_so() {
    let app = started().await;
    let session = console(&app, "ada@example.com", &["sofie_identity.*.*.*"]).await;
    let chosen = sofie::db::Uuid::from_u128(7);

    let targeted = app
        .post("/console/flags")
        .session(&session)
        .json(&json!({
            "flag": "shop.beta",
            "on": true,
            "environments": "test",
            "areas": "console, member",
            "principals": chosen.to_string(),
            "percentage": 0,
            "variants": "CLASSIC:1, COMPACT:3",
        }))
        .await;
    assert_eq!(targeted.status, 204, "{}", targeted.body);

    let page = app.get("/console/flags").session(&session).await;
    assert!(
        page.body.contains("<td>console, member</td>"),
        "{}",
        page.body
    );
    assert!(
        page.body.contains("<td>CLASSIC:1, COMPACT:3</td>"),
        "{}",
        page.body
    );
    let flags = Flags::start(app.db(), "test").await.unwrap();
    let in_console = |principal| Subject {
        area: Some("console"),
        principal: Some(principal),
    };
    assert!(flags.evaluate(in_console(chosen)).enabled(BETA));
    assert!(
        !flags
            .evaluate(in_console(sofie::db::Uuid::from_u128(8)))
            .enabled(BETA)
    );

    for (field, malformed) in [
        ("percentage", json!(101)),
        ("principals", json!("someone")),
        ("variants", json!("COMPACT")),
        ("areas", json!("con sole")),
    ] {
        let mut posted = json!({ "flag": "shop.beta", "on": true });
        posted[field] = malformed;
        let refused = app
            .post("/console/flags")
            .session(&session)
            .json(&posted)
            .await;
        assert_eq!(refused.status, 422, "{field}: {}", refused.body);
        assert!(refused.body.contains(field), "{field}: {}", refused.body);
    }
}

#[tokio::test]
async fn a_setting_changed_on_its_screen_reaches_another_instance() {
    let app = started().await;
    let session = console(&app, "ada@example.com", &["sofie_identity.*.*.*"]).await;
    let elsewhere = Flags::start(app.db(), "test").await.unwrap();
    let held = || {
        elsewhere
            .evaluate(Subject::default())
            .setting("shop.free_shipping_above", SettingValue::Number(5000))
    };
    assert_eq!(held(), SettingValue::Number(5000));

    let page = app.get("/console/settings").session(&session).await;
    assert!(
        page.body
            .contains("<td>shop.free_shipping_above</td><td>5000</td><td>5000</td>"),
        "{}",
        page.body
    );
    let changed = app
        .post("/console/settings")
        .session(&session)
        .json(&json!({ "setting": "shop.free_shipping_above", "value": "7500" }))
        .await;
    assert_eq!(changed.status, 204, "{}", changed.body);
    let refused = app
        .post("/console/settings")
        .session(&session)
        .json(&json!({ "setting": "shop.free_shipping_above", "value": "lots" }))
        .await;
    assert_eq!(refused.status, 422, "{}", refused.body);

    for _ in 0..50 {
        if held() == SettingValue::Number(7500) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the change did not reach the other instance within five seconds");
}

#[tokio::test]
async fn staff_without_the_permission_cannot_target_flags_or_change_settings() {
    let app = started().await;
    let reader = console(&app, "ada@example.com", &["sofie_identity.*.*.read"]).await;

    let page = app.get("/console/settings").session(&reader).await;
    assert_eq!(page.status, 200, "{}", page.body);
    let targeted = app
        .post("/console/flags")
        .session(&reader)
        .json(&json!({ "flag": "shop.beta", "on": true }))
        .await;
    assert_eq!(targeted.status, 403, "{}", targeted.body);
    let changed = app
        .post("/console/settings")
        .session(&reader)
        .json(&json!({ "setting": "shop.free_shipping_above", "value": "1" }))
        .await;
    assert_eq!(changed.status, 403, "{}", changed.body);

    let stranger = console(&app, "bob@example.com", &["shop.order.*.read"]).await;
    let hidden = app.get("/console/settings").session(&stranger).await;
    assert_eq!(hidden.status, 403, "{}", hidden.body);
}
