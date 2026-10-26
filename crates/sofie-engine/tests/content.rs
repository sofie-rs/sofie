#![cfg(feature = "content")]

mod common;

use serde_json::json;
use sofie_engine::Page;

const SCREEN: &str = r#"<Head title={data.title} description={data.summary} image={data.image} url={data.url} type="article" /><article><h1>{data.title}</h1><Markdown text={data.body} /></article>"#;

fn page(body: &str) -> String {
    Page {
        title: "Content page",
        lang: "en",
        body,
        notice: &[],
        nonce: "n0nce",
        islands: None,
        script_notices: sofie_engine::ScriptNotices::plain(),
    }
    .render()
}

#[test]
fn a_page_moves_its_head_into_the_document_head() {
    let template = common::loaded("content/web/page", SCREEN);
    let data = json!({
        "title": "About \"us\"",
        "summary": "Who we are",
        "image": "https://cdn.example/about.png",
        "url": "https://example.com/pages/about",
        "body": "# Story\n\nWe **ship**.",
    });

    let html = page(&template.render(&data).unwrap());

    let (head, body) = html.split_once("</head>").unwrap();
    assert!(
        head.contains("<title>About &quot;us&quot;</title>"),
        "{head}"
    );
    assert!(!head.contains("<title>Content page</title>"), "{head}");
    assert!(
        head.contains("<meta name=\"description\" content=\"Who we are\">"),
        "{head}"
    );
    assert!(
        head.contains("<link rel=\"canonical\" href=\"https://example.com/pages/about\">"),
        "{head}"
    );
    assert!(
        head.contains("<meta property=\"og:image\" content=\"https://cdn.example/about.png\">"),
        "{head}"
    );
    assert!(head.contains("<meta property=\"og:type\" content=\"article\">"));
    assert!(head.contains("content=\"summary_large_image\""));
    assert!(
        body.contains("<article><h1>About &quot;us&quot;</h1><h1 id=\"h-story\">Story</h1>\n<p>We <strong>ship</strong>.</p>\n</article>"),
        "{body}"
    );
    assert!(!body.contains("sofie-head"), "{body}");
}

#[test]
fn data_cannot_write_the_head_marks_or_markup() {
    let template = common::loaded("content/web/page", SCREEN);
    let data = json!({
        "title": "<!--sofie-head--><script>x()</script><!--/sofie-head-->",
        "body": "<!--sofie-head--><script>x()</script>",
        "url": "javascript:x()",
    });

    let html = page(&template.render(&data).unwrap());

    assert!(!html.contains("<script>x()"), "{html}");
    assert!(
        html.contains("<link rel=\"canonical\" href=\"#\">"),
        "{html}"
    );
    assert_eq!(html.matches("<title>").count(), 1, "{html}");
}

#[test]
fn a_page_without_a_head_keeps_its_route_title() {
    let html = page("<p>plain</p>");

    assert!(html.contains("<title>Content page</title>"), "{html}");
    assert!(html.contains("<p>plain</p></body>"), "{html}");
}

#[test]
fn a_head_value_without_data_writes_no_tag() {
    let template = common::loaded("content/web/page", SCREEN);
    let data = json!({"title": "", "summary": "Who we are", "image": null, "body": ""});

    let html = page(&template.render(&data).unwrap());

    let (head, _) = html.split_once("</head>").unwrap();
    assert!(!head.contains("og:image"), "{head}");
    assert!(
        !head.contains("og:title") && !head.contains("og:url"),
        "{head}"
    );
    assert!(!head.contains("content=\"\""), "{head}");
    assert!(head.contains("<title>Content page</title>"), "{head}");
    assert!(head.contains("og:description"), "{head}");
    assert!(
        head.contains("<meta name=\"twitter:card\" content=\"summary\">"),
        "{head}"
    );
}

#[test]
fn a_markdown_with_an_id_is_a_fragment_htmx_can_target() {
    let template = common::loaded(
        "content/web/intro",
        r#"<main><Markdown id="intro" text={data.body} /></main>"#,
    );
    let data = json!({ "body": "hi" });

    let html = template.render(&data).unwrap();
    let fragment = template.render_fragment(&data, "intro").unwrap();

    assert_eq!(html, "<main><div id=\"intro\"><p>hi</p>\n</div></main>");
    assert_eq!(fragment, "<div id=\"intro\"><p>hi</p>\n</div>");
}

#[test]
fn a_page_is_named_by_a_title_or_a_head_but_not_both() {
    for source in [
        r#"<title>T</title><Head title="U" />"#,
        r#"<Head title="U" /><title>T</title>"#,
    ] {
        let error = sofie_engine::Template::parse("content/web/head", source)
            .expect_err("the page is named twice")
            .to_string();

        assert!(
            error.contains("the page is named twice"),
            "{source}: {error}"
        );
    }
}

/// Only the first `<Head>` moved into `<head>`; the rest left a second title and tags in the body.
#[test]
fn a_page_has_one_head() {
    for (source, refusal) in [
        (
            r#"<Head title="T" /><Head description="D" />"#,
            "`<Head>` is written twice",
        ),
        (
            r"{#each data.posts as post}<Head title={post.title} />{/each}",
            "a `Head` cannot be inside `{#each}`",
        ),
        (
            r#"<Table id="t" of={data.posts} columns="title" as="post"><Head title={post.title} /></Table>"#,
            "a `Head` cannot be inside `{#each}` or a `Table` row",
        ),
    ] {
        let error = sofie_engine::Template::parse("content/web/head", source)
            .expect_err("the page gets two heads")
            .to_string();

        assert!(error.contains(refusal), "{source}: {error}");
    }
}

#[test]
fn each_branch_of_an_if_may_write_the_head() {
    let parsed = sofie_engine::Template::parse(
        "content/web/head",
        r#"{#if data.draft}<Head title="Draft" />{:else}<Head title={data.title} />{/if}"#,
    );

    assert!(parsed.is_ok(), "{:?}", parsed.err());
}

#[test]
fn a_head_takes_no_id() {
    let parsed = sofie_engine::Template::parse("content/web/head", r#"<Head id="h" title="T" />"#);

    assert!(parsed.is_err());
}

/// Text with `{…}` parts rendered nothing, and quoted text came out escaped twice.
#[test]
fn a_markdown_reads_its_text_from_data_only() {
    for source in [
        r##"<Markdown text="# Hi {data.name}" />"##,
        r#"<Markdown text='Say "hi" & bye' />"#,
    ] {
        let parsed = sofie_engine::Template::parse("content/web/text", source);
        let error = parsed
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        assert!(error.contains("`text` on `Markdown`"), "{source}: {error}");
    }
}
