//! Against Postgres, with sofie-testing's fake mail transport and a test clock.

use std::time::Duration;

use serde_json::json;
use sofie_core::{Result, SqlValue, Uuid};
use sofie_db::worker::Worker;
use sofie_db::{Db, Id, Queued};
use sofie_http::{App, Area, Principal, Routes};
use sofie_notify::{
    Changed, Channel, Digest, Inbox, InboxItem, MarkedRead, Notice, Notify, Preferences, Recipient,
    TimeOfDay, Topic,
};
use sofie_testing::{TestApp, TestCaller, TestPrincipal, sent_mails};
use sofie_values::{Email, TimeZone};

const SHIPPED: Topic = Topic::new("shop.order_shipped").channels(&[Channel::Inbox, Channel::Mail]);
const SIGNED_IN: Topic = Topic::new("shop.signed_in")
    .channels(&[Channel::Inbox, Channel::Mail])
    .urgent();
const RECEIPT: Topic = Topic::new("shop.receipt")
    .channels(&[Channel::Mail])
    .never_digested();
const ZONE: &str = "Europe/Istanbul";

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

fn recipient(me: &Principal<Members>) -> Recipient {
    Recipient::from(me.id()).email(&Email::parse("ada@example.test").unwrap())
}

async fn ship(me: Principal<Members>, notify: Notify) -> Result<Queued> {
    let notice = Notice::new("Your order shipped")?
        .body("It arrives on Friday.")?
        .link("/members/orders/7")?;
    notify.send(recipient(&me), SHIPPED, notice)
}

async fn ship_beyond_ascii(me: Principal<Members>, notify: Notify) -> Result<Queued> {
    let recipient = Recipient::from(me.id()).email(&Email::parse("ädä@example.test")?);
    notify.send(recipient, SHIPPED, Notice::new("Your order shipped")?)
}

async fn ship_to_work(me: Principal<Members>, notify: Notify) -> Result<Queued> {
    let recipient = Recipient::from(me.id()).email(&Email::parse("ada@work.example.test")?);
    notify.send(
        recipient,
        SHIPPED,
        Notice::new("Your order shipped to work")?,
    )
}

async fn sign_in_alert(me: Principal<Members>, notify: Notify) -> Result<Queued> {
    notify.send(recipient(&me), SIGNED_IN, Notice::new("A new sign-in")?)
}

struct Shown(serde_json::Value);

impl sofie_db::Outcome for Shown {
    fn reply(self) -> Result<sofie_db::Reply> {
        Ok(sofie_db::Reply::Json(self.0))
    }
}

impl sofie_db::ReadOnly for Shown {}

async fn receipt(me: Principal<Members>, notify: Notify) -> Result<Queued> {
    notify.send(recipient(&me), RECEIPT, Notice::new("Your receipt")?)
}

async fn read_newest(inbox: Inbox) -> Result<MarkedRead> {
    let newest = inbox.latest(1).await?;
    let first = newest.first().ok_or(sofie_core::Error::NotFound)?;
    inbox.mark_read(first.id).await
}

async fn inbox(inbox: Inbox) -> Result<Shown> {
    let items: Vec<InboxItem> = inbox.latest(10).await?;
    Ok(Shown(
        json!({ "unread": inbox.unread().await?, "items": items }),
    ))
}

async fn read_all(inbox: Inbox) -> Result<MarkedRead> {
    Ok(inbox.mark_all_read())
}

static MARKING_BUILT: tokio::sync::Notify = tokio::sync::Notify::const_new();
static MARKING_GO: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// Waits between building its outcome and returning it, so a test can read one meanwhile.
async fn read_all_slowly(inbox: Inbox) -> Result<MarkedRead> {
    let marked = inbox.mark_all_read();
    MARKING_BUILT.notify_one();
    MARKING_GO.notified().await;
    Ok(marked)
}

static CHOICE_BUILT: tokio::sync::Notify = tokio::sync::Notify::const_new();
static CHOICE_GO: tokio::sync::Notify = tokio::sync::Notify::const_new();

async fn mute_mail_slowly(preferences: Preferences) -> Result<Changed> {
    let changed = preferences.choose(SHIPPED, Channel::Mail, false)?;
    CHOICE_BUILT.notify_one();
    CHOICE_GO.notified().await;
    Ok(changed)
}

async fn mute_mail(preferences: Preferences) -> Result<Changed> {
    preferences.choose(SHIPPED, Channel::Mail, false)
}

async fn quiet_nights(preferences: Preferences) -> Result<Changed> {
    let hours = (TimeOfDay::parse("22:00")?, TimeOfDay::parse("07:00")?);
    preferences
        .quiet_hours(Some(hours), TimeZone::parse(ZONE)?)
        .await
}

async fn daily_digest(preferences: Preferences) -> Result<Changed> {
    let at = TimeOfDay::parse("08:00")?;
    preferences
        .digest(Digest::Daily(at), TimeZone::parse(ZONE)?)
        .await
}

async fn no_digest(preferences: Preferences) -> Result<Changed> {
    preferences
        .digest(Digest::Off, TimeZone::parse(ZONE)?)
        .await
}

async fn chosen(preferences: Preferences) -> Result<Shown> {
    serde_json::to_value(preferences.chosen().await?)
        .map(Shown)
        .map_err(|error| sofie_core::Error::internal("serializing preferences", error))
}

async fn export(me: Principal<Members>, read: sofie_db::Read) -> Result<Shown> {
    sofie_notify::personal_data::exported(&read, me.id().uuid())
        .await
        .map(Shown)
}

async fn forget(
    me: Principal<Members>,
    jobs: sofie_db::LocalJobs,
) -> Result<sofie_notify::personal_data::Forgotten> {
    sofie_notify::personal_data::forgetting(&jobs, me.id().uuid()).await
}

fn routes(routes: &mut Routes<Members>) {
    routes.post("/ship", ship);
    routes.post("/sign-in-alert", sign_in_alert);
    routes.post("/ship-beyond-ascii", ship_beyond_ascii);
    routes.post("/ship-to-work", ship_to_work);
    routes.get("/inbox", inbox);
    routes.post("/inbox/read", read_all);
    routes.post("/inbox/read-slowly", read_all_slowly);
    routes.post("/inbox/read-newest", read_newest);
    routes.post("/receipt", receipt);
    routes.post("/preferences/mute", mute_mail);
    routes.post("/preferences/mute-slowly", mute_mail_slowly);
    routes.post("/preferences/quiet", quiet_nights);
    routes.post("/preferences/digest", daily_digest);
    routes.post("/preferences/no-digest", no_digest);
    routes.get("/preferences", chosen);
    routes.post("/push", sofie_notify::subscribe_push::<Members>);
    routes.post("/push/stop", sofie_notify::unsubscribe_push::<Members>);
    routes.get("/personal-data", export);
    routes.post("/forget", forget);
}

async fn start() -> TestApp {
    sofie_testing::start(
        |db: Db| async move { Ok(App::new(db).area::<Members>("shop", routes)) },
        |db| sofie_notify::housekeeping(Worker::new(db)),
    )
    .await
}

fn member(id: u128) -> TestPrincipal<Members> {
    TestPrincipal::new(Id::from_uuid(Uuid::from_u128(id)))
}

async fn unread(caller: &TestCaller<'_>) -> serde_json::Value {
    caller.get("/members/inbox").accept_json().await.json()["unread"].clone()
}

fn move_to(app: &TestApp, utc: &str) {
    let target: sofie_core::Timestamp = utc.parse().unwrap();
    let now = app.db().clock().now().timestamp();
    let ahead = u64::try_from(target.as_second() - now.as_second()).expect("a later time");
    app.clock().advance(Duration::from_secs(ahead)).unwrap();
}

#[tokio::test]
async fn a_notification_reaches_the_inbox_and_the_mailbox_once_committed() {
    let app = start().await;
    let me = app.as_(&member(1));

    assert_eq!(me.post("/members/ship").accept_json().await.status, 204);
    app.drain().await;
    let listed = me.get("/members/inbox").accept_json().await.json();
    me.post("/members/ship").accept_json().await;
    app.drain().await;
    me.post("/members/inbox/read-newest").accept_json().await;
    let one_read = me.get("/members/inbox").accept_json().await.json();
    me.post("/members/inbox/read").accept_json().await;
    let after = me.get("/members/inbox").accept_json().await.json();

    assert_eq!(listed["unread"], 1);
    assert_eq!(listed["items"][0]["title"], "Your order shipped");
    assert_eq!(listed["items"][0]["link"], "/members/orders/7");
    assert_eq!(one_read["unread"], 1);
    assert_eq!(after["unread"], 0);
    let mails = sent_mails(&app);
    assert_eq!(mails.len(), 2);
    assert_eq!(mails[0].subject(), "Your order shipped");
    assert!(mails[0].body().html().contains("It arrives on Friday."));
    assert_eq!(unread(&app.as_(&member(2))).await, 0);
}

#[tokio::test]
async fn an_address_beyond_ascii_gets_the_mail_and_fills_the_inbox() {
    let app = start().await;
    let me = app.as_(&member(8));

    assert_eq!(
        me.post("/members/ship-beyond-ascii")
            .accept_json()
            .await
            .status,
        204
    );
    app.drain().await;

    let mails = sent_mails(&app);
    assert_eq!(mails.len(), 1);
    assert_eq!(mails[0].to()[0].email().as_str(), "ädä@example.test");
    assert_eq!(unread(&me).await, 1);
}

#[tokio::test]
async fn a_mail_that_cannot_link_back_is_skipped_but_the_inbox_is_filled() {
    let app = start().await;
    let outbox = sofie_mail::testing::fake_without_base_url(app.db());
    let me = app.as_(&member(9));

    assert_eq!(me.post("/members/ship").accept_json().await.status, 204);
    app.drain().await;

    assert!(outbox.mails().is_empty());
    assert_eq!(unread(&me).await, 1);
}

#[tokio::test]
async fn a_muted_channel_is_skipped_and_the_choice_is_shown() {
    let app = start().await;
    let me = app.as_(&member(3));

    me.post("/members/preferences/mute").accept_json().await;
    me.post("/members/ship").accept_json().await;
    app.drain().await;

    assert!(sent_mails(&app).is_empty());
    assert_eq!(unread(&me).await, 1);
    assert_eq!(
        me.get("/members/preferences").accept_json().await.json()["channels"],
        json!([{ "topic": "shop.order_shipped", "channel": "mail", "enabled": false }])
    );
}

#[tokio::test]
async fn quiet_hours_hold_mail_until_morning_but_not_an_urgent_one() {
    let app = start().await;
    move_to(&app, &format!("{}T20:00:00Z", year_ahead()));
    let me = app.as_(&member(4));
    me.post("/members/preferences/quiet").accept_json().await;

    // 23:00 in Istanbul.
    me.post("/members/ship").accept_json().await;
    me.post("/members/sign-in-alert").accept_json().await;
    app.drain().await;
    let at_night: Vec<String> = sent_mails(&app)
        .iter()
        .map(|mail| mail.subject().to_owned())
        .collect();
    move_to(&app, &format!("{}T04:00:01Z", next_day(&year_ahead())));
    app.drain().await;

    assert_eq!(at_night, ["A new sign-in"]);
    assert_eq!(sent_mails(&app).len(), 2);
    assert_eq!(sent_mails(&app)[1].subject(), "Your order shipped");
    assert_eq!(unread(&app.as_(&member(4))).await, 2);
}

#[tokio::test]
async fn a_daily_digest_mails_what_was_held_at_the_principals_local_time() {
    let app = start().await;
    let day = year_ahead();
    move_to(&app, &format!("{day}T06:00:00Z"));
    let me = app.as_(&member(5));
    assert_eq!(
        me.post("/members/preferences/digest")
            .accept_json()
            .await
            .status,
        204
    );

    me.post("/members/ship").accept_json().await;
    me.post("/members/ship").accept_json().await;
    me.post("/members/sign-in-alert").accept_json().await;
    me.post("/members/receipt").accept_json().await;
    app.drain().await;
    let before = sent_mails(&app).len();
    // 08:00 in Istanbul is 05:00 UTC, the next day.
    move_to(&app, &format!("{}T05:00:00Z", next_day(&day)));
    app.drain().await;
    app.drain().await;

    assert_eq!(
        before, 2,
        "only the urgent and the never digested are mailed at once"
    );
    let mails = sent_mails(&app);
    assert_eq!(mails.len(), 3);
    assert_eq!(mails[2].subject(), "2 notifications");
    let schedules = app
        .query(
            "SELECT next_at FROM sofie.local_schedules WHERE key = $1",
            &[SqlValue::Text(format!(
                "sofie_notify.digest:{}",
                Uuid::from_u128(5)
            ))],
        )
        .await;
    assert_eq!(
        schedules,
        [vec![SqlValue::Timestamp(
            format!("{}T05:00:00Z", next_day(&next_day(&day)))
                .parse()
                .unwrap()
        )]]
    );
}

#[tokio::test]
async fn stopping_the_digest_mails_what_it_held() {
    let app = start().await;
    let me = app.as_(&member(9));
    me.post("/members/preferences/digest").accept_json().await;
    me.post("/members/ship").accept_json().await;
    app.drain().await;
    let held = sent_mails(&app).len();

    assert_eq!(
        me.post("/members/preferences/no-digest")
            .accept_json()
            .await
            .status,
        204
    );
    app.drain().await;

    assert_eq!(held, 0);
    let mails = sent_mails(&app);
    assert_eq!(mails.len(), 1);
    assert_eq!(mails[0].subject(), "Your order shipped");
}

#[tokio::test]
async fn a_browser_subscribes_once_and_follows_whoever_subscribed_last() {
    let app = start().await;
    let browser = registration("https://push.example.test/send/abc", BROWSER_AUTH);

    let first = app
        .as_(&member(6))
        .post("/members/push")
        .json(&browser)
        .await;
    let second = app
        .as_(&member(7))
        .post("/members/push")
        .json(&browser)
        .await;
    let refused = app
        .as_(&member(7))
        .post("/members/push")
        .json(&registration("http://push.example.test/x", BROWSER_AUTH))
        .await;

    assert_eq!(first.status, 204, "{}", first.body);
    assert_eq!(second.status, 204);
    assert_eq!(refused.status, 422);
    let rows = app
        .query("SELECT principal FROM sofie.push_subscriptions", &[])
        .await;
    assert_eq!(rows, [vec![SqlValue::Uuid(Uuid::from_u128(7))]]);
    assert!(sofie_notify::SERVICE_WORKER.contains("showNotification"));
}

const BROWSER_KEY: &str =
    "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
const BROWSER_AUTH: &str = "BTBZMqHH6r4Tts7J_aSIgg";

fn registration(endpoint: &str, auth: &str) -> serde_json::Value {
    json!({ "endpoint": endpoint, "keys": { "p256dh": BROWSER_KEY, "auth": auth } })
}

#[tokio::test]
async fn a_principal_keeps_only_its_newest_browsers_subscribed() {
    let app = start().await;
    let me = app.as_(&member(10));

    for browser in 0..25 {
        let endpoint = format!("https://push.example.test/send/{browser}");
        let subscribed = me
            .post("/members/push")
            .json(&registration(&endpoint, BROWSER_AUTH))
            .await;
        assert_eq!(subscribed.status, 204, "{}", subscribed.body);
    }

    let live = app
        .query(
            "SELECT endpoint FROM sofie.push_subscriptions WHERE gone_at IS NULL ORDER BY endpoint",
            &[],
        )
        .await;
    assert_eq!(live.len(), 20);
    assert!(
        !live
            .iter()
            .any(|row| row[0] == SqlValue::Text("https://push.example.test/send/0".to_owned()))
    );

    let let_go = app
        .query(
            "SELECT version FROM sofie.push_subscriptions WHERE gone_at IS NOT NULL",
            &[],
        )
        .await;
    assert_eq!(let_go.len(), 5);
    assert!(let_go.iter().all(|row| row[0] == SqlValue::Int(2)));
}

#[tokio::test]
async fn a_live_subscription_is_not_taken_over_without_its_browser_s_secret() {
    let app = start().await;
    let endpoint = "https://push.example.test/send/abc";
    let first = app
        .as_(&member(11))
        .post("/members/push")
        .json(&registration(endpoint, BROWSER_AUTH))
        .await;

    let taken = app
        .as_(&member(12))
        .post("/members/push")
        .json(&registration(endpoint, "AAAAAAAAAAAAAAAAAAAAAA"))
        .await;

    assert_eq!(first.status, 204);
    assert_eq!(taken.status, 409, "{}", taken.body);
    let rows = app
        .query("SELECT principal FROM sofie.push_subscriptions", &[])
        .await;
    assert_eq!(rows, [vec![SqlValue::Uuid(Uuid::from_u128(11))]]);
}

#[tokio::test]
async fn a_digest_mails_a_bounded_number_and_holds_the_rest_for_the_next() {
    let app = start().await;
    let day = year_ahead();
    move_to(&app, &format!("{day}T06:00:00Z"));
    let me = app.as_(&member(13));
    me.post("/members/preferences/digest").accept_json().await;
    for _ in 0..51 {
        me.post("/members/ship").accept_json().await;
    }
    app.drain().await;

    move_to(&app, &format!("{}T05:00:00Z", next_day(&day)));
    app.drain().await;
    app.drain().await;

    let mails = sent_mails(&app);
    assert_eq!(mails.len(), 1);
    assert_eq!(mails[0].subject(), "50 notifications");
    let held = app
        .query(
            "SELECT count(*) FROM sofie.notify_digest_items WHERE sent_at IS NULL",
            &[],
        )
        .await;
    assert_eq!(held, [vec![SqlValue::Int(1)]]);
}

/// A day well ahead of the test clock's start, so moving there only goes forward.
fn year_ahead() -> String {
    let next_year = jiff::Timestamp::now()
        .to_zoned(jiff::tz::TimeZone::UTC)
        .date()
        .year()
        + 1;
    format!("{next_year}-03-10")
}

fn next_day(day: &str) -> String {
    let date: jiff::civil::Date = day.parse().unwrap();
    date.tomorrow().unwrap().to_string()
}

#[tokio::test]
async fn a_browser_taken_over_is_a_new_subscription_the_former_principal_s_pushes_miss() {
    let app = start().await;
    let browser = registration("https://push.example.test/send/abc", BROWSER_AUTH);
    let id = || async {
        app.query("SELECT id FROM sofie.push_subscriptions", &[])
            .await
            .remove(0)
            .remove(0)
    };
    app.as_(&member(6))
        .post("/members/push")
        .json(&browser)
        .await
        .assert_status(204);
    let first = id().await;
    app.as_(&member(6))
        .post("/members/push")
        .json(&browser)
        .await
        .assert_status(204);
    let renewed = id().await;

    app.as_(&member(7))
        .post("/members/push")
        .json(&browser)
        .await
        .assert_status(204);

    assert_eq!(renewed, first, "the same principal keeps its subscription");
    assert_ne!(id().await, first);
}

#[tokio::test]
async fn a_browser_taken_over_refreshes_the_former_principal_s_screens_too() {
    let app = start().await;
    let browser = registration("https://push.example.test/send/abc", BROWSER_AUTH);
    app.as_(&member(6))
        .post("/members/push")
        .json(&browser)
        .await
        .assert_status(204);
    let mut changes = sofie_db::committed_changes(app.db());

    app.as_(&member(7))
        .post("/members/push")
        .json(&browser)
        .await
        .assert_status(204);

    let topic = sofie_db::Topic::named("sofie", "push_subscriptions");
    let mut readers = Vec::new();
    while let Ok(sofie_db::Announced::Changed(announced)) = changes.try_recv() {
        let touched = announced.iter().filter(|change| change.topic == topic);
        readers.extend(touched.map(|change| change.readers));
    }
    for principal in [6, 7] {
        let owner = sofie_db::Readers::Owner(Uuid::from_u128(principal));
        assert!(readers.contains(&owner), "{principal}: {readers:?}");
    }
}

#[tokio::test]
async fn a_digest_that_cannot_link_back_is_mailed_without_its_links() {
    let app = start().await;
    let outbox = sofie_mail::testing::fake_without_base_url(app.db());
    let me = app.as_(&member(11));
    me.post("/members/preferences/digest").accept_json().await;
    me.post("/members/ship").accept_json().await;
    app.drain().await;

    me.post("/members/preferences/no-digest")
        .accept_json()
        .await;
    app.drain().await;

    let mails = outbox.mails();
    assert_eq!(mails.len(), 1);
    assert_eq!(mails[0].subject(), "Your order shipped");
}

#[tokio::test]
async fn marking_all_read_holds_while_one_is_read_meanwhile() {
    let app = start().await;
    let me = app.as_(&member(12));
    me.post("/members/ship").accept_json().await;
    me.post("/members/ship").accept_json().await;
    app.drain().await;

    let (marked, ()) = tokio::join!(me.post("/members/inbox/read-slowly").accept_json(), async {
        MARKING_BUILT.notified().await;
        me.post("/members/inbox/read-newest")
            .accept_json()
            .await
            .assert_status(204);
        MARKING_GO.notify_one();
    });

    assert_eq!(marked.status, 204, "{}", marked.body);
    assert_eq!(unread(&me).await, 0);
}

#[tokio::test]
async fn two_first_choices_at_once_both_hold() {
    let app = start().await;
    let me = app.as_(&member(13));

    let (slow, ()) = tokio::join!(
        me.post("/members/preferences/mute-slowly").accept_json(),
        async {
            CHOICE_BUILT.notified().await;
            me.post("/members/preferences/mute")
                .accept_json()
                .await
                .assert_status(204);
            CHOICE_GO.notify_one();
        }
    );

    assert_eq!(slow.status, 204, "{}", slow.body);
    assert_eq!(
        me.get("/members/preferences").accept_json().await.json()["channels"],
        json!([{ "topic": "shop.order_shipped", "channel": "mail", "enabled": false }])
    );
}

#[tokio::test]
async fn stopping_a_digest_holding_more_than_one_mail_takes_mails_all_of_it() {
    let app = start().await;
    let me = app.as_(&member(14));
    me.post("/members/preferences/digest").accept_json().await;
    for _ in 0..51 {
        me.post("/members/ship").accept_json().await;
    }
    app.drain().await;

    me.post("/members/preferences/no-digest")
        .accept_json()
        .await;
    app.drain().await;
    app.drain().await;

    let subjects: Vec<String> = sent_mails(&app)
        .iter()
        .map(|mail| mail.subject().to_owned())
        .collect();
    assert_eq!(subjects, ["50 notifications", "Your order shipped"]);
    let held = app
        .query(
            "SELECT count(*) FROM sofie.notify_digest_items WHERE sent_at IS NULL",
            &[],
        )
        .await;
    assert_eq!(held, [vec![SqlValue::Int(0)]]);
}

#[tokio::test]
async fn a_browser_unsubscribes_only_its_own_principal_s_subscription() {
    let app = start().await;
    let endpoint = "https://push.example.test/send/abc";
    app.as_(&member(15))
        .post("/members/push")
        .json(&registration(endpoint, BROWSER_AUTH))
        .await
        .assert_status(204);
    let stop = json!({ "endpoint": endpoint });
    let live = || async {
        app.query(
            "SELECT count(*) FROM sofie.push_subscriptions WHERE gone_at IS NULL",
            &[],
        )
        .await
    };

    let other = app
        .as_(&member(16))
        .post("/members/push/stop")
        .json(&stop)
        .await;
    let still = live().await;
    let own = app
        .as_(&member(15))
        .post("/members/push/stop")
        .json(&stop)
        .await;

    assert_eq!(other.status, 204, "{}", other.body);
    assert_eq!(still, [vec![SqlValue::Int(1)]]);
    assert_eq!(own.status, 204, "{}", own.body);
    assert_eq!(live().await, [vec![SqlValue::Int(0)]]);
}

#[tokio::test]
async fn read_inbox_items_and_mailed_digests_are_pruned_after_ninety_days() {
    let app = start().await;
    let reader = app.as_(&member(30));
    reader.post("/members/ship").accept_json().await;
    app.drain().await;
    reader
        .post("/members/inbox/read-newest")
        .accept_json()
        .await;
    reader.post("/members/ship").accept_json().await;
    let mailed = app.as_(&member(31));
    mailed
        .post("/members/preferences/digest")
        .accept_json()
        .await;
    mailed.post("/members/ship").accept_json().await;
    app.drain().await;
    mailed
        .post("/members/preferences/no-digest")
        .accept_json()
        .await;
    let held = app.as_(&member(32));
    held.post("/members/preferences/digest").accept_json().await;
    held.post("/members/ship").accept_json().await;
    app.drain().await;

    app.clock().advance(Duration::from_hours(24 * 91)).unwrap();
    app.drain().await;

    let inbox = app
        .as_(&member(30))
        .get("/members/inbox")
        .accept_json()
        .await
        .json();
    assert_eq!(inbox["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(inbox["unread"], 1);
    let digested = app
        .query("SELECT principal FROM sofie.notify_digest_items", &[])
        .await;
    assert_eq!(digested, [vec![SqlValue::Uuid(Uuid::from_u128(32))]]);
}

#[tokio::test]
async fn a_forgotten_principal_keeps_no_rows_and_gets_no_digest() {
    let app = start().await;
    let day = year_ahead();
    move_to(&app, &format!("{day}T06:00:00Z"));
    let other = app.as_(&member(41));
    other.post("/members/ship").accept_json().await;
    let me = app.as_(&member(40));
    me.post("/members/preferences/digest").accept_json().await;
    me.post("/members/ship").accept_json().await;
    app.drain().await;
    me.post("/members/preferences/mute").accept_json().await;
    me.post("/members/push")
        .json(&registration(
            "https://push.example.test/send/40",
            BROWSER_AUTH,
        ))
        .await;
    let mailed = sent_mails(&app).len();
    let exported = me.get("/members/personal-data").accept_json().await.json();

    assert_eq!(me.post("/members/forget").accept_json().await.status, 204);
    move_to(&app, &format!("{}T05:00:00Z", next_day(&day)));
    app.drain().await;

    assert_eq!(exported["inbox"][0]["title"], "Your order shipped");
    assert_eq!(exported["held_for_digest"][0]["email"], "ada@example.test");
    assert_eq!(exported["settings"]["digest"], "daily 08:00");
    assert_eq!(exported["channels"][0]["enabled"], false);
    assert_eq!(exported["subscribed_browsers"], 1);
    assert_eq!(sent_mails(&app).len(), mailed, "no digest after forgetting");
    for table in [
        "notify_settings",
        "notify_choices",
        "inbox",
        "notify_digest_items",
        "push_subscriptions",
    ] {
        let rows = app
            .query(
                &format!("SELECT principal FROM sofie.{table} WHERE principal = $1"),
                &[SqlValue::Uuid(Uuid::from_u128(40))],
            )
            .await;
        assert!(rows.is_empty(), "{table} keeps {rows:?}");
    }
    let schedules = app
        .query(
            "SELECT next_at FROM sofie.local_schedules WHERE key = $1",
            &[SqlValue::Text(format!(
                "sofie_notify.digest:{}",
                Uuid::from_u128(40)
            ))],
        )
        .await;
    assert_eq!(schedules, [vec![SqlValue::Null]]);
    // A day on, the session `other` signed in with has expired.
    assert_eq!(unread(&app.as_(&member(41))).await, 1);
}

#[tokio::test]
async fn deliveries_queued_before_forgetting_write_and_mail_nothing_for_the_principal() {
    let app = start().await;
    move_to(&app, &format!("{}T20:00:00Z", year_ahead()));
    let me = app.as_(&member(43));
    me.post("/members/preferences/quiet").accept_json().await;
    me.post("/members/ship").accept_json().await;
    app.drain().await;
    me.post("/members/ship").accept_json().await;

    assert_eq!(me.post("/members/forget").accept_json().await.status, 204);
    move_to(&app, &format!("{}T04:00:01Z", next_day(&year_ahead())));
    app.drain().await;

    assert!(sent_mails(&app).is_empty());
    let inbox = app
        .query(
            "SELECT principal FROM sofie.inbox WHERE principal = $1",
            &[SqlValue::Uuid(Uuid::from_u128(43))],
        )
        .await;
    assert!(inbox.is_empty(), "{inbox:?}");
    let left = app.query("SELECT name FROM sofie.messages", &[]).await;
    assert!(left.is_empty(), "{left:?}");
}

#[tokio::test]
async fn a_digest_mails_each_address_only_what_was_held_for_it() {
    let app = start().await;
    let day = year_ahead();
    move_to(&app, &format!("{day}T06:00:00Z"));
    let me = app.as_(&member(42));
    me.post("/members/preferences/digest").accept_json().await;
    me.post("/members/ship").accept_json().await;
    me.post("/members/ship-to-work").accept_json().await;
    app.drain().await;

    move_to(&app, &format!("{}T05:00:00Z", next_day(&day)));
    app.drain().await;
    app.drain().await;

    let mut mailed: Vec<(String, String)> = sent_mails(&app)
        .iter()
        .map(|mail| {
            (
                mail.to()[0].email().as_str().to_owned(),
                mail.subject().to_owned(),
            )
        })
        .collect();
    mailed.sort();
    assert_eq!(
        mailed,
        [
            (
                "ada@example.test".to_owned(),
                "Your order shipped".to_owned()
            ),
            (
                "ada@work.example.test".to_owned(),
                "Your order shipped to work".to_owned()
            ),
        ]
    );
}
