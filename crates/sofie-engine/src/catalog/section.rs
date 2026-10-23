use super::props::Props;
use crate::render::Renderer;
use crate::syntax::{Expression, LocatedError, Op};

#[derive(Debug)]
pub(crate) struct Section {
    pub title: Expression,
    pub children: Vec<Op>,
}

pub(super) fn build_section(props: &mut Props, children: Vec<Op>) -> Result<Section, LocatedError> {
    Ok(Section {
        title: props.required("title")?,
        children,
    })
}

pub(super) fn render_section<'a>(
    section: &'a Section,
    id: Option<&'a Expression>,
    renderer: &mut Renderer<'a>,
) -> Result<(), LocatedError> {
    renderer.push_markup("<section");
    renderer.write_id(id);
    renderer.push_markup("><h2>");
    renderer.write_text(&section.title);
    renderer.push_markup("</h2>");
    renderer.render_ops(&section.children)?;
    renderer.push_markup("</section>");
    Ok(())
}
