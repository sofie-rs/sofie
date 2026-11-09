use super::props::Props;
use super::target::Target;
use crate::render::{Renderer, is_truthy};
use crate::syntax::{Expression, LocatedError, Position};

#[derive(Debug)]
pub(crate) struct Action {
    pub post: Expression,
    pub label: Expression,
    pub target: Option<Target>,
    pub when: Option<Expression>,
    /// `with={{slot: slot.id}}`: posted as the route's input fields, in the order written.
    pub fields: Vec<PostedField>,
    position: Position,
}

#[derive(Debug)]
pub(crate) struct PostedField {
    pub name: String,
    pub value: Expression,
}

pub(super) fn build_action(props: &mut Props) -> Result<Action, LocatedError> {
    Ok(Action {
        post: props.required_post()?,
        label: props.required("label")?,
        target: Target::take(props)?,
        when: props.optional_value("when")?,
        fields: posted_fields(props)?,
        position: props.position(),
    })
}

/// Data or a literal only: a translation or a gate would post text the route cannot read back.
fn posted_fields(props: &mut Props) -> Result<Vec<PostedField>, LocatedError> {
    props
        .fields("with")
        .into_iter()
        .map(|attribute| match attribute.value {
            Expression::Path(_) | Expression::Text(_) | Expression::Integer(_) => Ok(PostedField {
                name: attribute.name,
                value: attribute.value,
            }),
            _ => bail!(
                attribute.position,
                "`with` on `Action` posts data: give `{}` a path such as `slot.id`, a string or \
                 an integer",
                attribute.name
            ),
        })
        .collect()
}

pub(super) fn render_action<'a>(
    action: &'a Action,
    id: Option<&'a Expression>,
    renderer: &mut Renderer<'a>,
) -> Result<(), LocatedError> {
    if let Some(when) = &action.when
        && !is_truthy(renderer.resolve(when))
    {
        return Ok(());
    }
    if action.fields.is_empty() {
        render_button(action, id, renderer);
        return Ok(());
    }
    render_form(action, id, renderer)
}

fn render_button<'a>(action: &'a Action, id: Option<&'a Expression>, renderer: &mut Renderer<'a>) {
    renderer.push_markup("<button type=\"button\"");
    renderer.write_id(id);
    write_htmx_post(action, renderer);
    renderer.write_text(&action.label);
    renderer.push_markup("</button>");
}

/// A form, so its fields post as hidden inputs, with JavaScript or without.
fn render_form<'a>(
    action: &'a Action,
    id: Option<&'a Expression>,
    renderer: &mut Renderer<'a>,
) -> Result<(), LocatedError> {
    renderer.push_markup("<form");
    renderer.write_id(id);
    renderer.push_markup(" method=\"post\"");
    renderer.write_url_attribute("action", &action.post);
    write_htmx_post(action, renderer);
    renderer.write_idempotency_key(action.position)?;
    for field in &action.fields {
        renderer.push_markup("<input type=\"hidden\" name=\"");
        renderer.push_markup(&field.name);
        renderer.push_markup("\" value=\"");
        renderer.write_text(&field.value);
        renderer.push_markup("\">");
    }
    renderer.push_markup("<button type=\"submit\">");
    renderer.write_text(&action.label);
    renderer.push_markup("</button></form>");
    Ok(())
}

fn write_htmx_post<'a>(action: &'a Action, renderer: &mut Renderer<'a>) {
    renderer.write_url_attribute("hx-post", &action.post);
    if let Some(target) = &action.target {
        target.write(renderer);
    }
    renderer.push_markup(" hx-swap=\"outerHTML\">");
}
