#![allow(dead_code, reason = "each test crate uses only some of these")]

use sofie::core::SqlValue;
use sofie::prelude::*;
use sofie_identity::{
    Account, AccountId, MemberArea, Permitted, RoleId, assign_role, create_role, grant,
};
use sofie_testing::{TestApp, TestSession};

pub struct Member;

impl Area for Member {
    const NAME: &'static str = "member";
    const PREFIX: &'static str = "/account";
    const SIGN_IN: &'static str = "/sign-in";
    type Principal = Account;
}

impl MemberArea for Member {
    const HOME: &'static str = "/account";
    const SIGN_UP: Option<&'static str> = Some("/sign-up");
}

pub struct Console;

impl Area for Console {
    const NAME: &'static str = "console";
    const PREFIX: &'static str = "/console";
    const SIGN_IN: &'static str = "/console/sign-in";
    const TOKENS: bool = true;
    type Principal = Permitted;
}

impl MemberArea for Console {
    const HOME: &'static str = "/console";
    const SIGN_UP: Option<&'static str> = Some("/console/sign-up");
}

pub const PASSWORD: &str = "correct horse battery";

pub async fn joined<A: MemberArea>(app: &TestApp, email: &str) -> (AccountId, TestSession) {
    let session = sofie_identity::testing::signed_up::<A>(app, email, PASSWORD).await;
    (account_id(app, email).await, session)
}

pub async fn account_id(app: &TestApp, email: &str) -> AccountId {
    let rows = app
        .query(
            "SELECT id FROM sofie_identity.account WHERE email = $1",
            &[SqlValue::Text(email.to_owned())],
        )
        .await;
    let SqlValue::Uuid(account) = rows[0][0] else {
        panic!("an account id is a uuid");
    };
    AccountId::from_uuid(account)
}

pub async fn grant_role(
    app: &TestApp,
    account: AccountId,
    name: &str,
    permissions: &[&str],
) -> RoleId {
    grant_role_in(app, "console", account, name, permissions).await
}

pub async fn grant_role_in(
    app: &TestApp,
    area: &str,
    account: AccountId,
    name: &str,
    permissions: &[&str],
) -> RoleId {
    confirm(app, account).await;
    let read = app.db().read();
    let mut role = create_role(&read, name).await.unwrap();
    app.db().commit(&mut role).await.unwrap();
    for permission in permissions {
        let mut granted = grant(&read, role.id(), permission).await.unwrap();
        app.db().commit(&mut granted).await.unwrap();
    }
    let mut membership = assign_role(&read, &app.db().clock(), account, area, Some(role.id()))
        .await
        .unwrap();
    app.db().commit(&mut membership).await.unwrap();
    role.id()
}

/// Giving a role distrusts an account nobody confirmed, as an attacker may have signed it up.
pub async fn confirm(app: &TestApp, account: AccountId) {
    app.query(
        "UPDATE sofie_identity.account SET verified_at = now() WHERE id = $1",
        &[SqlValue::Uuid(account.uuid())],
    )
    .await;
}
