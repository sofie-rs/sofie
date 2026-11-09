#![cfg(feature = "screens")]

use sofie_core::{Result, Uuid};
use sofie_db::worker::Worker;
use sofie_db::{Db, Id, Queued};
use sofie_http::{App, Area, Principal, Routes};
use sofie_notify::{Channel, Notice, Notify, Recipient, Topic};
use sofie_testing::{TestApp, TestPrincipal};

const SHIPPED: Topic = Topic::new("shop.order_shipped").channels(&[Channel::Inbox]);

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

async fn ship(me: Principal<Members>, notify: Notify) -> Result<Queued> {
    let notice = Notice::new("Your order shipped")?.link("/members/orders/7")?;
    notify.send(Recipient::from(me.id()), SHIPPED, notice)
}

async fn start() -> TestApp {
    sofie_testing::start(
        |db: Db| async move {
            Ok(App::new(db)
                .area::<Members>("sofie_notify", sofie_notify::inbox_routes::<Members>)
                .area::<Members>("shop", |routes: &mut Routes<Members>| {
                    routes.post("/ship", ship);
                }))
        },
        Worker::new,
    )
    .await
}

#[tokio::test]
async fn the_inbox_screen_hears_a_notification_arrive_and_shows_it_once_refetched() {
    let app = start().await;
    let me = app.as_(&TestPrincipal::<Members>::new(Id::from_uuid(
        Uuid::from_u128(1),
    )));
    let page = me.get("/members/notifications").await;
    assert_eq!(page.status, 200, "{}", page.body);
    let topic = sofie_db::Topic::named("sofie", "inbox").to_string();
    assert!(page.body.contains("id=\"sofie-live\""), "{}", page.body);
    assert!(page.body.contains(&topic), "{}", page.body);
    assert!(!page.body.contains("Your order shipped"), "{}", page.body);
    assert!(page.body.contains("Nothing here yet."), "{}", page.body);
    assert!(!page.body.contains("notify.inbox."), "{}", page.body);
    let mut changes = sofie_db::committed_changes(app.db());

    me.post("/members/ship")
        .accept_json()
        .await
        .assert_status(204);
    app.drain().await;

    let mut heard = false;
    while let Ok(sofie_db::Announced::Changed(announced)) = changes.try_recv() {
        heard |= announced.iter().any(|change| {
            change.topic.to_string() == topic
                && change.readers == sofie_db::Readers::Owner(Uuid::from_u128(1))
        });
    }
    assert!(heard, "the inbox's stream hears the delivery");
    let refetched = me.get("/members/notifications").htmx("inbox").await;
    assert!(
        refetched.body.contains("href=\"/members/orders/7\""),
        "{}",
        refetched.body
    );

    me.post("/members/notifications/read")
        .htmx("inbox")
        .await
        .assert_ok();
    let listed = me.get("/members/notifications").accept_json().await.json();
    assert_eq!(listed["any_unread"], false);
}
