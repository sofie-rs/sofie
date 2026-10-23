//! Every test captures telemetry, since a span's interest is cached per callsite.
#![cfg(feature = "flags")]

use std::time::Duration;

use sofie_core::{Error, Result};
use sofie_db::Db;
use sofie_flags::{Feature, Features, Flags, Rule, Variants};
use sofie_http::{App, Gates, RouteFeature, Routes, Web};
use sofie_testing::TestApp;

const SPLIT_PAYMENTS: Feature = Feature::new("shop.split_payments");
const BETA: Feature = Feature::new("shop.beta");
const CHECKOUT_LAYOUT: Variants<Layout> = Variants::new("shop.checkout_layout");

#[derive(Clone, Copy)]
enum Layout {
    Classic,
    Compact,
}

impl sofie_core::Variants for Layout {
    const TYPE: &'static str = "Layout";
    const VARIANTS: &'static [(&'static str, Self)] =
        &[("CLASSIC", Self::Classic), ("COMPACT", Self::Compact)];
}

const ROUTE_FEATURES: &[RouteFeature] = &[RouteFeature {
    method: "GET",
    path: "/beta",
    feature: BETA,
}];

async fn ok() -> Result<()> {
    Ok(())
}

async fn layout(features: Features) -> Result<()> {
    if features.enabled(SPLIT_PAYMENTS) {
        return Ok(());
    }
    Err(Error::Rule("payments are not split".to_owned()))
}

async fn checkout(features: Features) -> Result<()> {
    sofie_telemetry::product_event!(checkout.opened);
    let layout = features.variant(CHECKOUT_LAYOUT);
    sofie_telemetry::product_event!(order.placed);
    layout
        .map(|_| ())
        .ok_or_else(|| Error::Rule("no layout is chosen yet".to_owned()))
}

async fn browse() -> Result<()> {
    sofie_telemetry::product_event!(page.viewed);
    Ok(())
}

async fn listed() -> Result<Vec<()>> {
    Ok(Vec::new())
}

fn routes(routes: &mut Routes<Web>) {
    routes.get("/listed", listed);
    routes.post("/pay", ok).feature(SPLIT_PAYMENTS);
    routes.get("/beta", ok);
    routes.get("/layout", layout);
    routes.get("/checkout", checkout);
    routes.get("/browse", browse);
}

async fn started() -> TestApp {
    sofie_testing::start(
        |db: Db| async move {
            db.schema().migrate(&[]).await?;
            let flags = Flags::start(&db, "test").await?;
            let gates = Gates::default()
                .features(flags)
                .route_features(ROUTE_FEATURES);
            Ok(App::new(db).gates(gates).area::<Web>("shop", routes))
        },
        sofie_db::worker::Worker::new,
    )
    .await
}

/// The switcher exits at once, or the test database's drop waits on its half-open connection.
async fn switch_on(app: &TestApp, name: &str, path: &str, post: bool) {
    switch(app, name, Rule::everyone(), path, post).await;
}

async fn switch(app: &TestApp, name: &str, rule: Rule, path: &str, post: bool) {
    let elsewhere = Flags::start(app.db(), "test").await.unwrap();
    elsewhere.set(name, rule).await.unwrap();
    drop(elsewhere);
    for _ in 0..50 {
        let request = if post { app.post(path) } else { app.get(path) };
        if request.accept_json().await.status == 204 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{name} did not reach the app within five seconds");
}

#[tokio::test]
async fn a_route_behind_an_off_flag_is_not_found_until_it_is_switched_on() {
    let _telemetry = sofie_telemetry::capture();
    let app = started().await;
    assert_eq!(app.post("/pay").accept_json().await.status, 404);
    assert_eq!(app.get("/beta").accept_json().await.status, 404);

    switch_on(&app, "shop.split_payments", "/pay", true).await;
    switch_on(&app, "shop.beta", "/beta", false).await;

    assert_eq!(app.post("/pay").accept_json().await.status, 204);
    assert_eq!(app.get("/beta").accept_json().await.status, 204);
}

#[tokio::test]
async fn another_method_on_a_flagged_route_is_not_found_rather_than_naming_it() {
    let _telemetry = sofie_telemetry::capture();
    let app = started().await;

    assert_eq!(app.get("/pay").accept_json().await.status, 404);
    assert_eq!(app.post("/beta").accept_json().await.status, 404);
}

/// Flags change an answer without a row read, so switching one changes every tag.
#[cfg(not(feature = "development"))]
#[tokio::test]
async fn a_switched_flag_changes_the_tag_a_browser_holds() {
    let _telemetry = sofie_telemetry::capture();
    let app = started().await;
    let first = app.get("/listed").accept_json().await;
    let tag = first.header("etag").to_owned();
    let held = app
        .get("/listed")
        .header("if-none-match", &tag)
        .accept_json()
        .await;
    assert_eq!(held.status, 304);

    switch_on(&app, "shop.beta", "/beta", false).await;

    let after = app
        .get("/listed")
        .header("if-none-match", &tag)
        .accept_json()
        .await;
    assert_eq!(after.status, 200);
    assert_ne!(after.header("etag"), tag);
}

#[tokio::test]
async fn a_handler_reads_the_flags_and_the_span_names_those_evaluated() {
    let telemetry = sofie_telemetry::capture();
    let app = started().await;
    assert_eq!(app.get("/layout").accept_json().await.status, 422);

    switch_on(&app, "shop.split_payments", "/pay", true).await;

    assert_eq!(app.get("/layout").accept_json().await.status, 204);
    let recorded: Vec<String> = telemetry
        .spans()
        .into_iter()
        .filter(|span| span.name == "request")
        .filter_map(|span| span.fields.get("feature_flags").cloned())
        .collect();
    assert!(
        recorded.contains(&"shop.split_payments=off".to_owned())
            && recorded.contains(&"shop.split_payments=on".to_owned()),
        "{recorded:?}"
    );
}

#[tokio::test]
async fn a_product_event_carries_the_variants_evaluated_before_it() {
    let telemetry = sofie_telemetry::capture();
    let app = started().await;
    let compact = Rule {
        variants: vec![("COMPACT".to_owned(), 1)],
        ..Rule::everyone()
    };
    switch(&app, "shop.checkout_layout", compact, "/checkout", false).await;
    assert_eq!(app.get("/browse").accept_json().await.status, 204);

    let carried = |name: &str| {
        telemetry
            .product_events()
            .into_iter()
            .filter(|event| event.fields.get("product.event").map(String::as_str) == Some(name))
            .map(|event| event.fields.get("feature_flags").cloned())
            .next_back()
            .flatten()
    };
    assert_eq!(
        carried("order.placed").as_deref(),
        Some("shop.checkout_layout=COMPACT")
    );
    assert_eq!(carried("checkout.opened"), None);
    assert_eq!(carried("page.viewed"), None);
}

#[tokio::test]
async fn a_flagged_route_without_the_apps_flags_stops_the_app() {
    let db = Db::new(sofie_testing::FakeDriver::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let served = App::new(db)
        .area::<Web>("shop", routes)
        .serving(|serving| serving.signing_keys(sofie_http::SigningKeys::generate().unwrap()))
        .serve(listener, async {})
        .await;
    let error = served.expect_err("the app does not start");
    assert!(error.to_string().contains("POST /pay"), "{error}");
}

#[cfg(all(feature = "toolbar", debug_assertions))]
#[tokio::test]
async fn a_toolbar_override_answers_a_flag_for_this_browser_only() {
    let _telemetry = sofie_telemetry::capture();
    let app = started().await;
    let overriding = |flag: &'static str, answer: &'static str| {
        app.post("/_sofie/toolbar")
            .header("sec-fetch-site", "same-origin")
            .form(&[("action", "flag"), ("flag", flag), ("answer", answer)])
    };
    let cookie_of = |answer: sofie_testing::TestResponse| {
        let set = answer.header("set-cookie");
        set.split(';').next().unwrap().to_owned()
    };
    let split = cookie_of(overriding("shop.split_payments", "on").await);
    let beta = cookie_of(overriding("shop.beta", "on").await);
    let compact = cookie_of(overriding("shop.checkout_layout", "COMPACT").await);
    let get = |path: &str, cookie: &str| app.get(path).header("cookie", cookie).accept_json();

    let layout = get("/layout", &split).await;
    let beta = get("/beta", &beta).await;
    let compact = get("/checkout", &compact).await;
    let stored = app.get("/layout").accept_json().await;

    layout.assert_status(204);
    beta.assert_status(204);
    compact.assert_status(204);
    stored.assert_status(422);
}

#[cfg(feature = "parse")]
mod refreshed_pages {
    use serde_json::{Value, json};
    use sofie_core::{Error, Result};
    use sofie_db::{Db, Outcome, ReadOnly, Reply};
    use sofie_flags::Flags;
    use sofie_http::{App, Gates, Routes, Web};
    use sofie_testing::TestApp;

    use super::BETA;

    const SECRET: &str =
        r#"<Section title="Secret"><Table id="notes" of={data} columns="text" /></Section>"#;

    struct Listing(Value);

    impl Outcome for Listing {
        fn reply(self) -> Result<Reply> {
            Ok(Reply::Json(self.0))
        }
    }

    impl ReadOnly for Listing {}

    async fn secret_notes() -> Result<Listing> {
        Ok(Listing(json!([{ "text": "classified" }])))
    }

    async fn noted() -> Result<()> {
        Ok(())
    }

    async fn refused() -> Result<()> {
        Err(Error::invalid("text", "is required"))
    }

    fn routes(routes: &mut Routes<Web>) {
        routes
            .screen("/secret", "secret", secret_notes)
            .feature(BETA);
        routes
            .post("/secret/note", noted)
            .then_screen("secret", "notes");
        routes.post("/secret", refused);
    }

    async fn started() -> TestApp {
        sofie_testing::start(
            |db: Db| async move {
                db.schema().migrate(&[]).await?;
                let flags = Flags::start(&db, "test").await?;
                let screens = sofie_http::screens_from_sources(&[("shop/web/secret", SECRET)])?;
                Ok(App::new(db)
                    .screens(screens)
                    .gates(Gates::default().features(flags))
                    .area::<Web>("shop", routes))
            },
            sofie_db::worker::Worker::new,
        )
        .await
    }

    #[tokio::test]
    async fn an_action_shows_no_page_whose_own_feature_is_off() {
        let _telemetry = sofie_telemetry::capture();
        let app = started().await;

        let page = app.get("/secret").header("accept", "text/html").await;
        let refreshed = app
            .post("/secret/note")
            .header("sec-fetch-site", "same-origin")
            .htmx("notes")
            .await;
        let refused = app
            .post("/secret")
            .header("sec-fetch-site", "same-origin")
            .header("accept", "text/html")
            .form(&[("text", "")])
            .await;

        page.assert_status(404);
        assert!(!refreshed.body.contains("classified"), "{}", refreshed.body);
        assert!(!refused.body.contains("classified"), "{}", refused.body);
    }
}
