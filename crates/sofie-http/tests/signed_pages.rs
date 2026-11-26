#![cfg(all(feature = "signed-links", feature = "engine"))]

use std::time::Duration;

use http::StatusCode;
use serde_json::json;
use sofie_core::{Error, Result};
use sofie_db::{Db, Outcome, ReadOnly, Reply};
use sofie_http::{App, Routes, SignedUrls, Web};
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

struct Link(String);

impl Outcome for Link {
    fn reply(self) -> Result<Reply> {
        Ok(Reply::Json(json!(self.0)))
    }
}

impl ReadOnly for Link {}

async fn ticket_link(urls: SignedUrls) -> Result<Link> {
    urls.sign("/ticket?seat=4", Duration::from_hours(1))
        .map(Link)
}

async fn claim_ticket() -> Result<()> {
    Ok(())
}

fn routes(routes: &mut Routes<Web>) {
    routes.get("/ticket-link", ticket_link);
    routes.screen("/ticket", "ticket", show_invite).signed();
    routes
        .post("/ticket", claim_ticket)
        .signed()
        .then_screen("ticket", "ticket");
    routes.screen("/invite", "invite", show_invite).signed();
    routes.post("/invite", accept_invite).then_redirect("/");
}

/// A refused form comes back on the page at its path, which must not open a page its own
/// signature guards.
#[tokio::test]
async fn a_refused_form_does_not_show_a_signed_page_without_its_signature() {
    let server = TestServer::start(app()).await;

    let refused = server.post("/invite").form(&[]).await;

    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        !refused.body.contains("the-invite-code"),
        "{}",
        refused.body
    );
}

/// The signature covers the link's path and query, so the page at the action's own path is
/// reached through the very link the form posted to.
#[tokio::test]
async fn an_action_on_a_signed_link_shows_its_signed_page_through_the_same_link() {
    let server = TestServer::start(app()).await;
    let link = server.get("/ticket-link").await.json();
    let link = link.as_str().unwrap();

    let claimed = server.post(link).form(&[]).await;
    let page = server
        .get(claimed.headers["location"].to_str().unwrap())
        .await;

    assert_eq!(claimed.status, StatusCode::SEE_OTHER);
    assert_eq!(claimed.headers["location"], link);
    assert_eq!(page.status, StatusCode::OK);
}

fn app() -> App {
    let screens = sofie_http::screens_from_sources(&[
        ("invites/web/invite", INVITE_SCREEN),
        ("invites/web/ticket", INVITE_SCREEN),
    ]);
    App::new(Db::new(FakeDriver::default()))
        .screens(screens.unwrap())
        .area("invites", routes)
}
