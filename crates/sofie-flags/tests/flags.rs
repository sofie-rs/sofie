use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use sofie_core::{Error, Result, SqlValue, Uuid};
use sofie_db::{BoxFuture, Call, Db, Dialect, Driver, RowSink, Step, Wakeups};
use sofie_flags::{DeclaredSetting, Feature, Flags, Rule, SettingValue, Subject, Variants, gated};
use sofie_postgres::Postgres;
use sofie_testing::TestDatabase;

const SPLIT_PAYMENTS: Feature = Feature::new("order.split_payments");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layout {
    Classic,
    Compact,
}

impl sofie_core::Variants for Layout {
    const TYPE: &'static str = "Layout";
    const VARIANTS: &'static [(&'static str, Self)] =
        &[("CLASSIC", Self::Classic), ("COMPACT", Self::Compact)];
}

const LAYOUT: Variants<Layout> = Variants::new("order.checkout_layout");

async fn migrated(database: &TestDatabase) -> Db {
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    db.schema().migrate(&[]).await.unwrap();
    db
}

fn member(principal: Uuid) -> Subject {
    Subject {
        area: Some("member"),
        principal: Some(principal),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_flag_nobody_switched_is_off_and_a_stored_rule_is_read_back() {
    let database = TestDatabase::create().await;
    let db = migrated(&database).await;
    let flags = Flags::start(&db, "production").await.unwrap();
    let anyone = member(Uuid::from_u128(1));
    assert!(!flags.evaluate(anyone).enabled(SPLIT_PAYMENTS));

    flags
        .set("order.split_payments", Rule::everyone())
        .await
        .unwrap();

    let features = flags.evaluate(anyone);
    assert!(features.enabled(SPLIT_PAYMENTS));
    assert_eq!(features.evaluated(), "order.split_payments=on");
    let restarted = Flags::start(&db, "production").await.unwrap();
    assert_eq!(
        restarted.rules(),
        vec![("order.split_payments".to_owned(), Rule::everyone())]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_malformed_stored_flag_is_left_out_and_the_others_still_load() {
    let database = TestDatabase::create().await;
    let db = migrated(&database).await;
    Flags::start(&db, "production")
        .await
        .unwrap()
        .set("order.split_payments", Rule::everyone())
        .await
        .unwrap();
    let raw = Postgres::connect(database.url()).unwrap();
    sofie_db::query_values(
        &raw,
        "INSERT INTO sofie.flags (id, version, name, rule, changed_at) \
         VALUES (gen_random_uuid(), 1, 'order.broken', '{\"on\": \"yes\"}', now())",
        &[],
    )
    .await
    .unwrap();

    let restarted = Flags::start(&db, "production").await.unwrap();

    assert_eq!(
        restarted.rules(),
        vec![("order.split_payments".to_owned(), Rule::everyone())]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_change_made_right_after_another_process_started_reaches_it_without_waiting_for_expiry() {
    let database = TestDatabase::create().await;
    let db = migrated(&database).await;
    let here = Flags::start(&db, "production").await.unwrap();
    let elsewhere = Flags::start(&migrated(&database).await, "production")
        .await
        .unwrap();
    here.set("order.split_payments", Rule::everyone())
        .await
        .unwrap();

    let anyone = member(Uuid::from_u128(1));
    for _ in 0..50 {
        if elsewhere.evaluate(anyone).enabled(SPLIT_PAYMENTS) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the change did not reach the other process within five seconds");
}

fn free_shipping() -> Vec<DeclaredSetting> {
    vec![DeclaredSetting {
        name: "order.free_shipping_above".to_owned(),
        default: SettingValue::Number(5000),
    }]
}

#[tokio::test(flavor = "multi_thread")]
async fn a_setting_changed_here_is_held_by_another_instance() {
    let database = TestDatabase::create().await;
    let here = Flags::start(&migrated(&database).await, "production")
        .await
        .unwrap();
    let elsewhere = Flags::start(&migrated(&database).await, "production")
        .await
        .unwrap();
    here.declare_settings(free_shipping()).unwrap();
    let anyone = Subject::default();
    let before = here.evaluate(anyone).version();
    assert_eq!(
        here.evaluate(anyone).settings()[0].value,
        SettingValue::Number(5000)
    );

    here.change_setting("order.free_shipping_above", " 7500")
        .await
        .unwrap();

    assert_ne!(here.evaluate(anyone).version(), before);
    let default = || SettingValue::Number(5000);
    for _ in 0..50 {
        let held = elsewhere
            .evaluate(anyone)
            .setting("order.free_shipping_above", default());
        if held == SettingValue::Number(7500) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the change did not reach the other instance within five seconds");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_setting_is_changed_only_when_declared_and_only_to_its_kind() {
    let database = TestDatabase::create().await;
    let flags = Flags::start(&migrated(&database).await, "production")
        .await
        .unwrap();
    flags.declare_settings(free_shipping()).unwrap();
    for (name, value) in [
        ("order.free_shipping_above", "lots"),
        ("order.unknown", "1"),
    ] {
        assert!(matches!(
            flags.change_setting(name, value).await,
            Err(Error::Invalid(_))
        ));
    }
    let twice = [free_shipping(), free_shipping()].concat();
    assert!(flags.declare_settings(twice).is_err());
    assert_eq!(
        flags.evaluate(Subject::default()).setting(
            "order.free_shipping_above",
            SettingValue::Text(String::new())
        ),
        SettingValue::Text(String::new()),
        "nothing stored keeps the default"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn targeting_and_variants_follow_the_stored_rule() {
    let database = TestDatabase::create().await;
    let flags = Flags::start(&migrated(&database).await, "staging")
        .await
        .unwrap();
    let chosen = Uuid::from_u128(42);
    let rule = Rule {
        environments: vec!["staging".to_owned()],
        principals: vec![chosen],
        percentage: 0,
        variants: vec![("COMPACT".to_owned(), 1)],
        ..Rule::everyone()
    };
    flags.set("order.checkout_layout", rule).await.unwrap();

    assert_eq!(
        flags.evaluate(member(chosen)).variant(LAYOUT),
        Some(Layout::Compact)
    );
    assert_eq!(
        flags.evaluate(member(Uuid::from_u128(7))).variant(LAYOUT),
        None
    );
    let production = Flags::start(&migrated(&database).await, "production")
        .await
        .unwrap();
    assert_eq!(production.evaluate(member(chosen)).variant(LAYOUT), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_variant_flag_nobody_switched_is_recorded_as_off() {
    let database = TestDatabase::create().await;
    let flags = Flags::start(&migrated(&database).await, "production")
        .await
        .unwrap();
    let features = flags.evaluate(member(Uuid::from_u128(1)));

    assert_eq!(features.variant(LAYOUT), None);
    assert_eq!(features.evaluated(), "order.checkout_layout=off");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_malformed_name_or_percentage_is_refused() {
    let database = TestDatabase::create().await;
    let flags = Flags::start(&migrated(&database).await, "production")
        .await
        .unwrap();
    assert!(flags.set("split_payments", Rule::everyone()).await.is_err());
    let too_many = Rule {
        percentage: 101,
        ..Rule::everyone()
    };
    assert!(flags.set("order.split_payments", too_many).await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_settled_flag_is_listed_with_the_time_it_settled() {
    let database = TestDatabase::create().await;
    let flags = Flags::start(&migrated(&database).await, "production")
        .await
        .unwrap();
    flags
        .set("order.split_payments", Rule::everyone())
        .await
        .unwrap();
    let half = Rule {
        percentage: 50,
        ..Rule::everyone()
    };
    flags.set("order.checkout_layout", half).await.unwrap();

    let settled = flags.settled();
    assert_eq!(settled.len(), 1);
    assert_eq!(settled[0].name, "order.split_payments");
    assert!(settled[0].on);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_gated_job_runs_only_while_its_feature_is_on() {
    let database = TestDatabase::create().await;
    let flags = Flags::start(&migrated(&database).await, "production")
        .await
        .unwrap();
    let ran = Arc::new(AtomicBool::new(false));
    let job = {
        let ran = Arc::clone(&ran);
        move || {
            let ran = Arc::clone(&ran);
            async move {
                ran.store(true, Ordering::SeqCst);
                Result::Ok(())
            }
        }
    };
    let gated_job = gated(&flags, SPLIT_PAYMENTS, job);

    Call::<(), ()>::call(&gated_job, &mut ()).await.unwrap();
    assert!(!ran.load(Ordering::SeqCst));

    flags
        .set("order.split_payments", Rule::everyone())
        .await
        .unwrap();
    Call::<(), ()>::call(&gated_job, &mut ()).await.unwrap();
    assert!(ran.load(Ordering::SeqCst));
}

/// A change lands while the next listener subscribes, so no wake-up tells of it.
struct Interrupted {
    postgres: Postgres,
    elsewhere: Db,
    subscriptions: AtomicUsize,
}

struct Failing;

impl Wakeups for Failing {
    fn next(&mut self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async { Err(Error::internal_message("the listening connection dropped")) })
    }
}

struct Silent;

impl Wakeups for Silent {
    fn next(&mut self) -> BoxFuture<'_, Result<()>> {
        Box::pin(std::future::pending())
    }
}

impl Driver for Interrupted {
    fn dialect(&self) -> &dyn Dialect {
        self.postgres.dialect()
    }

    fn query<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [SqlValue],
        rows: &'a mut dyn RowSink,
    ) -> BoxFuture<'a, Result<()>> {
        self.postgres.query(sql, params, rows)
    }

    fn transaction(&self, steps: Vec<Step>) -> BoxFuture<'_, Result<()>> {
        self.postgres.transaction(steps)
    }

    fn wakeups(&self) -> Option<BoxFuture<'_, Result<Box<dyn Wakeups>>>> {
        let subscription = self.subscriptions.fetch_add(1, Ordering::SeqCst);
        Some(Box::pin(async move {
            if subscription == 0 {
                return Ok(Box::new(Failing) as Box<dyn Wakeups>);
            }
            if subscription == 1 {
                let elsewhere = Flags::start(&self.elsewhere, "production").await?;
                elsewhere
                    .set("order.split_payments", Rule::everyone())
                    .await?;
            }
            Ok(Box::new(Silent) as Box<dyn Wakeups>)
        }))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_change_made_while_listening_resumed_is_read_without_waiting_for_expiry() {
    let database = TestDatabase::create().await;
    let interrupted = Interrupted {
        postgres: Postgres::connect(database.url()).unwrap(),
        elsewhere: migrated(&database).await,
        subscriptions: AtomicUsize::new(0),
    };
    let flags = Flags::start(&Db::new(interrupted), "production")
        .await
        .unwrap();

    let anyone = member(Uuid::from_u128(1));
    for _ in 0..50 {
        if flags.evaluate(anyone).enabled(SPLIT_PAYMENTS) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the change did not reach the process within five seconds");
}

/// Every subscription drops at once, as behind a proxy that closes listening connections.
struct Dropping {
    postgres: Postgres,
    subscriptions: Arc<AtomicUsize>,
}

impl Driver for Dropping {
    fn dialect(&self) -> &dyn Dialect {
        self.postgres.dialect()
    }

    fn query<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [SqlValue],
        rows: &'a mut dyn RowSink,
    ) -> BoxFuture<'a, Result<()>> {
        self.postgres.query(sql, params, rows)
    }

    fn transaction(&self, steps: Vec<Step>) -> BoxFuture<'_, Result<()>> {
        self.postgres.transaction(steps)
    }

    fn wakeups(&self) -> Option<BoxFuture<'_, Result<Box<dyn Wakeups>>>> {
        self.subscriptions.fetch_add(1, Ordering::SeqCst);
        Some(Box::pin(async {
            Ok(Box::new(Failing) as Box<dyn Wakeups>)
        }))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_listener_that_keeps_dropping_is_not_resubscribed_in_a_tight_loop() {
    let database = TestDatabase::create().await;
    let subscriptions = Arc::new(AtomicUsize::new(0));
    let dropping = Dropping {
        postgres: Postgres::connect(database.url()).unwrap(),
        subscriptions: Arc::clone(&subscriptions),
    };
    migrated(&database).await;
    let _flags = Flags::start(&Db::new(dropping), "production")
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_secs(1)).await;

    let subscribed = subscriptions.load(Ordering::SeqCst);
    assert!(
        subscribed <= 10,
        "subscribed {subscribed} times in a second"
    );
}
