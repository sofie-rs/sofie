use std::sync::{Arc, Mutex};

use sofie::db::BoxFuture;
use sofie::prelude::*;
use sofie::serde_json::{Value, json};
use sofie_testing::{TestApp, TestResponse, TestSession, test_app};
use sofie_webhooks::WebhookSender;
use tracker_contract::area::{Member, Staff};

const PASSWORD: &str = "correct horse battery";

#[derive(Default)]
struct Receiver {
    bodies: Mutex<Vec<String>>,
}

impl WebhookSender for Receiver {
    fn post<'a>(
        &'a self,
        _url: &'a str,
        _headers: &'a [(&'static str, String)],
        body: &'a str,
    ) -> BoxFuture<'a, Result<u16>> {
        self.bodies.lock().unwrap().push(body.to_owned());
        Box::pin(std::future::ready(Ok(204)))
    }
}

async fn post(app: &TestApp, path: &str, session: &TestSession, body: &Value) -> TestResponse {
    app.post(path)
        .session(session)
        .accept_json()
        .json(body)
        .await
}

async fn founded_team(app: &TestApp, email: &str) -> TestSession {
    let member = sofie_identity::testing::signed_up::<Member>(app, email, PASSWORD).await;
    post(
        app,
        "/member/organizations",
        &member,
        &json!({ "name": "Acme" }),
    )
    .await
    .assert_ok();
    let listed = app
        .get("/member/organizations")
        .session(&member)
        .accept_json()
        .await
        .json();
    let team = listed["organizations"][0]["id"].clone();
    let entered = post(
        app,
        "/member/organizations/enter",
        &member,
        &json!({ "organization": team }),
    )
    .await;
    entered.assert_ok();
    entered.session().expect("entering signs in to the team")
}

async fn add_task(app: &TestApp, team: &TestSession, title: &str) -> TestResponse {
    post(app, "/team/tasks", team, &json!({ "title": title })).await
}

#[test_app]
async fn a_trial_lifts_the_free_plan_s_limit_and_a_finished_task_is_announced(app: TestApp) {
    let receiver = Arc::new(Receiver::default());
    sofie::db::provide::<dyn WebhookSender>(
        app.db(),
        Arc::clone(&receiver) as Arc<dyn WebhookSender>,
    );
    let team = founded_team(&app, "ada@example.com").await;
    for title in ["Write the brief", "Book the venue", "Send invitations"] {
        add_task(&app, &team, title).await.assert_ok();
    }

    let over_limit = add_task(&app, &team, "Order catering").await;
    assert_eq!(over_limit.status, 422, "{}", over_limit.body);
    post(&app, "/team/billing/trial", &team, &json!({}))
        .await
        .assert_ok();
    let catering = add_task(&app, &team, "Order catering").await.record_id();
    let billing = app.get("/team/billing").session(&team).accept_json().await;
    assert_eq!(billing.json()["plan"], "pro", "{}", billing.body);

    post(
        &app,
        "/team/webhooks",
        &team,
        &json!({ "url": "https://hooks.example.com/tasks", "events": ["task.completed"] }),
    )
    .await
    .assert_ok();
    post(
        &app,
        &format!("/team/tasks/{catering}/complete"),
        &team,
        &json!({}),
    )
    .await
    .assert_ok();
    app.drain().await;

    let bodies = receiver.bodies.lock().unwrap().clone();
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    let announced: Value = sofie::serde_json::from_str(&bodies[0]).unwrap();
    assert_eq!(announced["type"], "task.completed");
    assert_eq!(announced["data"]["title"], "Order catering");
}

#[test_app]
async fn staff_signs_in_as_a_member_and_back(app: TestApp) {
    sofie_identity::testing::signed_up::<Member>(&app, "ada@example.com", PASSWORD).await;
    let staff =
        sofie_identity::testing::granted::<Staff>(&app, &["sofie_identity.impersonation.*.*"])
            .await;

    let started = app
        .as_(&staff)
        .post("/staff/impersonation")
        .accept_json()
        .json(&json!({ "email": "ada@example.com", "reason": "ticket 7: tasks missing" }))
        .await;

    started.assert_status(204);
    let ada = started.session().expect("a session as the member");
    assert!(app.principal_of::<Member>(&ada).is_some());
}
