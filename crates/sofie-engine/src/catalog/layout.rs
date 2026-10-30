use super::props::Props;
use super::{Component, Content};
use crate::render::Renderer;
use crate::syntax::{Conditional, Expression, LocatedError, Loop, Op, Position};

/// HTML's interactive content, which a link may not hold.
const INTERACTIVE_TAGS: &[&str] = &[
    "a", "audio", "button", "details", "embed", "iframe", "input", "label", "select", "textarea",
    "video",
];

#[derive(Debug)]
pub(crate) enum Layout {
    Grid(Vec<Op>),
    Card {
        href: Option<Expression>,
        children: Vec<Op>,
    },
    Image {
        src: Expression,
        alt: Expression,
    },
}

impl Layout {
    pub(super) fn children(&self) -> &[Op] {
        match self {
            Self::Grid(children) | Self::Card { children, .. } => children,
            Self::Image { .. } => &[],
        }
    }

    pub(super) fn children_mut(&mut self) -> &mut [Op] {
        match self {
            Self::Grid(children) | Self::Card { children, .. } => children,
            Self::Image { .. } => &mut [],
        }
    }
}

pub(super) fn build_card(
    props: &mut Props,
    children: Vec<Op>,
    position: Position,
) -> Result<Layout, LocatedError> {
    let href = props.optional_url("href")?;
    if href.is_some()
        && let Some(found) = interactive_in(&children)
    {
        bail!(
            position,
            "{found} cannot sit inside a `Card` with `href`: the card is a link, and a link \
             cannot hold another control; drop `href` or move {found} out of the card"
        );
    }
    Ok(Layout::Card { href, children })
}

fn interactive_in(ops: &[Op]) -> Option<String> {
    ops.iter().find_map(|op| match op {
        Op::Markup(markup) => interactive_tag(markup).map(|tag| format!("`<{tag}>`")),
        Op::Component(component) => interactive_component(component),
        Op::Identified { body, .. } | Op::Each(Loop { body, .. }) => interactive_in(body),
        Op::If(Conditional {
            then_branch,
            else_branch,
            ..
        }) => interactive_in(then_branch).or_else(|| interactive_in(else_branch)),
        Op::Text(_) | Op::Url(_) => None,
    })
}

fn interactive_tag(markup: &str) -> Option<&'static str> {
    markup.split('<').skip(1).find_map(|rest| {
        let name: String = rest
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .map(|character| character.to_ascii_lowercase())
            .collect();
        INTERACTIVE_TAGS.iter().copied().find(|tag| *tag == name)
    })
}

fn interactive_component(component: &Component) -> Option<String> {
    let name = match &component.content {
        Content::Layout(Layout::Card { href: Some(_), .. }) => "Card href",
        Content::Action(_) => "Action",
        Content::Form(_) | Content::FormPart(_) => "Form",
        Content::Table(_) => "Table",
        #[cfg(feature = "islands")]
        Content::Island(_) => "Island",
        #[cfg(feature = "content")]
        Content::Published(super::content::Published::Markdown(_)) => "Markdown",
        _ => return interactive_in(component.children()),
    };
    Some(format!("`{name}`"))
}

pub(super) fn build_image(props: &mut Props) -> Result<Layout, LocatedError> {
    Ok(Layout::Image {
        src: props.required_url("src")?,
        alt: props.required("alt")?,
    })
}

pub(super) fn render<'a>(
    layout: &'a Layout,
    id: Option<&'a Expression>,
    renderer: &mut Renderer<'a>,
) -> Result<(), LocatedError> {
    match layout {
        Layout::Grid(children) => render_box("sofie-grid", None, children, id, renderer),
        Layout::Card { href, children } => {
            render_box("sofie-card", href.as_ref(), children, id, renderer)
        }
        Layout::Image { src, alt } => {
            renderer.push_markup("<img class=\"sofie-image\"");
            renderer.write_id(id);
            renderer.write_url_attribute("src", src);
            renderer.push_markup(" alt=\"");
            renderer.write_text(alt);
            renderer.push_markup("\" loading=\"lazy\">");
            Ok(())
        }
    }
}

fn render_box<'a>(
    class: &str,
    href: Option<&'a Expression>,
    children: &'a [Op],
    id: Option<&'a Expression>,
    renderer: &mut Renderer<'a>,
) -> Result<(), LocatedError> {
    let tag = if href.is_some() { "a" } else { "div" };
    renderer.push_markup("<");
    renderer.push_markup(tag);
    renderer.push_markup(" class=\"");
    renderer.push_markup(class);
    renderer.push_markup("\"");
    renderer.write_id(id);
    if let Some(href) = href {
        renderer.write_url_attribute("href", href);
    }
    renderer.push_markup(">");
    renderer.render_ops(children)?;
    renderer.push_markup("</");
    renderer.push_markup(tag);
    renderer.push_markup(">");
    Ok(())
}
