//! Scaffolded by `sofie scaffold tests` at 2026-11-27 07:44:08 UTC for module `tracker`; yours from now on. A later run writes new or changed facts to a new file and names the tests of `_FACTS` it supersedes.

const _FACTS: &[(&str, &str)] = &[
    (
        "transitions TaskStatus OPEN>DONE DONE>OPEN",
        "task_status_moves_only_along_its_transitions",
    ),
    ("schema Task", "task_table_matches_its_schema"),
    ("constraint Task.title min 1", "task_title_keeps_min_1"),
    ("constraint Task.title max 120", "task_title_keeps_max_120"),
    (
        "store Task",
        "task_loads_as_saved_and_a_stale_save_conflicts",
    ),
    (
        "route tracker.staff.staff_home needs tracker.home.*.read",
        "staff_home_refuses_a_principal_without_its_permission",
    ),
    (
        "route tracker.staff.staff_home succeeds",
        "staff_home_succeeds",
    ),
    (
        "route tracker.team.list_tasks refuses a caller without a principal",
        "list_tasks_refuses_a_caller_without_a_principal",
    ),
    (
        "route tracker.team.list_tasks succeeds",
        "list_tasks_succeeds",
    ),
    (
        "screen tracker/team/tasks renders",
        "tasks_screen_renders_with_fixtures",
    ),
    (
        "route tracker.team.create_task refuses a caller without a principal",
        "create_task_refuses_a_caller_without_a_principal",
    ),
    (
        "route tracker.team.create_task succeeds",
        "create_task_succeeds",
    ),
    (
        "route tracker.team.complete_task refuses a caller without a principal",
        "complete_task_refuses_a_caller_without_a_principal",
    ),
    (
        "route tracker.team.complete_task hides another owner's task",
        "complete_task_hides_another_owner_s_task",
    ),
    (
        "route tracker.team.complete_task succeeds",
        "complete_task_succeeds",
    ),
    (
        "route tracker.team.reopen_task refuses a caller without a principal",
        "reopen_task_refuses_a_caller_without_a_principal",
    ),
    (
        "route tracker.team.reopen_task hides another owner's task",
        "reopen_task_hides_another_owner_s_task",
    ),
    (
        "route tracker.team.reopen_task succeeds",
        "reopen_task_succeeds",
    ),
    (
        "route tracker.team.reopen_task is not found while tracker.reopening is off",
        "reopen_task_is_not_found_while_its_flag_is_off",
    ),
    (
        "route tracker.team.billing_overview refuses a caller without a principal",
        "billing_overview_refuses_a_caller_without_a_principal",
    ),
    (
        "route tracker.team.billing_overview succeeds",
        "billing_overview_succeeds",
    ),
    (
        "screen tracker/team/billing renders",
        "billing_screen_renders_with_fixtures",
    ),
    (
        "route tracker.team.start_trial refuses a caller without a principal",
        "start_trial_refuses_a_caller_without_a_principal",
    ),
    (
        "route tracker.team.start_trial succeeds",
        "start_trial_succeeds",
    ),
    (
        "route tracker.team.upgrade refuses a caller without a principal",
        "upgrade_refuses_a_caller_without_a_principal",
    ),
    (
        "route tracker.team.upgrade succeeds",
        "upgrade_fails_without_a_payment_provider",
    ),
    (
        "route tracker.team.manage_billing refuses a caller without a principal",
        "manage_billing_refuses_a_caller_without_a_principal",
    ),
    (
        "route tracker.team.manage_billing succeeds",
        "manage_billing_refuses_a_team_that_never_paid",
    ),
    (
        "route tracker.web.list_plans succeeds",
        "list_plans_succeeds",
    ),
    (
        "screen tracker/web/pricing renders",
        "pricing_screen_renders_with_fixtures",
    ),
];

use sofie_testing::{TestApp, test_app};

#[test]
fn task_status_moves_only_along_its_transitions() {
    sofie_testing::assert_moves::<tracker::TaskStatus>(&[("OPEN", &["DONE"]), ("DONE", &["OPEN"])]);
}

#[test_app]
async fn task_table_matches_its_schema(app: TestApp) {
    sofie_testing::assert_schema::<tracker::Task>(&app).await;
}

#[test]
fn task_title_keeps_min_1() {
    sofie_testing::assert_boundary("title", tracker::Task::__TITLE, "min 1");
}

#[test]
fn task_title_keeps_max_120() {
    sofie_testing::assert_boundary("title", tracker::Task::__TITLE, "max 120");
}

#[test_app]
async fn task_loads_as_saved_and_a_stale_save_conflicts(app: TestApp) {
    let member = sofie_testing::TestPrincipal::<tracker_contract::area::Team>::fresh(&app);
    let first = app
        .as_(&member)
        .request(&tracker_contract::routes::team::create_task())
        .json(&sofie_testing::fixture_input::<tracker::actions::NewTask>(
            &app,
        ))
        .await
        .record_id();
    let second = app
        .as_(&member)
        .request(&tracker_contract::routes::team::create_task())
        .json(&sofie_testing::fixture_input::<tracker::actions::NewTask>(
            &app,
        ))
        .await
        .record_id();
    sofie_testing::assert_store::<tracker::Task>(
        &app,
        sofie::db::Id::from_uuid(first),
        sofie::db::Id::from_uuid(second),
    )
    .await;
}

#[test_app]
async fn staff_home_refuses_a_principal_without_its_permission(app: TestApp) {
    let principal = sofie_testing::TestPrincipal::<tracker_contract::area::Staff>::fresh(&app);
    app.as_(&principal)
        .request(&tracker_contract::routes::staff::staff_home())
        .accept_json()
        .await
        .assert_status(403);
}

#[test_app]
async fn staff_home_succeeds(app: TestApp) {
    let member = sofie_identity::testing::granted::<tracker_contract::area::Staff>(
        &app,
        &["tracker.home.*.read"],
    )
    .await;
    app.as_(&member)
        .request(&tracker_contract::routes::staff::staff_home())
        .accept_json()
        .await
        .assert_ok();
}

#[test_app]
async fn list_tasks_refuses_a_caller_without_a_principal(app: TestApp) {
    app.request(&tracker_contract::routes::team::list_tasks())
        .accept_json()
        .await
        .assert_status(401);
}

#[test_app]
async fn list_tasks_succeeds(app: TestApp) {
    let member = sofie_testing::TestPrincipal::<tracker_contract::area::Team>::fresh(&app);
    app.as_(&member)
        .request(&tracker_contract::routes::team::list_tasks())
        .accept_json()
        .await
        .assert_ok();
}

#[test_app]
async fn tasks_screen_renders_with_fixtures(app: TestApp) {
    let member = sofie_testing::TestPrincipal::<tracker_contract::area::Team>::fresh(&app);
    let _shown_0 = app
        .as_(&member)
        .request(&tracker_contract::routes::team::create_task())
        .json(&sofie_testing::fixture_input::<tracker::actions::NewTask>(
            &app,
        ))
        .await
        .record_id();
    let page = app
        .as_(&member)
        .request(&tracker_contract::routes::team::list_tasks())
        .header("accept", "text/html")
        .await;
    page.assert_ok();
    assert!(page.body.starts_with("<!doctype html>"), "{}", page.body);
}

#[test_app]
async fn create_task_refuses_a_caller_without_a_principal(app: TestApp) {
    app.request(&tracker_contract::routes::team::create_task())
        .accept_json()
        .await
        .assert_status(401);
}

#[test_app]
async fn create_task_succeeds(app: TestApp) {
    let member = sofie_testing::TestPrincipal::<tracker_contract::area::Team>::fresh(&app);
    app.as_(&member)
        .request(&tracker_contract::routes::team::create_task())
        .json(&sofie_testing::fixture_input::<tracker::actions::NewTask>(
            &app,
        ))
        .await
        .assert_ok();
}

#[test_app]
async fn complete_task_refuses_a_caller_without_a_principal(app: TestApp) {
    app.request(&tracker_contract::routes::team::complete_task(
        sofie::db::Id::<tracker::Task>::from_uuid(
            "00000000-0000-4000-8000-000000000000".parse().unwrap(),
        ),
    ))
    .accept_json()
    .await
    .assert_status(401);
}

#[test_app]
async fn complete_task_hides_another_owner_s_task(app: TestApp) {
    let member = sofie_testing::TestPrincipal::<tracker_contract::area::Team>::fresh(&app);
    let other = sofie_testing::TestPrincipal::<tracker_contract::area::Team>::fresh(&app);
    let record = app
        .as_(&other)
        .request(&tracker_contract::routes::team::create_task())
        .json(&sofie_testing::fixture_input::<tracker::actions::NewTask>(
            &app,
        ))
        .await
        .record_id();
    app.as_(&member)
        .request(&tracker_contract::routes::team::complete_task(
            sofie::db::Id::<tracker::Task>::from_uuid(record),
        ))
        .accept_json()
        .await
        .assert_status(404);
}

#[test_app]
async fn complete_task_succeeds(app: TestApp) {
    let member = sofie_testing::TestPrincipal::<tracker_contract::area::Team>::fresh(&app);
    let record_0 = app
        .as_(&member)
        .request(&tracker_contract::routes::team::create_task())
        .json(&sofie_testing::fixture_input::<tracker::actions::NewTask>(
            &app,
        ))
        .await
        .record_id();
    app.as_(&member)
        .request(&tracker_contract::routes::team::complete_task(
            sofie::db::Id::<tracker::Task>::from_uuid(record_0),
        ))
        .accept_json()
        .await
        .assert_ok();
}

#[test_app]
async fn reopen_task_refuses_a_caller_without_a_principal(app: TestApp) {
    app.request(&tracker_contract::routes::team::reopen_task(
        sofie::db::Id::<tracker::Task>::from_uuid(
            "00000000-0000-4000-8000-000000000000".parse().unwrap(),
        ),
    ))
    .accept_json()
    .await
    .assert_status(401);
}

#[test_app]
async fn reopen_task_hides_another_owner_s_task(app: TestApp) {
    let member = sofie_testing::TestPrincipal::<tracker_contract::area::Team>::fresh(&app);
    let other = sofie_testing::TestPrincipal::<tracker_contract::area::Team>::fresh(&app);
    let record = app
        .as_(&other)
        .request(&tracker_contract::routes::team::create_task())
        .json(&sofie_testing::fixture_input::<tracker::actions::NewTask>(
            &app,
        ))
        .await
        .record_id();
    app.as_(&member)
        .request(&tracker_contract::routes::team::reopen_task(
            sofie::db::Id::<tracker::Task>::from_uuid(record),
        ))
        .accept_json()
        .await
        .assert_status(404);
}

#[test_app]
async fn reopen_task_succeeds(app: TestApp) {
    let member = sofie_testing::TestPrincipal::<tracker_contract::area::Team>::fresh(&app);
    let record_0 = app
        .as_(&member)
        .request(&tracker_contract::routes::team::create_task())
        .json(&sofie_testing::fixture_input::<tracker::actions::NewTask>(
            &app,
        ))
        .await
        .record_id();
    app.as_(&member)
        .request(&tracker_contract::routes::team::complete_task(
            sofie::db::Id::<tracker::Task>::from_uuid(record_0),
        ))
        .await
        .assert_ok();
    app.as_(&member)
        .request(&tracker_contract::routes::team::reopen_task(
            sofie::db::Id::<tracker::Task>::from_uuid(record_0),
        ))
        .accept_json()
        .await
        .assert_ok();
}

#[test_app]
async fn reopen_task_is_not_found_while_its_flag_is_off(app: TestApp) {
    app.flags()
        .set("tracker.reopening", sofie::flags::Rule::off())
        .await
        .unwrap();
    let member = sofie_testing::TestPrincipal::<tracker_contract::area::Team>::fresh(&app);
    app.as_(&member)
        .request(&tracker_contract::routes::team::reopen_task(
            sofie::db::Id::<tracker::Task>::from_uuid(
                "00000000-0000-4000-8000-000000000000".parse().unwrap(),
            ),
        ))
        .accept_json()
        .await
        .assert_status(404);
}

#[test_app]
async fn billing_overview_refuses_a_caller_without_a_principal(app: TestApp) {
    app.request(&tracker_contract::routes::team::billing_overview())
        .accept_json()
        .await
        .assert_status(401);
}

#[test_app]
async fn billing_overview_succeeds(app: TestApp) {
    let member = sofie_testing::TestPrincipal::<tracker_contract::area::Team>::fresh(&app);
    let shown = app
        .as_(&member)
        .request(&tracker_contract::routes::team::billing_overview())
        .accept_json()
        .await;
    shown.assert_ok();
    let shown = shown.json();
    for field in ["plan", "status", "open_tasks", "open_task_limit"] {
        assert!(shown.get(field).is_some(), "{field} missing from {shown}");
    }
}

#[test_app]
async fn billing_screen_renders_with_fixtures(app: TestApp) {
    let member = sofie_testing::TestPrincipal::<tracker_contract::area::Team>::fresh(&app);
    let _shown_0 = app
        .as_(&member)
        .request(&tracker_contract::routes::team::create_task())
        .json(&sofie_testing::fixture_input::<tracker::actions::NewTask>(
            &app,
        ))
        .await
        .record_id();
    let shown = app
        .as_(&member)
        .request(&tracker_contract::routes::team::billing_overview())
        .accept_json()
        .await
        .json();
    let page = app
        .as_(&member)
        .request(&tracker_contract::routes::team::billing_overview())
        .header("accept", "text/html")
        .await;
    page.assert_ok();
    assert!(page.body.starts_with("<!doctype html>"), "{}", page.body);
    for field in ["plan", "open_task_limit"] {
        let value = shown[field]
            .as_str()
            .unwrap_or_else(|| panic!("{field} is not text in {shown}"));
        let mut written = String::new();
        sofie::engine::push_escaped(&mut written, value);
        assert!(
            page.body.contains(&written),
            "{field} `{written}` missing from {}",
            page.body
        );
    }
}

#[test_app]
async fn start_trial_refuses_a_caller_without_a_principal(app: TestApp) {
    app.request(&tracker_contract::routes::team::start_trial())
        .accept_json()
        .await
        .assert_status(401);
}

#[test_app]
async fn start_trial_succeeds(app: TestApp) {
    let member = sofie_testing::TestPrincipal::<tracker_contract::area::Team>::fresh(&app);
    app.as_(&member)
        .request(&tracker_contract::routes::team::start_trial())
        .accept_json()
        .await
        .assert_ok();
}

#[test_app]
async fn upgrade_refuses_a_caller_without_a_principal(app: TestApp) {
    app.request(&tracker_contract::routes::team::upgrade())
        .accept_json()
        .await
        .assert_status(401);
}

#[test_app]
async fn upgrade_fails_without_a_payment_provider(app: TestApp) {
    let member = sofie_testing::TestPrincipal::<tracker_contract::area::Team>::fresh(&app);
    app.as_(&member)
        .request(&tracker_contract::routes::team::upgrade())
        .accept_json()
        .await
        .assert_status(500);
}

#[test_app]
async fn manage_billing_refuses_a_caller_without_a_principal(app: TestApp) {
    app.request(&tracker_contract::routes::team::manage_billing())
        .accept_json()
        .await
        .assert_status(401);
}

#[test_app]
async fn manage_billing_refuses_a_team_that_never_paid(app: TestApp) {
    let member = sofie_testing::TestPrincipal::<tracker_contract::area::Team>::fresh(&app);
    app.as_(&member)
        .request(&tracker_contract::routes::team::manage_billing())
        .accept_json()
        .await
        .assert_status(422);
}

#[test_app]
async fn list_plans_succeeds(app: TestApp) {
    app.request(&tracker_contract::routes::web::list_plans())
        .accept_json()
        .await
        .assert_ok();
}

#[test_app]
async fn pricing_screen_renders_with_fixtures(app: TestApp) {
    let page = app
        .request(&tracker_contract::routes::web::list_plans())
        .header("accept", "text/html")
        .await;
    page.assert_ok();
    assert!(page.body.starts_with("<!doctype html>"), "{}", page.body);
}
