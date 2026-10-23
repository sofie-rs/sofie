use sofie_core::Result;
use sofie_db::Db;
use sofie_http::{App, Media, Routes, Text, Web};
use sofie_testing::{FakeDriver, TestServer};

async fn sitemap() -> Result<Text> {
    Ok(Text(
        "<urlset><url><loc>https://example.com/</loc></url></urlset>".to_owned(),
    ))
}

async fn not_text() -> Result<()> {
    Ok(())
}

fn routes(routes: &mut Routes<Web>) {
    routes.get("/sitemap.xml", sitemap).document(Media::Xml);
    routes.get("/robots.txt", not_text).document(Media::Text);
}

#[tokio::test]
async fn a_document_route_answers_its_loader_s_text_as_its_media() {
    let server =
        TestServer::start(App::new(Db::new(FakeDriver::default())).area("site", routes)).await;

    let answered = server.get("/sitemap.xml").await;
    let defect = server.get("/robots.txt").await;

    answered.assert_status(200);
    assert_eq!(
        answered.header("content-type"),
        "application/xml; charset=utf-8"
    );
    assert_eq!(
        answered.body,
        "<urlset><url><loc>https://example.com/</loc></url></urlset>"
    );
    defect.assert_status(500);
}
