//! `<Markdown text={…} />` and `<Head title={…} description={…} image={…} url={…} />`.

use super::markdown::push_html;
use super::props::Props;
use crate::page::{HEAD_END, HEAD_START};
use crate::render::{Renderer, text_of};
use crate::syntax::{Expression, LocatedError};

#[derive(Debug)]
pub(crate) enum Published {
    Markdown(Expression),
    Head(Head),
}

/// What a page tells search engines and link previews about itself.
#[derive(Debug)]
pub(crate) struct Head {
    pub(crate) title: Option<Expression>,
    pub(crate) description: Option<Expression>,
    pub(crate) image: Option<Expression>,
    pub(crate) url: Option<Expression>,
    pub(crate) kind: Option<Expression>,
}

pub(super) fn build_markdown(props: &mut Props) -> Result<Published, LocatedError> {
    Ok(Published::Markdown(props.required_path("text")?))
}

pub(super) fn build_head(props: &mut Props) -> Published {
    Published::Head(Head {
        title: props.optional("title"),
        description: props.optional("description"),
        image: props.optional("image"),
        url: props.optional("url"),
        kind: props.optional("type"),
    })
}

/// An id wraps the markdown, which has no element of its own to carry it.
pub(super) fn render<'a>(
    published: &'a Published,
    id: Option<&'a Expression>,
    renderer: &mut Renderer<'a>,
) {
    match published {
        Published::Markdown(text) => {
            if id.is_some() {
                renderer.push_markup("<div");
                renderer.write_id(id);
                renderer.push_markup(">");
            }
            let text = text_of(renderer.resolve(text)).unwrap_or_default();
            push_html(renderer.output(), &text);
            if id.is_some() {
                renderer.push_markup("</div>");
            }
        }
        Published::Head(head) => render_head(head, renderer),
    }
}

/// Marked so the page moves it into `<head>`; data is escaped, so no value can write the marks.
fn render_head<'a>(head: &'a Head, renderer: &mut Renderer<'a>) {
    renderer.push_markup(HEAD_START);
    let text: fn(&mut Renderer<'a>, &'a Expression) = Renderer::write_text;
    let url: fn(&mut Renderer<'a>, &'a Expression) = Renderer::write_url;
    let tags = [
        ("<title>", &head.title, text, "</title>"),
        (
            "<meta name=\"description\" content=\"",
            &head.description,
            text,
            "\">",
        ),
        (
            "<meta property=\"og:title\" content=\"",
            &head.title,
            text,
            "\">",
        ),
        (
            "<meta property=\"og:description\" content=\"",
            &head.description,
            text,
            "\">",
        ),
        (
            "<meta property=\"og:type\" content=\"",
            &head.kind,
            text,
            "\">",
        ),
        ("<link rel=\"canonical\" href=\"", &head.url, url, "\">"),
        (
            "<meta property=\"og:url\" content=\"",
            &head.url,
            url,
            "\">",
        ),
    ];
    for (open, value, write, close) in tags {
        if let Some(value) = value {
            write_tag(renderer, open, value, write, close);
        }
    }
    let has_image = head.image.as_ref().is_some_and(|image| {
        let open = "<meta property=\"og:image\" content=\"";
        write_tag(renderer, open, image, url, "\">")
    });
    let card = if has_image {
        "summary_large_image"
    } else {
        "summary"
    };
    renderer.push_markup("<meta name=\"twitter:card\" content=\"");
    renderer.push_markup(card);
    renderer.push_markup("\">");
    renderer.push_markup(HEAD_END);
}

/// A tag whose value comes out empty is taken back: previews read an empty `og:image` as a
/// broken image rather than none.
fn write_tag<'a>(
    renderer: &mut Renderer<'a>,
    open: &str,
    value: &'a Expression,
    write: fn(&mut Renderer<'a>, &'a Expression),
    close: &str,
) -> bool {
    let start = renderer.output().len();
    renderer.push_markup(open);
    let value_start = renderer.output().len();
    write(renderer, value);
    if renderer.output().len() == value_start {
        renderer.output().truncate(start);
        return false;
    }
    renderer.push_markup(close);
    true
}
