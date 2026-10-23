use serde_json::json;
use sofie::db::Db;
use sofie::http::App;
use sofie_testing::TestApp;

async fn demo() -> TestApp {
    sofie_testing::start(migrated, islands_demo::worker).await
}

async fn migrated(db: Db) -> sofie::core::Result<App> {
    db.schema()
        .migrate(&sofie::http::migrations(islands_demo::MODULES))
        .await?;
    islands_demo::app(db).await
}

fn attribute<'h>(html: &'h str, marker: &str, name: &str) -> &'h str {
    let tag = &html[html.find(marker).unwrap()..];
    let value = &tag[tag.find(&format!("{name}=\"")).unwrap() + name.len() + 2..];
    &value[..value.find('"').unwrap()]
}

#[tokio::test]
async fn an_island_page_mounts_the_island_with_its_loaders_data() {
    let demo = demo().await;
    let note = json!({ "title": "</script><script>alert(1)</script>", "body": "two words" });
    demo.post("/notes").json(&note).await.assert_ok();

    let page = demo.get("/").await;

    page.assert_ok();
    assert!(
        page.body.contains(
            "<note-editor><script type=\"application/json\">{\"body\":\"two words\",\"title\":\
             \"\\u003c/script\\u003e\\u003cscript\\u003ealert(1)\\u003c/script\\u003e\"}</script>\
             </note-editor>"
        ),
        "{}",
        page.body
    );
    let nonce = attribute(&page.body, "<script type=\"module\"", "nonce");
    assert!(
        page.header("content-security-policy")
            .contains(&format!("script-src 'self' 'nonce-{nonce}'")),
        "{}",
        page.header("content-security-policy")
    );
    let as_json = demo.get("/").accept_json().await;
    assert_eq!(as_json.json(), note);
}

#[tokio::test]
async fn the_areas_bundle_is_served_for_good_and_loaded_by_its_screens() {
    let demo = demo().await;

    let summary = demo.get("/summary").await;

    summary.assert_ok();
    assert!(
        summary
            .body
            .contains("<word-count><script type=\"application/json\">"),
        "{}",
        summary.body
    );
    let script = attribute(&summary.body, "<script type=\"module\"", "src");
    let bundle = demo.get(script).await;
    bundle.assert_ok();
    assert_eq!(
        bundle.header("content-type"),
        "application/javascript; charset=utf-8"
    );
    assert!(bundle.header("cache-control").contains("immutable"));
    assert!(bundle.body.contains("word-count") && bundle.body.contains("note-editor"));
    let stylesheet = attribute(
        &summary.body,
        "<link rel=\"stylesheet\" href=\"/_sofie/web-",
        "href",
    );
    let styles = demo.get(stylesheet).await;
    assert!(styles.body.contains(".note-editor"), "{}", styles.body);
}

#[tokio::test]
async fn the_bundle_is_answered_with_the_copy_its_build_compressed() {
    let demo = demo().await;
    let summary = demo.get("/summary").await;
    let script = attribute(&summary.body, "<script type=\"module\"", "src");

    let bundle = demo.get(script).header("accept-encoding", "br").await;

    bundle.assert_ok();
    assert_eq!(bundle.header("content-encoding"), "br");
    let built = include_bytes!(concat!(env!("OUT_DIR"), "/islands/web.js.br"));
    assert_eq!(bundle.header("content-length"), built.len().to_string());
}
