#![cfg(all(feature = "session-list", feature = "i18n", feature = "parse"))]

use serde_json::json;
use sofie_core::{Result, Timestamp, Uuid};
use sofie_db::{BoxFuture, Db, File, Id, Outcome, ReadOnly, Reply};
use sofie_http::{
    Admitter, App, Area, AreaMembers, Routes, Session, SessionRecord, SignInRight, Web,
};
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

/// Every session is its account's, which chose Turkish.
struct Store;

impl AreaMembers for Store {
    fn session_record<'a>(
        &'a self,
        _db: &'a Db,
        _session: Uuid,
        _seen: Option<Timestamp>,
    ) -> BoxFuture<'a, Result<SessionRecord>> {
        Box::pin(std::future::ready(Ok(SessionRecord::Live {
            account: MEMBER,
            expires_at: None,
            impersonator: None,
            language: Some("tr".into()),
        })))
    }
}

struct Tests;

impl Admitter for Tests {}

async fn sign_in(session: Session) -> Result<()> {
    session.sign_in::<Members>(SignInRight::of::<Tests>(), Id::from_uuid(MEMBER))?;
    Ok(())
}

struct Greeting;

impl Outcome for Greeting {
    fn reply(self) -> Result<Reply> {
        Ok(Reply::Json(json!({})))
    }
}

impl ReadOnly for Greeting {}

async fn greeting() -> Result<Greeting> {
    Ok(Greeting)
}

fn app() -> App {
    let mut screens = sofie_http::screens_from_sources(&[(
        "shop/members/greeting",
        r#"<Section title={t("greeting.title")}></Section>"#,
    )])
    .unwrap();
    let english = File::new("lang/en.toml", b"[greeting]\ntitle = \"Hello\"\n");
    let turkish = File::new("lang/tr.toml", b"[greeting]\ntitle = \"Merhaba\"\n");
    screens.translate(
        sofie_http::translations(
            &["en", "tr"],
            &[("shop", english), ("shop", turkish)],
            &[],
            &[],
        )
        .unwrap(),
    );
    App::new(Db::new(FakeDriver::default()))
        .screens(screens)
        .membership(Store)
        .area("shop", |routes: &mut Routes<Web>| {
            routes.post("/sign-in", sign_in);
        })
        .area("shop", |routes: &mut Routes<Members>| {
            routes.screen("/greeting", "greeting", greeting);
        })
}

#[tokio::test]
async fn a_signed_in_account_s_chosen_language_wins_over_the_browser_s() {
    let server = TestServer::start(app()).await;
    let session = server.post("/sign-in").await.session().unwrap();

    let page = server
        .get("/members/greeting")
        .session(&session)
        .header("accept-language", "en")
        .await;

    page.assert_status(200);
    assert!(page.body.contains("<html lang=\"tr\""), "{}", page.body);
    assert!(page.body.contains("Merhaba"), "{}", page.body);
}
