use serde_json::json;
use sofie::db::Db;
use sofie::prelude::*;
use sofie_content::{Settings, admin_routes};
use sofie_testing::TestApp;

/// Open to anyone, so the tests need no sign-in; an app writes pages in a `Permitted` area.
struct Desk;

impl Area for Desk {
    const NAME: &'static str = "desk";
    const PREFIX: &'static str = "/desk";
    const SIGN_IN: &'static str = "/";
    type Principal = sofie::http::Visitor;
}

async fn started() -> TestApp {
    sofie_testing::start(
        |db: Db| async move {
            db.schema().migrate(&[sofie_content::migrations()]).await?;
            sofie_content::install(&db, Settings::new("https://example.com/")?);
            let mut screens = sofie::http::Screens::default();
            screens.add_builtin(sofie_content::MODULE.screens)?;
            Ok(App::new(db)
                .screens(screens)
                .area::<Web>("sofie_content", sofie_content::routes)
                .area::<Desk>("sofie_content", admin_routes::<Desk>))
        },
        sofie::worker::Worker::new,
    )
    .await
}

fn page(title: &str) -> serde_json::Value {
    json!({
        "title": title,
        "summary": "Who we are",
        "body": "# Story\n\nWe **ship**. <script>x()</script>",
        "image": "https://cdn.example/about.png",
    })
}

async fn written(app: &TestApp, title: &str) -> String {
    let created = app.post("/desk/pages/new").json(&page(title)).await;
    created.record_id().to_string()
}

async fn retitled(app: &TestApp, id: &str, title: &str) {
    app.post(&format!("/desk/pages/{id}/edit"))
        .json(&page(title))
        .await
        .assert_ok();
}

async fn acted(app: &TestApp, id: &str, action: &str) {
    app.post(&format!("/desk/pages/{id}/{action}"))
        .json(&json!({}))
        .await
        .assert_ok();
}

#[tokio::test]
async fn a_page_is_served_only_while_published() {
    let app = started().await;
    let id = written(&app, "About <us>").await;

    let draft = app.get("/pages/about-us").await;
    acted(&app, &id, "publish").await;
    let published = app.get("/pages/about-us").await;
    acted(&app, &id, "withdraw").await;
    let withdrawn = app.get("/pages/about-us").await;

    draft.assert_status(404);
    published.assert_status(200);
    let (head, body) = published.body.split_once("</head>").unwrap();
    assert!(head.contains("<title>About &lt;us&gt;</title>"), "{head}");
    assert!(
        head.contains("<link rel=\"canonical\" href=\"https://example.com/pages/about-us\">"),
        "{head}"
    );
    assert!(
        head.contains("<meta property=\"og:image\" content=\"https://cdn.example/about.png\">"),
        "{head}"
    );
    assert!(
        body.contains("<p>We <strong>ship</strong>. &lt;script&gt;"),
        "{body}"
    );
    withdrawn.assert_status(404);
}

#[tokio::test]
async fn the_sitemap_lists_published_pages_with_their_last_change() {
    let app = started().await;
    let about = written(&app, "About").await;
    written(&app, "Draft").await;
    acted(&app, &about, "publish").await;

    let sitemap = app.get("/sitemap.xml").await;

    sitemap.assert_status(200);
    assert_eq!(
        sitemap.header("content-type"),
        "application/xml; charset=utf-8"
    );
    assert!(
        sitemap
            .body
            .contains("<url><loc>https://example.com/pages/about</loc><lastmod>"),
        "{}",
        sitemap.body
    );
    assert!(!sitemap.body.contains("draft"), "{}", sitemap.body);
}

#[tokio::test]
async fn a_client_reading_the_sitemap_over_and_over_is_throttled() {
    let app = started().await;

    for _ in 0..6 {
        app.get("/sitemap.xml").await.assert_status(200);
    }
    let again = app.get("/sitemap.xml").await;

    again.assert_status(429);
}

#[tokio::test]
async fn a_title_names_one_page_and_a_path_that_is_no_slug_names_none() {
    let app = started().await;
    written(&app, "About").await;

    let taken = app.post("/desk/pages/new").json(&page("ABOUT!")).await;
    let not_a_slug = app.get("/pages/Not%20A%20Slug").await;

    taken.assert_status(422);
    assert!(taken.body.contains("title"), "{}", taken.body);
    not_a_slug.assert_status(404);
}

#[tokio::test]
async fn a_page_s_earlier_addresses_move_permanently_to_its_current_one() {
    let app = started().await;
    let id = written(&app, "About").await;
    acted(&app, &id, "publish").await;
    retitled(&app, &id, "Our Story").await;
    retitled(&app, &id, "Our Team").await;

    let first = app.get("/pages/about?from=mail&x=1").await;
    let second = app.get("/pages/our-story").await;
    let current = app.get("/pages/our-team").await;
    let other = written(&app, "About").await;
    acted(&app, &other, "publish").await;
    let reused = app.get("/pages/about").await;

    first.assert_status(301);
    assert_eq!(first.header("location"), "/pages/our-team?from=mail&x=1");
    second.assert_status(301);
    assert_eq!(second.header("location"), "/pages/our-team");
    current.assert_status(200);
    reused.assert_status(200);
}

#[tokio::test]
async fn a_title_in_any_script_gives_its_page_an_address() {
    let app = started().await;
    let lunch = written(&app, "Öğle Yemeği").await;
    let about = written(&app, "关于我们").await;
    acted(&app, &lunch, "publish").await;
    acted(&app, &about, "publish").await;

    let turkish = app.get("/pages/ogle-yemegi").await;
    let chinese = app.get("/pages/%E5%85%B3%E4%BA%8E%E6%88%91%E4%BB%AC").await;

    turkish.assert_status(200);
    chinese.assert_status(200);
    assert!(
        chinese
            .body
            .contains("https://example.com/pages/%E5%85%B3%E4%BA%8E%E6%88%91%E4%BB%AC"),
        "{}",
        chinese.body
    );
}

#[tokio::test]
async fn a_page_s_text_is_bounded() {
    let app = started().await;
    let page = json!({
        "title": "t".repeat(201),
        "summary": "s".repeat(501),
        "body": "b".repeat(50_001),
    });

    let refused = app.post("/desk/pages/new").json(&page).await;

    refused.assert_status(422);
    for field in ["title", "summary", "body"] {
        assert!(refused.body.contains(field), "{field}: {}", refused.body);
    }
    assert!(refused.body.contains("at most"), "{}", refused.body);
}

#[tokio::test]
async fn a_page_titled_in_a_script_with_marks_is_served_at_its_slug() {
    let app = started().await;
    let id = written(&app, "नमस्ते").await;
    acted(&app, &id, "publish").await;

    // नमस्ते, its virama kept in the word.
    let greeting = app
        .get("/pages/%E0%A4%A8%E0%A4%AE%E0%A4%B8%E0%A5%8D%E0%A4%A4%E0%A5%87")
        .await;

    greeting.assert_status(200);
}
