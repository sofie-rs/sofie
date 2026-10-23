use serde_json::json;
use sofie::db::Db;
use sofie::http::App;
use sofie_testing::TestApp;

const SCREEN: &str = "application/vnd.sofie.screen+json";

async fn demo() -> TestApp {
    sofie_testing::start(migrated, clients_demo::worker).await
}

async fn migrated(db: Db) -> sofie::core::Result<App> {
    db.schema()
        .migrate(&sofie::http::migrations(clients_demo::MODULES))
        .await?;
    clients_demo::app(db).await
}

#[tokio::test]
async fn the_svelte_area_answers_its_shell_its_bundle_and_envelopes() {
    let demo = demo().await;

    let page = demo.get("/board/notes").header("accept", "text/html").await;

    page.assert_ok();
    assert!(
        page.body.contains("<div id=\"sofie-app\"></div>"),
        "{}",
        page.body
    );
    let script = page
        .body
        .split("<script type=\"module\" src=\"")
        .nth(1)
        .unwrap();
    let bundle = demo.get(&script[..script.find('"').unwrap()]).await;
    bundle.assert_ok();
    assert!(
        bundle
            .header("content-type")
            .starts_with("application/javascript")
    );
    let written = demo
        .post("/board/notes")
        .form(&[
            ("title", "First"),
            ("body", "From the runtime"),
            ("price.amount", "12.50"),
            ("price.currency", "EUR"),
        ])
        .header("accept", SCREEN)
        .header("sec-fetch-site", "same-origin")
        .await;
    written.assert_ok();
    assert_eq!(written.json()["screen"], "notes/board/notes");
    assert_eq!(written.json()["data"][0]["title"], "First");
    assert_eq!(
        written.json()["data"][0]["price"],
        json!({ "minor": 1250, "currency": "EUR" })
    );
}

#[tokio::test]
async fn the_html_area_keeps_serving_pages() {
    let demo = demo().await;

    let page = demo.get("/desk/notes").header("accept", "text/html").await;

    page.assert_ok();
    assert!(
        page.body.contains("<form id=\"write-note\""),
        "{}",
        page.body
    );
    assert!(!page.body.contains("sofie-app"), "{}", page.body);
}
