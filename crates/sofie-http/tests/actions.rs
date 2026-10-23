//! An `Action` in a loop posts its row's fields, which the route reads as its input.
use serde_json::{Value, json};
use sofie_core::Result;
use sofie_db::{Db, Input, InputFields, Outcome, ReadOnly, Reply, Resolve, Writes};
use sofie_http::{App, Routes, Web};
use sofie_testing::{FakeDriver, TestServer};

const SLOTS_SCREEN: &str = "<ul>{#each data as slot}<li><Action id=\"book-{slot.id}\" \
    post=\"/slots/book\" label=\"Book {slot.id}\" with={{slot: slot.id}} /></li>{/each}</ul>";

struct Answer(Value);

impl Outcome for Answer {
    fn reply(self) -> Result<Reply> {
        Ok(Reply::Json(self.0))
    }
}

impl ReadOnly for Answer {}

struct Booking {
    slot: u32,
}

impl Input for Booking {
    fn from_json(value: Value) -> Result<Self> {
        let mut fields = InputFields::new(value)?;
        let slot = fields.take("slot");
        fields.finish()?;
        Ok(Self { slot: slot? })
    }
}

impl<C: Writes> Resolve<C> for Booking {
    fn resolve(context: &mut C) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(context.input().and_then(Self::from_json))
    }
}

async fn list_slots() -> Result<Answer> {
    Ok(Answer(json!([{ "id": 7 }, { "id": 8 }])))
}

async fn book(booking: Booking) -> Result<Answer> {
    Ok(Answer(json!({ "booked": booking.slot })))
}

async fn slots_server() -> TestServer {
    let screens = sofie_http::screens_from_sources(&[("shop/web/slots", SLOTS_SCREEN)]).unwrap();
    let app = App::new(Db::new(FakeDriver::default()))
        .screens(screens)
        .area("shop", |routes: &mut Routes<Web>| {
            routes.screen("/slots", "slots", list_slots);
            routes.post("/slots/book", book);
        });
    TestServer::start(app).await
}

/// The hidden inputs of the form with `id`, as a browser would post them.
fn posted_form(html: &str, id: &str) -> String {
    let start = html.find(&format!("<form id=\"{id}\"")).unwrap();
    let form = &html[start..start + html[start..].find("</form>").unwrap()];
    form.split("<input type=\"hidden\" name=\"")
        .skip(1)
        .map(|input| {
            let (name, rest) = input.split_once("\" value=\"").unwrap();
            let value = &rest[..rest.find('"').unwrap()];
            format!("{name}={value}")
        })
        .collect::<Vec<_>>()
        .join("&")
}

#[tokio::test]
async fn a_row_s_action_posts_its_fields_to_the_route_s_input() {
    let server = slots_server().await;
    let page = server.get("/slots").header("accept", "text/html").await;
    let body = posted_form(&page.body, "book-8");

    let booked = server
        .post("/slots/book")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body.clone())
        .await;

    assert_eq!(body, "slot=8");
    assert_eq!(booked.json(), json!({ "booked": 8 }));
}

#[tokio::test]
async fn a_client_area_s_action_posts_its_fields_as_json() {
    let server = slots_server().await;

    let booked = server.post("/slots/book").json(&json!({ "slot": 7 })).await;

    assert_eq!(booked.json(), json!({ "booked": 7 }));
}
