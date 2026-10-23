//! Run with `cargo test --release --features toolbar --test toolbar_release`: a
//! release build with the feature on holds none of the toolbar.
#![cfg(all(feature = "toolbar", not(debug_assertions)))]

use sofie_core::{Result, Uuid};
use sofie_db::{Db, Id};
use sofie_http::{App, Area, Principal, Routes, Web};
use sofie_testing::{FakeDriver, TestServer};

const MEMBER: Uuid = Uuid::from_u128(0x0190_a7d2_0000_7000_8000_0000_0000_0001);

struct Account;

impl sofie_http::AreaPrincipal for Account {
    const ACCESS: sofie_http::Access = sofie_http::Access::Owned;
    type Record = Self;
}

struct Members;

impl Area for Members {
    const NAME: &'static str = "members";
    const PREFIX: &'static str = "/members";
    const SIGN_IN: &'static str = "/sign-in";
    type Principal = Account;
}

async fn mine(_me: Principal<Members>) -> Result<()> {
    Ok(())
}

async fn hello() -> Result<()> {
    Ok(())
}

#[tokio::test]
async fn a_release_build_has_no_toolbar_and_ignores_its_cookie() {
    let screens = [("notes/web/hello", "<Section title=\"Hello\"></Section>")];
    let app = App::new(Db::new(FakeDriver::default()))
        .screens(sofie_http::screens_from_sources(&screens).unwrap())
        .area("notes", |routes: &mut Routes<Web>| {
            routes.screen("/hello", "hello", hello);
        })
        .area("notes", |routes: &mut Routes<Members>| {
            routes.get("/mine", mine);
        });
    let server = TestServer::start(app).await;
    let cookie = format!("__Host-sofie-toolbar=as=members:{MEMBER}");

    let form = server
        .post("/_sofie/toolbar")
        .header("sec-fetch-site", "same-origin")
        .form(&[("action", "reset_clock")])
        .await;
    let acting = server
        .get("/members/mine")
        .header("cookie", &cookie)
        .accept_json()
        .await;
    let page = server.get("/hello").header("cookie", &cookie).await;

    form.assert_status(404);
    acting.assert_status(401);
    page.assert_ok();
    assert!(!page.body.contains("sofie-toolbar"), "{}", page.body);
}
