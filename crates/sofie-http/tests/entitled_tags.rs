#![cfg(all(
    feature = "entitlements",
    feature = "etag",
    not(feature = "development")
))]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use http::StatusCode;
use serde_json::json;
use sofie_core::{Result, Uuid};
use sofie_db::{BoxFuture, Db, Id, Outcome, Read, ReadOnly, Reply};
use sofie_http::{Admitter, App, Area, Entitled, Gates, Routes, Session, SignInRight, Web};
use sofie_testing::{FakeDriver, TestServer};

const MEMBER: Uuid = Uuid::from_u128(0x0190_a7d2_0000_7000_8000_0000_0000_0001);
const PLAN_SCREEN: &str = r#"<p>{#if entitled("shop.export")}export{:else}locked{/if}</p>"#;

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

static UPGRADED: AtomicBool = AtomicBool::new(false);

struct Plans;

impl Entitled for Plans {
    fn held<'a>(
        &'a self,
        _read: &'a Read,
        _area: &'static str,
        _principal: Uuid,
    ) -> BoxFuture<'a, Result<Arc<[&'static str]>>> {
        let held: Arc<[&'static str]> = if UPGRADED.load(Ordering::SeqCst) {
            Arc::from(["shop.export"])
        } else {
            Arc::from([])
        };
        Box::pin(std::future::ready(Ok(held)))
    }
}

struct Plan;

impl Outcome for Plan {
    fn reply(self) -> Result<Reply> {
        Ok(Reply::Json(json!({})))
    }
}

impl ReadOnly for Plan {}

async fn show_plan() -> Result<Plan> {
    Ok(Plan)
}

struct Tests;

impl Admitter for Tests {}

async fn sign_in(session: Session) -> Result<()> {
    session.sign_in::<Members>(SignInRight::of::<Tests>(), Id::from_uuid(MEMBER))?;
    Ok(())
}

fn web_routes(routes: &mut Routes<Web>) {
    routes.post("/sign-in", sign_in);
}

fn member_routes(routes: &mut Routes<Members>) {
    routes.screen("/plan", "plan", show_plan);
}

/// Entitlements are read before the rows a tag folds in, so they key the tag themselves.
#[tokio::test]
async fn an_upgraded_plan_changes_the_tag_of_a_page_it_shows() {
    let screens = sofie_http::screens_from_sources(&[("shop/members/plan", PLAN_SCREEN)]);
    let app = App::new(Db::new(FakeDriver::default()))
        .screens(screens.unwrap())
        .gates(Gates::default().entitlements(Plans))
        .area("shop", web_routes)
        .area("shop", member_routes);
    let server = TestServer::start(app).await;
    let signed_in = server
        .post("/sign-in")
        .header("sec-fetch-site", "same-origin")
        .await;
    let cookie = signed_in.header("set-cookie");
    let cookie = cookie.split(';').next().unwrap().to_owned();
    let locked = server.get("/members/plan").header("cookie", &cookie).await;
    let tag = locked.header("etag").to_owned();

    UPGRADED.store(true, Ordering::SeqCst);
    let upgraded = server
        .get("/members/plan")
        .header("cookie", &cookie)
        .header("if-none-match", &tag)
        .await;

    assert!(locked.body.contains("locked"), "{}", locked.body);
    assert_eq!(upgraded.status, StatusCode::OK);
    assert!(upgraded.body.contains("export"), "{}", upgraded.body);
}
