use super::field::write_option;
use super::props::Props;
use crate::escape::push_escaped;
use crate::render::{Renderer, text_of, write_escaped};
use crate::syntax::{Expression, LocatedError, Position};
use crate::translations::Tone;

#[derive(Debug)]
pub(crate) struct Badge {
    pub value: Expression,
    pub enumeration: String,
    #[cfg_attr(
        not(feature = "parse"),
        expect(dead_code, reason = "it locates what the template check reports")
    )]
    pub position: Position,
}

pub(super) fn build_badge(props: &mut Props, position: Position) -> Result<Badge, LocatedError> {
    Ok(Badge {
        value: props.required_path("value")?,
        enumeration: props.required_literal("enum")?.0,
        position,
    })
}

/// A value the enum does not list is shown as it is, in the neutral tone.
pub(super) fn render_badge<'a>(
    badge: &'a Badge,
    id: Option<&'a Expression>,
    renderer: &mut Renderer<'a>,
) {
    let language = renderer.language;
    let value = renderer.resolve(&badge.value);
    let variant = text_of(value).and_then(|name| {
        language
            .enumeration(&badge.enumeration)?
            .iter()
            .find(|variant| variant.name == name)
    });
    renderer.push_markup("<span class=\"");
    renderer.push_markup(
        variant
            .map_or(Tone::Neutral, |variant| variant.tone)
            .class(),
    );
    renderer.push_markup("\"");
    renderer.write_id(id);
    renderer.push_markup(">");
    let out = renderer.output();
    match variant {
        Some(variant) => push_escaped(out, language.text(&variant.key, &variant.humanized)),
        None => write_escaped(out, value),
    }
    out.push_str("</span>");
}

pub(super) fn write_enum_options(
    renderer: &mut Renderer<'_>,
    options: Option<&Expression>,
    selected: Option<&str>,
) {
    let Some(Expression::Authored(enumeration)) = options else {
        return;
    };
    let language = renderer.language;
    for variant in language.enumeration(enumeration).unwrap_or_default() {
        let label = language.text(&variant.key, &variant.humanized);
        write_option(renderer.output(), &variant.name, label, selected);
    }
}
