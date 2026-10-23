#![cfg(feature = "feature-flags")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use sofie::core::Result;
use sofie::db::Db;
use sofie::flags::{Feature, Flags, Rule};
use sofie::http::{App, Gates, RouteFeature, Routes, Web};
use sofie::worker::Worker;
use sofie_testing::TestApp;

const SPLIT: Feature = Feature::new("shop.split");
const MINUTE: Duration = Duration::from_secs(60);

static TICKS: AtomicUsize = AtomicUsize::new(0);

#[sofie::feature(SPLIT)]
async fn split() -> Result<()> {
    Ok(())
}

#[sofie::feature(SPLIT)]
async fn tick() -> Result<()> {
    TICKS.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

const SHOP: sofie::Module = sofie::Module {
    tier: sofie::Tier::Core,
    settings: &[sofie::Setting {
        name: "free_shipping_above",
        default: sofie::SettingValue::Number(5000),
    }],
    ..sofie::Module::new("shop")
};

async fn shipping_page(settings: sofie::RuntimeSettings) -> Result<()> {
    settings.number(&SHOP, "free_shipping_above").map(drop)
}

fn routes(routes: &mut Routes<Web>) {
    routes.get("/split", split);
    routes.get("/free-shipping", shipping_page);
}

const ROUTE_FEATURES: &[RouteFeature] = &[RouteFeature {
    method: "GET",
    path: "/split",
    feature: SPLIT,
}];

#[expect(
    clippy::unnecessary_wraps,
    reason = "the shape `sofie gen` writes, where loading a module's config can fail"
)]
fn app(db: Db, flags: Flags) -> Result<App> {
    Ok(App::new(db)
        .gates(
            Gates::default()
                .features(flags)
                .route_features(ROUTE_FEATURES),
        )
        .area::<Web>("shop", routes))
}

fn worker(db: Db, flags: &Flags) -> Worker {
    Worker::new(db).every("shop.tick", MINUTE, sofie::flags::gated(flags, SPLIT, tick))
}

async fn started() -> (TestApp, Flags) {
    let shared: Arc<OnceLock<Flags>> = Arc::default();
    let for_app = Arc::clone(&shared);
    let app = sofie_testing::start(
        |db: Db| async move {
            db.schema().migrate(&[]).await?;
            let flags = Flags::start(&db, "test").await?;
            for_app.get_or_init(|| flags.clone());
            app(db, flags)
        },
        |db| worker(db, shared.get().expect("the app starts the flags first")),
    )
    .await;
    let flags = shared.get().expect("the app started the flags").clone();
    (app, flags)
}

#[tokio::test]
async fn a_flag_hides_its_route_and_skips_its_schedule_until_switched_on() {
    let (app, flags) = started().await;

    assert_eq!(app.get("/split").accept_json().await.status, 404);
    app.drain().await;
    assert_eq!(TICKS.load(Ordering::SeqCst), 0);

    flags.set(SPLIT.name(), Rule::everyone()).await.unwrap();
    app.clock().advance(MINUTE).unwrap();
    app.drain().await;

    assert_eq!(app.get("/split").accept_json().await.status, 204);
    assert_eq!(TICKS.load(Ordering::SeqCst), 1);
}

#[test]
fn the_wired_app_and_worker_fit_run_with_flags() {
    let main = || sofie::run_with_flags(&[], async |db, flags| app(db, flags), worker);
    let _: fn() -> Result<()> = main;
}

#[tokio::test]
async fn a_setting_is_read_as_its_declared_kind() {
    let (app, flags) = started().await;
    assert_eq!(app.get("/free-shipping").accept_json().await.status, 204);
    let held = || sofie::RuntimeSettings::from(flags.evaluate(sofie::flags::Subject::default()));
    assert_eq!(held().number(&SHOP, "free_shipping_above").unwrap(), 5000);

    sofie::declare_settings(&flags, &[SHOP]).unwrap();
    flags
        .change_setting("shop.free_shipping_above", "7500")
        .await
        .unwrap();

    assert_eq!(held().number(&SHOP, "free_shipping_above").unwrap(), 7500);
    assert!(held().switch(&SHOP, "free_shipping_above").is_err());
    assert!(held().number(&SHOP, "unknown").is_err());
}
