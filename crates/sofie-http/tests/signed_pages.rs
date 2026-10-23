#![cfg(all(feature = "signed-links", feature = "engine"))]

use http::StatusCode;
use serde_json::json;
use sofie_core::{Error, Result};
use sofie_db::{Db, Outcome, ReadOnly, Reply};
use sofie_http::{App, Routes, Web};
use sofie_testing::{FakeDriver, TestServer};

const INVITE_SCREEN: &str = "<p>{data.code}</p>";

struct Invite;

impl Outcome for Invite {
    fn reply(self) -> Result<Reply> {
        Ok(Reply::Json(json!({ "code": "the-invite-code" })))
    }
}

impl ReadOnly for Invite {}

async fn show_invite() -> Result<Invite> {
    Ok(Invite)
}

async fn accept_invite() -> Result<()> {
    Err(Error::invalid("name", "is required"))
}

fn routes(routes: &mut Routes<Web>) {
    routes.screen("/invite", "invite", show_invite).signed();
    routes.post("/invite", accept_invite).then_redirect("/");
}

/// A refused form comes back on the page at its path, which must not open a page its own
/// signature guards.
#[tokio::test]
async fn a_refused_form_does_not_show_a_signed_page_without_its_signature() {
    let screens = sofie_http::screens_from_sources(&[("invites/web/invite", INVITE_SCREEN)]);
    let app = App::new(Db::new(FakeDriver::default()))
        .screens(screens.unwrap())
        .area("invites", routes);
    let server = TestServer::start(app).await;

    let refused = server.post("/invite").form(&[]).await;

    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        !refused.body.contains("the-invite-code"),
        "{}",
        refused.body
    );
}
