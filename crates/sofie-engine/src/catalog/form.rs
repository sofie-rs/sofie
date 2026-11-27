use sofie_values::FormField;

use super::field::{Field, is_required, render_field};
use super::props::{Props, is_element_id, is_field_name};
use super::target::Target;
use super::{Component, Content};
use crate::render::Renderer;
use crate::syntax::{Expression, LocatedError, Op, Position};

/// htmx sends its id as `HX-Trigger`, so the reply can place errors under the fields.
#[derive(Debug)]
pub(crate) struct Form {
    pub id: String,
    pub post: Expression,
    pub target: Option<Target>,
    pub parts: Vec<FormPart>,
    skipped: Vec<String>,
    position: Position,
}

#[derive(Debug)]
pub(crate) enum FormPart {
    Field(Box<Field>),
    Submit(Submit),
}

#[derive(Debug)]
pub(crate) struct Submit {
    pub label: Expression,
}

/// Shown again in the form whose `post` is `action`, so nothing typed is lost but a password.
#[derive(Clone, Copy, Debug)]
pub struct Submitted<'a> {
    pub action: &'a str,
    pub input: &'a serde_json::Value,
    pub errors: &'a [(&'a str, String)],
}

impl<'a> Submitted<'a> {
    pub(super) fn error(&self, field: &str) -> Option<&'a str> {
        self.errors
            .iter()
            .find(|(name, _)| *name == field)
            .map(|(_, message)| message.as_str())
    }
}

pub(super) fn build_form(
    props: &mut Props,
    children: Vec<Op>,
    position: Position,
) -> Result<Form, LocatedError> {
    let (id, id_position) = props.required_literal("id")?;
    if !is_element_id(&id) {
        bail!(
            id_position,
            "`Form` id `{id}` is not an element id (a letter, then letters, digits, `-` and `_`)"
        );
    }
    let parts = form_parts(children, position)?;
    check_unique_names(&id, &parts)?;
    let skipped = skipped_names(props)?;
    if let Some(field) = written_fields(&parts).find(|field| skipped.contains(&field.name)) {
        bail!(
            field.position,
            "`Form` `{id}` both skips and shows `{}`",
            field.name
        );
    }
    Ok(Form {
        id,
        post: props.required_post()?,
        target: Target::take(props)?,
        parts,
        skipped,
        position,
    })
}

fn skipped_names(props: &mut Props) -> Result<Vec<String>, LocatedError> {
    let Some(attribute) = props.take("skip") else {
        return Ok(Vec::new());
    };
    let names: Vec<String> = match &attribute.value {
        Expression::Authored(text) => text.split_whitespace().map(str::to_owned).collect(),
        _ => Vec::new(),
    };
    if names.is_empty() || names.iter().any(|name| !is_field_name(name)) {
        bail!(
            attribute.position,
            "`Form` needs `skip` as quoted field names, such as `skip=\"note internal_code\"`"
        );
    }
    Ok(names)
}

fn written_fields(parts: &[FormPart]) -> impl Iterator<Item = &Field> {
    parts.iter().filter_map(|part| match part {
        FormPart::Field(field) => Some(&**field),
        FormPart::Submit(_) => None,
    })
}

pub(super) fn build_submit(props: &mut Props) -> Result<Submit, LocatedError> {
    Ok(Submit {
        label: props.required("label")?,
    })
}

fn form_parts(children: Vec<Op>, position: Position) -> Result<Vec<FormPart>, LocatedError> {
    let mut parts = Vec::new();
    for child in children {
        match child {
            Op::Markup(text) if text.trim().is_empty() => {}
            Op::Component(Component {
                content: Content::FormPart(part),
                ..
            }) => parts.push(part),
            _ => {
                bail!(position, "`Form` takes only `Field` and `Submit` children");
            }
        }
    }
    Ok(parts)
}

fn check_unique_names(form_id: &str, parts: &[FormPart]) -> Result<(), LocatedError> {
    let mut seen: Vec<&str> = Vec::new();
    for field in written_fields(parts) {
        if seen.contains(&field.name.as_str()) {
            bail!(
                field.position,
                "`Form` `{form_id}` has two fields named `{}`",
                field.name
            );
        }
        seen.push(&field.name);
    }
    Ok(())
}

impl Form {
    pub(super) fn field_names(&self) -> impl Iterator<Item = &str> {
        written_fields(&self.parts).map(|field| field.name.as_str())
    }

    pub(super) fn describe(
        &mut self,
        fields: &'static [FormField],
        module: Option<&str>,
    ) -> Result<(), LocatedError> {
        let mut written = Vec::new();
        let mut submits = Vec::new();
        for part in std::mem::take(&mut self.parts) {
            match part {
                FormPart::Field(field) => written.push(field),
                submit @ FormPart::Submit(_) => submits.push(submit),
            }
        }
        self.check_skipped(fields)?;
        for described in fields {
            if self.skipped.iter().any(|name| name == described.name) {
                continue;
            }
            let mut field = match written
                .iter()
                .position(|field| field.name == described.name)
            {
                Some(index) => written.swap_remove(index),
                None => Box::new(Field::of(described, module, self.position)),
            };
            field.describe(described)?;
            self.parts.push(FormPart::Field(field));
        }
        if let Some(stray) = written.first() {
            let names: Vec<&str> = fields.iter().map(|described| described.name).collect();
            bail!(
                stray.position,
                "`Field` `{}` is not a field of the input form `{}` posts; its fields are {}",
                stray.name,
                self.id,
                names.join(", ")
            );
        }
        self.parts.extend(submits);
        Ok(())
    }

    /// A skipped field the input requires would refuse every post, as nothing else can send it.
    fn check_skipped(&self, fields: &[FormField]) -> Result<(), LocatedError> {
        for name in &self.skipped {
            match fields.iter().find(|described| described.name == name) {
                None => {
                    let names: Vec<&str> = fields.iter().map(|described| described.name).collect();
                    bail!(
                        self.position,
                        "`Form` `{}` skips `{name}`, which is not a field of the input it posts; \
                         its fields are {}",
                        self.id,
                        names.join(", ")
                    );
                }
                Some(described) if is_required(described) => bail!(
                    self.position,
                    "`Form` `{}` skips `{name}`, which the input requires; show it, or make it \
                     an `Option` the action fills",
                    self.id
                ),
                Some(_) => {}
            }
        }
        Ok(())
    }
}

/// Posts even without JavaScript, so fields never land in a URL.
pub(super) fn render_form<'a>(
    form: &'a Form,
    renderer: &mut Renderer<'a>,
) -> Result<(), LocatedError> {
    renderer.push_markup("<form id=\"");
    renderer.push_markup(&form.id);
    renderer.push_markup("\" method=\"post\"");
    if form
        .parts
        .iter()
        .any(|part| matches!(part, FormPart::Field(field) if field.is_file()))
    {
        renderer.push_markup(" enctype=\"multipart/form-data\"");
    }
    renderer.write_post_attribute("action", &form.post);
    renderer.write_post_attribute("hx-post", &form.post);
    if let Some(target) = &form.target {
        target.write(renderer);
    }
    renderer.push_markup(" hx-swap=\"outerHTML\">");
    renderer.write_idempotency_key(form.position)?;
    let submitted = renderer.submitted_to(&form.post);
    for part in &form.parts {
        match part {
            FormPart::Field(field) => render_field(field, &form.id, submitted, renderer)?,
            FormPart::Submit(submit) => render_submit(submit, renderer),
        }
    }
    renderer.push_markup("</form>");
    Ok(())
}

fn render_submit<'a>(submit: &'a Submit, renderer: &mut Renderer<'a>) {
    renderer.push_markup("<button type=\"submit\">");
    renderer.write_text(&submit.label);
    renderer.push_markup("</button>");
}
