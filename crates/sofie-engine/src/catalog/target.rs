use super::props::{Props, is_element_id};
use crate::render::Renderer;
use crate::syntax::{Expression, LocatedError};

#[derive(Debug)]
pub(crate) struct Target {
    pub id: Expression,
}

impl Target {
    pub(super) fn take(props: &mut Props) -> Result<Option<Self>, LocatedError> {
        let Some(attribute) = props.take("target") else {
            return Ok(None);
        };
        if let Expression::Authored(literal) = &attribute.value
            && !is_element_id(literal)
        {
            bail!(
                attribute.position,
                "target `{literal}` is not an element id (a letter, then letters, digits, `-` and `_`)"
            );
        }
        Ok(Some(Self {
            id: attribute.value,
        }))
    }

    /// Bad data is not the template's defect: an id that is no plain id targets nothing rather
    /// than a wider selector, as a listing's does. A plain id has nothing to escape.
    /// The element stays in the page, so htmx would queue a second click as a second write.
    pub(super) fn write<'a>(&'a self, renderer: &mut Renderer<'a>) {
        renderer.push_markup(" hx-target=\"#");
        if is_element_id(renderer.plain_text(&self.id)) {
            renderer.write_text(&self.id);
        }
        renderer.push_markup("\" hx-sync=\"this:drop\"");
    }
}
