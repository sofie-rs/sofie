use serde_json::{Value, json};
use sofie::billing::{Plan, Plans};
use sofie::db::Db;
use sofie::prelude::*;
use sofie_identity::{Account, MemberArea, Memberships};
use sofie_tenancy::{
    MemberJoined, MemberLeft, MemberRoleChanged, Organization, OrganizationArea,
    OrganizationDeleted, OrganizationRole,
};
use sofie_testing::{TestApp, TestResponse, TestSession, assert_raised, sent_mails};

struct Member;

impl Area for Member {
    const NAME: &'static str = "member";
    const PREFIX: &'static str = "/account";
    const SIGN_IN: &'static str = "/sign-in";
    type Principal = Account;
}

impl MemberArea for Member {
    const HOME: &'static str = "/account/organizations";
    const SIGN_UP: Option<&'static str> = Some("/sign-up");
}

struct Workspace;

impl Area for Workspace {
    const NAME: &'static str = "workspace";
    const PREFIX: &'static str = "/org";
    const SIGN_IN: &'static str = "/account/organizations";
    type Principal = Organization;
}

impl OrganizationArea for Workspace {
    const HOME: &'static str = "/org/members";
}

/// sofie-testing's fake mailer links to it.
const BASE_URL: &str = "http://localhost";

/// The dev-dependency on this crate turns `billing` on, so an app's plans are installed.
const FREE: Plans = Plans::new(Plan::new("free"), &[]);

async fn started() -> TestApp {
    started_with(&FREE).await
}

async fn started_with(plans: &'static Plans) -> TestApp {
    sofie_testing::start(
        move |db: Db| async move {
            db.schema()
                .migrate(&[
                    sofie_identity::migrations(),
                    sofie::billing::migrations(),
                    sofie_tenancy::migrations(),
                ])
                .await?;
            sofie::billing::install(&db, plans);
            sofie_identity::account_data::register(
                &db,
                std::sync::Arc::new(sofie_tenancy::OrganizationMemberships::new(&db)),
            );
            let mut screens = sofie::http::Screens::default();
            screens.add_builtin(sofie_identity::module(&[]).screens)?;
            Ok(App::new(db)
                .screens(screens)
                .membership(Memberships)
                .area::<Web>("sofie_identity", sofie_identity::routes::<Member>)
                .area::<Member>(
                    "sofie_tenancy",
                    sofie_tenancy::member_routes::<Member, Workspace>,
                )
                .area::<Member>(
                    "sofie_identity",
                    sofie_identity::account_data::routes::<Member>,
                )
                .area::<Workspace>(
                    "sofie_tenancy",
                    sofie_tenancy::organization_routes::<Workspace, Member>,
                ))
        },
        |db| sofie_tenancy::worker(sofie::worker::Worker::new(db)),
    )
    .await
}

async fn signed_up(app: &TestApp, email: &str) -> TestSession {
    let credentials = json!({ "email": email, "password": "correct horse battery" });
    app.post("/sign-up").json(&credentials).await.assert_ok();
    let answer = app.post("/sign-in").json(&credentials).await;
    answer.session().unwrap()
}

async fn post(app: &TestApp, path: &str, session: &TestSession, body: &Value) -> TestResponse {
    app.post(path)
        .session(session)
        .accept_json()
        .json(body)
        .await
}

async fn get(app: &TestApp, path: &str, session: &TestSession) -> Value {
    let answer = app.get(path).session(session).accept_json().await;
    answer.assert_ok();
    answer.json()
}

async fn entered(app: &TestApp, session: &TestSession, organization: &Value) -> TestSession {
    let answer = post(
        app,
        "/account/organizations/enter",
        session,
        &json!({ "organization": organization }),
    )
    .await;
    answer.assert_ok();
    answer
        .session()
        .expect("entering signs in to the organization")
}

async fn invitation_link(app: &TestApp) -> String {
    app.drain().await;
    let mails = sent_mails(app);
    let text = mails.last().unwrap().body().text().to_owned();
    let start = text.find(BASE_URL).unwrap() + BASE_URL.len();
    let end = start + text[start..].find(')').unwrap();
    text[start..end].to_owned()
}

async fn founded(app: &TestApp) -> (TestSession, Value) {
    let ada = signed_up(app, "ada@example.com").await;
    post(
        app,
        "/account/organizations",
        &ada,
        &json!({ "name": "Acme" }),
    )
    .await
    .assert_ok();
    let organizations = get(app, "/account/organizations", &ada).await;
    assert_eq!(organizations["organizations"][0]["name"], "Acme");
    let acme = organizations["organizations"][0]["id"].clone();
    (entered(app, &ada, &acme).await, acme)
}

async fn invite(app: &TestApp, session: &TestSession, email: &str) -> TestResponse {
    let invitation = json!({ "email": email, "role": "MEMBER" });
    post(app, "/org/members/invite", session, &invitation).await
}

async fn accept(app: &TestApp, link: &str, session: &TestSession) -> TestResponse {
    let page = get(app, link, session).await;
    assert_eq!(page["invitation"]["organization"], "Acme");
    let action = page["invitation"]["action"].as_str().unwrap();
    post(app, action, session, &json!({})).await
}

#[tokio::test]
async fn a_founder_invites_a_member_who_joins_by_the_mailed_link() {
    let app = started().await;
    let (ada_in_acme, acme) = founded(&app).await;
    let principal = app.principal_of::<Workspace>(&ada_in_acme).unwrap();
    assert_eq!(principal.id().uuid().to_string(), acme.as_str().unwrap());

    invite(&app, &ada_in_acme, "grace@example.com")
        .await
        .assert_ok();
    let link = invitation_link(&app).await;
    assert!(
        link.starts_with("/account/invitations?invitation="),
        "{link}"
    );
    let twice = invite(&app, &ada_in_acme, "grace@example.com").await;
    assert_eq!(twice.status, 422, "{}", twice.body);

    let eve = signed_up(&app, "eve@example.com").await;
    let forwarded = accept(&app, &link, &eve).await;
    assert_eq!(forwarded.status, 422, "{}", forwarded.body);

    let grace = signed_up(&app, "grace@example.com").await;
    accept(&app, &link, &grace).await.assert_ok();
    let joined = get(&app, "/account/organizations", &grace).await;
    assert_eq!(joined["organizations"][0]["name"], "Acme");
    let members = get(&app, "/org/members", &ada_in_acme).await;
    assert_eq!(members["members"].as_array().unwrap().len(), 2, "{members}");
    assert!(get(&app, &link, &grace).await["invitation"].is_null());
}

#[tokio::test]
async fn roles_hold_who_manages_members_and_an_organization_keeps_an_owner() {
    let app = started().await;
    let (ada_in_acme, acme) = founded(&app).await;
    invite(&app, &ada_in_acme, "grace@example.com")
        .await
        .assert_ok();
    let link = invitation_link(&app).await;
    let grace = signed_up(&app, "grace@example.com").await;
    accept(&app, &link, &grace).await.assert_ok();
    let grace_in_acme = entered(&app, &grace, &acme).await;

    let refused = invite(&app, &grace_in_acme, "bob@example.com").await;
    assert_eq!(refused.status, 422, "{}", refused.body);
    let members = get(&app, "/org/members", &ada_in_acme).await;
    let id_of = |email: &str| {
        members["members"]
            .as_array()
            .unwrap()
            .iter()
            .find(|member| member["email"] == email)
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let ada_id = id_of("ada@example.com");
    let last_owner = post(
        &app,
        &format!("/org/members/{ada_id}/remove"),
        &ada_in_acme,
        &json!({}),
    )
    .await;
    assert_eq!(last_owner.status, 422, "{}", last_owner.body);

    let grace_id = id_of("grace@example.com");
    post(
        &app,
        &format!("/org/members/{grace_id}/remove"),
        &ada_in_acme,
        &json!({}),
    )
    .await
    .assert_ok();
    let gone = app
        .get("/org/members")
        .session(&grace_in_acme)
        .accept_json()
        .await;
    assert_eq!(
        gone.status, 401,
        "a removed member's session in the organization ended: {}",
        gone.body
    );
    let still = app
        .get("/account/organizations")
        .session(&grace)
        .accept_json()
        .await;
    assert_eq!(
        still.status, 200,
        "its session in the member area lives: {}",
        still.body
    );
}

#[tokio::test]
async fn a_removal_ends_the_member_s_sessions_for_that_organization_only() {
    let app = started().await;
    let (ada_in_acme, acme) = founded(&app).await;
    invite(&app, &ada_in_acme, "grace@example.com")
        .await
        .assert_ok();
    let link = invitation_link(&app).await;
    let grace = signed_up(&app, "grace@example.com").await;
    accept(&app, &link, &grace).await.assert_ok();
    let grace_in_acme = entered(&app, &grace, &acme).await;
    post(
        &app,
        "/account/organizations",
        &grace,
        &json!({ "name": "Globex" }),
    )
    .await
    .assert_ok();
    let organizations = get(&app, "/account/organizations", &grace).await;
    let globex = organizations["organizations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|organization| organization["name"] == "Globex")
        .unwrap()["id"]
        .clone();
    let grace_in_globex = entered(&app, &grace, &globex).await;
    let grace_id = own_member_id(&app, &ada_in_acme, "grace@example.com").await;

    post(
        &app,
        &format!("/org/members/{grace_id}/remove"),
        &ada_in_acme,
        &json!({}),
    )
    .await
    .assert_ok();

    let status = |session| {
        let app = &app;
        async move {
            app.get("/org/members")
                .session(session)
                .accept_json()
                .await
                .status
        }
    };
    assert_eq!(status(&grace_in_acme).await, 401);
    assert_eq!(status(&grace_in_globex).await, 200);
}

#[tokio::test]
async fn members_and_pending_invitations_hold_to_the_plan_s_seats() {
    const PLANS: Plans = Plans::new(Plan::new("team").seats(2), &[]);
    let app = started_with(&PLANS).await;
    let (ada_in_acme, _) = founded(&app).await;

    invite(&app, &ada_in_acme, "grace@example.com")
        .await
        .assert_ok();
    let full = invite(&app, &ada_in_acme, "bob@example.com").await;

    assert_eq!(
        full.status, 422,
        "the owner and one invitation fill two seats: {}",
        full.body
    );
}

#[tokio::test]
async fn a_withdrawn_invitation_frees_its_seat_and_its_link_opens_nothing() {
    const PLANS: Plans = Plans::new(Plan::new("team").seats(2), &[]);
    let app = started_with(&PLANS).await;
    let (ada_in_acme, _) = founded(&app).await;
    invite(&app, &ada_in_acme, "grace@example.com")
        .await
        .assert_ok();
    let link = invitation_link(&app).await;
    let members = get(&app, "/org/members", &ada_in_acme).await;
    let id = members["invitations"][0]["id"].as_str().unwrap().to_owned();

    post(
        &app,
        &format!("/org/members/invitations/{id}/withdraw"),
        &ada_in_acme,
        &json!({}),
    )
    .await
    .assert_ok();

    let members = get(&app, "/org/members", &ada_in_acme).await;
    assert_eq!(members["invitations"], json!([]), "{members}");
    invite(&app, &ada_in_acme, "bob@example.com")
        .await
        .assert_ok();
    let grace = signed_up(&app, "grace@example.com").await;
    assert!(get(&app, &link, &grace).await["invitation"].is_null());
}

async fn count(app: &TestApp, sql: &str) -> i64 {
    let rows = app.query(sql, &[]).await;
    match rows[0][0] {
        sofie::core::SqlValue::Int(count) => count,
        ref other => panic!("a count is an integer, not {other:?}"),
    }
}

/// Ada founds Acme and Grace joins it as a second owner.
async fn two_owners(app: &TestApp) -> (TestSession, TestSession) {
    let (ada_in_acme, acme) = founded(app).await;
    let owner = json!({ "email": "grace@example.com", "role": "OWNER" });
    post(app, "/org/members/invite", &ada_in_acme, &owner)
        .await
        .assert_ok();
    let link = invitation_link(app).await;
    let grace = signed_up(app, "grace@example.com").await;
    accept(app, &link, &grace).await.assert_ok();
    (ada_in_acme, entered(app, &grace, &acme).await)
}

async fn own_member_id(app: &TestApp, session: &TestSession, email: &str) -> String {
    let members = get(app, "/org/members", session).await;
    members["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|member| member["email"] == email)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn two_owners_leaving_at_once_leave_one_behind() {
    let app = started().await;
    let (ada, grace) = two_owners(&app).await;
    let ada_id = own_member_id(&app, &ada, "ada@example.com").await;
    let grace_id = own_member_id(&app, &grace, "grace@example.com").await;

    let ada_path = format!("/org/members/{ada_id}/remove");
    let grace_path = format!("/org/members/{grace_id}/remove");
    let empty = json!({});
    let (ada_left, grace_left) = tokio::join!(
        post(&app, &ada_path, &ada, &empty),
        post(&app, &grace_path, &grace, &empty),
    );

    let owners = count(
        &app,
        "SELECT count(*) FROM sofie_tenancy.organization_member \
         WHERE role = 'OWNER' AND deleted_at IS NULL",
    )
    .await;
    assert_eq!(owners, 1, "{} / {}", ada_left.body, grace_left.body);
}

#[tokio::test]
async fn joining_a_role_change_and_leaving_are_raised() {
    let app = started().await;
    let (ada, _) = two_owners(&app).await;
    let grace_id = own_member_id(&app, &ada, "grace@example.com").await;

    for step in ["admin", "admin", "remove"] {
        post(
            &app,
            &format!("/org/members/{grace_id}/{step}"),
            &ada,
            &json!({}),
        )
        .await
        .assert_ok();
    }

    let joined = assert_raised::<MemberJoined>(&app);
    let changed = assert_raised::<MemberRoleChanged>(&app);
    let left = assert_raised::<MemberLeft>(&app);
    assert_eq!(
        joined.iter().map(|member| member.role).collect::<Vec<_>>(),
        [OrganizationRole::Owner, OrganizationRole::Owner],
        "the founder, then the invited owner"
    );
    assert_eq!(changed.len(), 1, "giving the role it holds changes nothing");
    assert_eq!(changed[0].role, OrganizationRole::Admin);
    assert_eq!(left.len(), 1);
    assert_eq!(
        (left[0].organization, left[0].account),
        (changed[0].organization, changed[0].account)
    );
    assert_eq!(joined[1].account, left[0].account);
}

#[tokio::test]
async fn two_invitations_at_once_take_no_more_than_the_free_seats() {
    const PLANS: Plans = Plans::new(Plan::new("team").seats(2), &[]);
    let app = started_with(&PLANS).await;
    let (ada_in_acme, _) = founded(&app).await;

    tokio::join!(
        invite(&app, &ada_in_acme, "grace@example.com"),
        invite(&app, &ada_in_acme, "bob@example.com"),
    );

    let pending = count(
        &app,
        "SELECT count(*) FROM sofie_tenancy.invitation WHERE accepted_at IS NULL",
    )
    .await;
    assert_eq!(pending, 1);
}

#[tokio::test]
async fn one_address_invited_twice_at_once_is_invited_once() {
    let app = started().await;
    let (ada_in_acme, _) = founded(&app).await;

    for round in 0..5 {
        let email = format!("grace{round}@example.com");
        let (first, second) = tokio::join!(
            invite(&app, &ada_in_acme, &email),
            invite(&app, &ada_in_acme, &email),
        );
        assert!(
            first.status.as_u16() < 500 && second.status.as_u16() < 500,
            "{} {}",
            first.body,
            second.body
        );
    }

    let pending = count(
        &app,
        "SELECT count(*) FROM sofie_tenancy.invitation WHERE accepted_at IS NULL",
    )
    .await;
    assert_eq!(pending, 5);
}

#[tokio::test]
async fn a_member_is_not_invited_again() {
    let app = started().await;
    let (ada_in_acme, _) = two_owners(&app).await;

    let again = invite(&app, &ada_in_acme, "grace@example.com").await;

    assert_eq!(again.status, 422, "{}", again.body);
    assert!(again.body.contains("is a member already"), "{}", again.body);
}

#[tokio::test]
async fn the_last_owner_keeps_its_account_and_another_leaves_with_it() {
    let app = started().await;
    let (ada, acme) = founded(&app).await;
    let ada_account = signed_in_again(&app, "ada@example.com").await;
    let close = |session: TestSession, email: &'static str| {
        let app = &app;
        async move {
            post(
                app,
                "/account/account-data/close",
                &session,
                &json!({ "email": email }),
            )
            .await
        }
    };

    let refused = close(ada_account.clone(), "ada@example.com").await;
    assert_eq!(refused.status, 422, "{}", refused.body);
    assert!(
        refused.body.contains("last owner of Acme"),
        "{}",
        refused.body
    );

    let owner = json!({ "email": "grace@example.com", "role": "OWNER" });
    post(&app, "/org/members/invite", &ada, &owner)
        .await
        .assert_ok();
    let link = invitation_link(&app).await;
    let grace = signed_up(&app, "grace@example.com").await;
    accept(&app, &link, &grace).await.assert_ok();
    entered(&app, &grace, &acme).await;
    close(grace, "grace@example.com").await.assert_ok();

    let live = count(
        &app,
        "SELECT count(*) FROM sofie_tenancy.organization_member WHERE deleted_at IS NULL",
    )
    .await;
    assert_eq!(live, 1, "only Ada is left");
    assert_eq!(assert_raised::<MemberLeft>(&app).len(), 1);
}

async fn signed_in_again(app: &TestApp, email: &str) -> TestSession {
    let credentials = json!({ "email": email, "password": "correct horse battery" });
    app.post("/sign-in")
        .json(&credentials)
        .await
        .session()
        .unwrap()
}

#[tokio::test]
async fn invitations_are_mailed_at_a_bounded_pace() {
    let app = started().await;
    let (ada, _) = founded(&app).await;

    let mut statuses = Vec::new();
    for index in 0..12 {
        let answer = invite(&app, &ada, &format!("person{index}@example.com")).await;
        statuses.push(answer.status.as_u16());
    }

    assert!(
        statuses[..10].iter().all(|status| *status < 300),
        "{statuses:?}"
    );
    assert_eq!(statuses[11], 429, "{statuses:?}");
}

#[tokio::test]
async fn an_invitation_accepted_after_the_seats_shrank_is_refused() {
    const SMALL: Plan = Plan::new("small").seats(1).trial_days(14);
    const PLANS: Plans = Plans::new(Plan::new("team").seats(3), &[SMALL]);
    let app = started_with(&PLANS).await;
    let (ada_in_acme, acme) = founded(&app).await;
    invite(&app, &ada_in_acme, "grace@example.com")
        .await
        .assert_ok();
    let link = invitation_link(&app).await;
    let organization: sofie::db::Uuid = acme.as_str().unwrap().parse().unwrap();
    let billing = sofie::billing::install(app.db(), &PLANS);
    let mut trial = billing
        .start_trial(sofie::billing::Subscriber::new(organization), SMALL)
        .await
        .unwrap();
    app.db().commit(&mut trial).await.unwrap();
    let grace = signed_up(&app, "grace@example.com").await;

    let accepted = accept(&app, &link, &grace).await;

    assert_eq!(accepted.status, 422, "{}", accepted.body);
    let members = count(
        &app,
        "SELECT count(*) FROM sofie_tenancy.organization_member WHERE deleted_at IS NULL",
    )
    .await;
    assert_eq!(members, 1);
}

#[tokio::test]
async fn closing_an_account_withdraws_the_invitations_it_sent() {
    let app = started().await;
    let (_, grace_in_acme) = two_owners(&app).await;
    invite(&app, &grace_in_acme, "bob@example.com")
        .await
        .assert_ok();
    let grace = signed_in_again(&app, "grace@example.com").await;

    post(
        &app,
        "/account/account-data/close",
        &grace,
        &json!({ "email": "grace@example.com" }),
    )
    .await
    .assert_ok();

    let open = count(
        &app,
        "SELECT count(*) FROM sofie_tenancy.invitation \
         WHERE accepted_at IS NULL AND expires_at > now()",
    )
    .await;
    assert_eq!(open, 0);
}

#[tokio::test]
async fn removing_a_member_withdraws_the_invitations_it_sent() {
    let app = started().await;
    let (ada, grace) = two_owners(&app).await;
    invite(&app, &grace, "bob@example.com").await.assert_ok();
    invite(&app, &ada, "carol@example.com").await.assert_ok();
    let grace_id = own_member_id(&app, &ada, "grace@example.com").await;

    post(
        &app,
        &format!("/org/members/{grace_id}/remove"),
        &ada,
        &json!({}),
    )
    .await
    .assert_ok();

    let open = count(
        &app,
        "SELECT count(*) FROM sofie_tenancy.invitation \
         WHERE accepted_at IS NULL AND expires_at > now()",
    )
    .await;
    assert_eq!(open, 1, "only the remaining owner's invitation stays open");
}

#[tokio::test]
async fn demoting_a_member_withdraws_the_invitations_its_new_role_could_not_send() {
    let app = started().await;
    let (ada, grace) = two_owners(&app).await;
    invite(&app, &grace, "bob@example.com").await.assert_ok();
    let owner_invitation = json!({ "email": "carol@example.com", "role": "OWNER" });
    post(&app, "/org/members/invite", &grace, &owner_invitation)
        .await
        .assert_ok();
    let grace_id = own_member_id(&app, &ada, "grace@example.com").await;
    let open = || {
        count(
            &app,
            "SELECT count(*) FROM sofie_tenancy.invitation \
             WHERE accepted_at IS NULL AND expires_at > now()",
        )
    };
    let demote = |role: &str| format!("/org/members/{grace_id}/{role}");

    post(&app, &demote("admin"), &ada, &json!({}))
        .await
        .assert_ok();
    let as_admin = open().await;
    post(&app, &demote("member"), &ada, &json!({}))
        .await
        .assert_ok();

    assert_eq!(as_admin, 1, "an admin invites no owner");
    assert_eq!(open().await, 0, "a member invites nobody");
}

#[tokio::test]
async fn an_invitation_racing_its_inviter_s_demotion_stays_closed() {
    let app = started().await;
    let (ada, grace) = two_owners(&app).await;
    let grace_id = own_member_id(&app, &ada, "grace@example.com").await;
    let role = |role: &str| format!("/org/members/{grace_id}/{role}");

    for round in 0..10 {
        post(&app, &role("admin"), &ada, &json!({}))
            .await
            .assert_ok();
        let (email, member, empty) = (format!("bob{round}@example.com"), role("member"), json!({}));
        let (invited, demoted) = tokio::join!(
            invite(&app, &grace, &email),
            post(&app, &member, &ada, &empty),
        );
        // An invitation committed while the demotion was read makes it a conflict, retried as
        // a client would; the retry withdraws that invitation too.
        if demoted.status == 409 {
            post(&app, &member, &ada, &empty).await.assert_ok();
        } else {
            demoted.assert_ok();
        }

        let open = count(
            &app,
            "SELECT count(*) FROM sofie_tenancy.invitation \
             WHERE accepted_at IS NULL AND expires_at > now()",
        )
        .await;
        assert_eq!(open, 0, "round {round}: {}", invited.body);
    }
}

async fn verified(app: &TestApp, email: &str) -> bool {
    let sql = format!(
        "SELECT count(*) FROM sofie_identity.account \
         WHERE email = '{email}' AND verified_at IS NOT NULL"
    );
    count(app, &sql).await == 1
}

#[tokio::test]
async fn accepting_by_the_mailed_link_confirms_the_invited_address_only() {
    let app = started().await;
    let (ada_in_acme, _) = founded(&app).await;
    invite(&app, &ada_in_acme, "Grace@Example.com")
        .await
        .assert_ok();
    let link = invitation_link(&app).await;
    let eve = signed_up(&app, "eve@example.com").await;
    let grace = signed_up(&app, "grace@example.com").await;
    assert!(!verified(&app, "grace@example.com").await);

    let forwarded = accept(&app, &link, &eve).await;
    accept(&app, &link, &grace).await.assert_ok();

    assert_eq!(forwarded.status, 422, "{}", forwarded.body);
    assert!(!verified(&app, "eve@example.com").await);
    assert!(verified(&app, "grace@example.com").await);
}

#[tokio::test]
async fn accepting_needs_the_signed_link() {
    let app = started().await;
    let (ada_in_acme, _) = founded(&app).await;
    invite(&app, &ada_in_acme, "grace@example.com")
        .await
        .assert_ok();
    let link = invitation_link(&app).await;
    let unsigned = link.split('&').next().unwrap().to_owned();
    let grace = signed_up(&app, "grace@example.com").await;

    let refused = post(&app, &unsigned, &grace, &json!({})).await;

    assert!(refused.status.is_client_error(), "{}", refused.body);
    assert!(!verified(&app, "grace@example.com").await);
}

/// Ada founds Acme and Grace joins it as a member.
async fn with_a_member(app: &TestApp) -> (TestSession, TestSession) {
    let (ada_in_acme, acme) = founded(app).await;
    invite(app, &ada_in_acme, "grace@example.com")
        .await
        .assert_ok();
    let link = invitation_link(app).await;
    let grace = signed_up(app, "grace@example.com").await;
    accept(app, &link, &grace).await.assert_ok();
    (ada_in_acme, entered(app, &grace, &acme).await)
}

async fn roles(app: &TestApp, session: &TestSession) -> Vec<(String, String)> {
    let members = get(app, "/org/members", session).await;
    let mut roles: Vec<(String, String)> = members["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|member| {
            (
                member["email"].as_str().unwrap().to_owned(),
                member["role"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    roles.sort();
    roles
}

#[tokio::test]
async fn an_owner_hands_the_organization_over_and_stays_an_admin() {
    let app = started().await;
    let (ada, grace) = with_a_member(&app).await;
    let ada_id = own_member_id(&app, &ada, "ada@example.com").await;
    let grace_id = own_member_id(&app, &ada, "grace@example.com").await;
    let hand_over = |id: &str| format!("/org/members/{id}/handover");

    let by_a_member = post(&app, &hand_over(&ada_id), &grace, &json!({})).await;
    let to_itself = post(&app, &hand_over(&ada_id), &ada, &json!({})).await;
    post(&app, &hand_over(&grace_id), &ada, &json!({}))
        .await
        .assert_ok();

    assert_eq!(by_a_member.status, 422, "{}", by_a_member.body);
    assert_eq!(to_itself.status, 422, "{}", to_itself.body);
    assert_eq!(
        roles(&app, &grace).await,
        [
            ("ada@example.com".to_owned(), "ADMIN".to_owned()),
            ("grace@example.com".to_owned(), "OWNER".to_owned()),
        ]
    );
}

#[tokio::test]
async fn a_member_leaves_but_the_last_owner_does_not() {
    let app = started().await;
    let (ada, grace) = with_a_member(&app).await;

    let last_owner = post(&app, "/org/members/leave", &ada, &json!({})).await;
    post(&app, "/org/members/leave", &grace, &json!({}))
        .await
        .assert_ok();

    assert_eq!(last_owner.status, 422, "{}", last_owner.body);
    let gone = app.get("/org/members").session(&grace).accept_json().await;
    assert_eq!(gone.status, 401, "{}", gone.body);
    assert_eq!(
        roles(&app, &ada).await,
        [("ada@example.com".to_owned(), "OWNER".to_owned())]
    );
    assert_eq!(assert_raised::<MemberLeft>(&app).len(), 1);
}

#[tokio::test]
async fn only_an_owner_renames_the_organization() {
    let app = started().await;
    let (ada, grace) = with_a_member(&app).await;
    let globex = json!({ "name": " Globex " });

    let by_a_member = post(&app, "/org/organization/rename", &grace, &globex).await;
    let blank = post(
        &app,
        "/org/organization/rename",
        &ada,
        &json!({ "name": " " }),
    )
    .await;
    post(&app, "/org/organization/rename", &ada, &globex)
        .await
        .assert_ok();

    assert_eq!(by_a_member.status, 422, "{}", by_a_member.body);
    assert_eq!(blank.status, 422, "{}", blank.body);
    let account = signed_in_again(&app, "grace@example.com").await;
    let organizations = get(&app, "/account/organizations", &account).await;
    assert_eq!(organizations["organizations"][0]["name"], "Globex");
}

#[tokio::test]
async fn an_owner_deletes_the_organization_and_every_member_leaves_it() {
    let app = started().await;
    let (ada, grace) = with_a_member(&app).await;
    invite(&app, &ada, "bob@example.com").await.assert_ok();

    let by_a_member = post(&app, "/org/organization/delete", &grace, &json!({})).await;
    post(&app, "/org/organization/delete", &ada, &json!({}))
        .await
        .assert_ok();

    assert_eq!(by_a_member.status, 422, "{}", by_a_member.body);
    let live = |sql: &'static str| count(&app, sql);
    assert_eq!(
        live("SELECT count(*) FROM sofie_tenancy.organization WHERE deleted_at IS NULL").await,
        0
    );
    assert_eq!(
        live("SELECT count(*) FROM sofie_tenancy.organization_member WHERE deleted_at IS NULL")
            .await,
        0
    );
    assert_eq!(
        live(
            "SELECT count(*) FROM sofie_tenancy.invitation \
             WHERE accepted_at IS NULL AND expires_at > now()"
        )
        .await,
        0
    );
    let deleted = assert_raised::<OrganizationDeleted>(&app);
    assert_eq!(deleted.len(), 1);
    assert_eq!(assert_raised::<MemberLeft>(&app).len(), 2);
    let ada_account = signed_in_again(&app, "ada@example.com").await;
    let organizations = get(&app, "/account/organizations", &ada_account).await;
    assert_eq!(organizations["organizations"], json!([]));
    let gone = app.get("/org/members").session(&ada).accept_json().await;
    assert_eq!(gone.status, 401, "{}", gone.body);
    let gone = app.get("/org/members").session(&grace).accept_json().await;
    assert_eq!(gone.status, 401, "{}", gone.body);
    post(
        &app,
        "/account/account-data/close",
        &ada_account,
        &json!({ "email": "ada@example.com" }),
    )
    .await
    .assert_ok();
}

#[tokio::test]
async fn an_invitation_left_open_on_a_deleted_organization_is_not_accepted() {
    let app = started().await;
    let (ada, _) = founded(&app).await;
    invite(&app, &ada, "grace@example.com").await.assert_ok();
    let link = invitation_link(&app).await;
    let grace = signed_up(&app, "grace@example.com").await;
    let page = get(&app, &link, &grace).await;
    let action = page["invitation"]["action"].as_str().unwrap().to_owned();
    post(&app, "/org/organization/delete", &ada, &json!({}))
        .await
        .assert_ok();
    // As an invitation committed while the deletion was read would be left.
    app.query(
        "UPDATE sofie_tenancy.invitation SET expires_at = expires_at + interval '1 day' \
         RETURNING id",
        &[],
    )
    .await;

    let accepted = post(&app, &action, &grace, &json!({})).await;

    assert!(accepted.status.is_client_error(), "{}", accepted.body);
    let members = count(
        &app,
        "SELECT count(*) FROM sofie_tenancy.organization_member WHERE deleted_at IS NULL",
    )
    .await;
    assert_eq!(members, 0);
}

#[tokio::test]
async fn an_organization_whose_plan_renews_at_the_provider_is_not_deleted() {
    const PRO: Plan = Plan::new("pro").seats(5).trial_days(14);
    const PLANS: Plans = Plans::new(Plan::new("free"), &[PRO]);
    let app = started_with(&PLANS).await;
    let (ada, acme) = founded(&app).await;
    let organization: sofie::db::Uuid = acme.as_str().unwrap().parse().unwrap();
    let billing = sofie::billing::install(app.db(), &PLANS);
    let mut trial = billing
        .start_trial(sofie::billing::Subscriber::new(organization), PRO)
        .await
        .unwrap();
    app.db().commit(&mut trial).await.unwrap();
    let billed = |sql: &'static str| {
        let app = &app;
        async move {
            app.query(sql, &[sofie::core::SqlValue::Uuid(organization)])
                .await
        }
    };
    billed(
        "UPDATE sofie_billing.subscription SET status = 'active', provider = 'stripe', \
         reference = 'sub_1' WHERE subscriber = $1 RETURNING id",
    )
    .await;

    let running = post(&app, "/org/organization/delete", &ada, &json!({})).await;
    billed(
        "UPDATE sofie_billing.subscription SET cancels_at_period_end = true \
         WHERE subscriber = $1 RETURNING id",
    )
    .await;
    let canceled = post(&app, "/org/organization/delete", &ada, &json!({})).await;

    assert_eq!(running.status, 422, "{}", running.body);
    assert!(
        running.body.contains("Cancel the organization's plan"),
        "{}",
        running.body
    );
    canceled.assert_ok();
}

/// Without a provider set up the cancellation fails, which shows the listener asked for it.
#[tokio::test]
async fn a_plan_started_for_a_deleted_organization_is_canceled_at_the_provider() {
    const PRO: Plan = Plan::new("pro").seats(5).trial_days(14);
    const PLANS: Plans = Plans::new(Plan::new("free"), &[PRO]);
    let app = started_with(&PLANS).await;
    let (ada, acme) = founded(&app).await;
    let organization: sofie::db::Uuid = acme.as_str().unwrap().parse().unwrap();
    post(&app, "/org/organization/delete", &ada, &json!({}))
        .await
        .assert_ok();
    app.drain().await;
    let billing = sofie::billing::install(app.db(), &PLANS);
    let mut late = billing
        .start_trial(sofie::billing::Subscriber::new(organization), PRO)
        .await
        .unwrap();
    app.db().commit(&mut late).await.unwrap();
    app.query(
        "UPDATE sofie_billing.subscription SET status = 'active', provider = 'stripe', \
         reference = 'sub_1' WHERE subscriber = $1 RETURNING id",
        &[sofie::core::SqlValue::Uuid(organization)],
    )
    .await;

    let failures = sofie_testing::drain_failing(&app).await;

    assert_eq!(failures.len(), 1, "{failures:?}");
    assert_eq!(failures[0].job, "sofie_tenancy.cancel_deleted_plan");
    assert!(
        failures[0].error.contains("no payment provider"),
        "{}",
        failures[0].error
    );
}

#[tokio::test]
async fn closing_the_last_owner_s_account_names_every_organization_it_holds_alone() {
    let app = started().await;
    founded(&app).await;
    let ada = signed_in_again(&app, "ada@example.com").await;
    post(
        &app,
        "/account/organizations",
        &ada,
        &json!({ "name": "Globex" }),
    )
    .await
    .assert_ok();

    let refused = post(
        &app,
        "/account/account-data/close",
        &ada,
        &json!({ "email": "ada@example.com" }),
    )
    .await;

    assert_eq!(refused.status, 422, "{}", refused.body);
    assert!(
        refused.body.contains("last owner of Acme, Globex"),
        "{}",
        refused.body
    );
}
