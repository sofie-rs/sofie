use super::props::Props;
use crate::islands::{is_island_name, props_json, push_props};
use crate::render::Renderer;
use crate::syntax::{Expression, LocatedError};

#[derive(Debug)]
pub(crate) struct Island {
    pub tag: String,
    pub props: Expression,
}

pub(super) fn build_island(props: &mut Props) -> Result<Island, LocatedError> {
    let (tag, position) = props.required_literal("name")?;
    if !is_island_name(&tag) {
        bail!(
            position,
            "`{tag}` is no island name: name `islands/<name>.svelte` in lowercase with a hyphen, \
             such as `order-chart`"
        );
    }
    Ok(Island {
        tag,
        props: props.required_value("props")?,
    })
}

pub(super) fn render_island<'a>(
    island: &'a Island,
    id: Option<&'a Expression>,
    renderer: &mut Renderer<'a>,
) {
    let props = props_json(renderer.resolve(&island.props));
    renderer.push_markup("<");
    renderer.push_markup(&island.tag);
    renderer.write_id(id);
    renderer.push_markup(">");
    push_props(renderer.output(), &props);
    renderer.push_markup("</");
    renderer.push_markup(&island.tag);
    renderer.push_markup(">");
}
