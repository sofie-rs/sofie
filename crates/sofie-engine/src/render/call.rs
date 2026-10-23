use sofie_core::Shown;

use super::value::{local_and_zone, write_plain};
use super::{Renderer, Resolved, text_of, write_escaped};
use crate::catalog::money_of;
use crate::escape::push_escaped;
use crate::message::Recorded;
use crate::syntax::Expression;
use crate::translations::{Argument, Call, CountedKeys, Format, Message, Part};

pub(super) fn write_call<'a>(renderer: &mut Renderer<'a>, call: &'a Call) {
    match call {
        Call::Translate {
            key,
            counted,
            arguments,
            recorded,
            ..
        } => write_translation(
            renderer,
            key,
            counted.as_ref(),
            arguments,
            recorded.as_ref(),
        ),
        Call::Format { format, value, .. } => write_formatted(renderer, *format, value),
        Call::Label { key, humanized, .. } => {
            let message = renderer.language.message(key).and_then(Message::plain);
            push_escaped(&mut renderer.out, message.unwrap_or(humanized));
        }
    }
}

/// A key the translations lack is written in the text compiled into the screen, else as the key,
/// so the gap shows; the template check refuses it before that.
fn write_translation<'a>(
    renderer: &mut Renderer<'a>,
    key: &'a str,
    counted: Option<&CountedKeys>,
    arguments: &'a [Argument],
    recorded: Option<&'a Recorded>,
) {
    let count = arguments
        .iter()
        .find(|argument| argument.name == "count")
        .map(|count| renderer.resolve(count.plain_value()));
    let message = match (counted, count) {
        (Some(keys), Some(count)) => renderer.language.counted_message(key, keys, count),
        _ => renderer.language.message(key),
    }
    .or_else(|| Some(recorded?.counted(count.flatten())));
    let Some(message) = message else {
        push_escaped(&mut renderer.out, key);
        return;
    };
    for part in message.parts() {
        match part {
            Part::Text(text) => push_escaped(&mut renderer.out, text),
            Part::Argument(name) => {
                if let Some(argument) = arguments.iter().find(|argument| argument.name == *name) {
                    write_argument(renderer, argument.plain_value());
                }
            }
        }
    }
}

fn write_argument<'a>(renderer: &mut Renderer<'a>, value: &'a Expression) {
    let resolved = renderer.resolve(value);
    if is_number(resolved) {
        write_number(renderer, resolved);
    } else {
        renderer.write_text(value);
    }
}

fn write_number(renderer: &mut Renderer<'_>, number: Resolved<'_>) {
    let conventions = renderer.language.conventions();
    let Renderer { out, scratch, .. } = renderer;
    scratch.clear();
    write_plain(scratch, number);
    conventions.write_number(out, scratch);
}

fn write_formatted<'a>(renderer: &mut Renderer<'a>, format: Format, value: &'a Expression) {
    let resolved = renderer.resolve(value);
    if matches!(format, Format::Number) && is_number(resolved) {
        write_number(renderer, resolved);
        return;
    }
    let conventions = renderer.language.conventions();
    let Renderer { out, scratch, .. } = renderer;
    match format {
        Format::Number => write_escaped(out, resolved),
        Format::Money => {
            if let Some(amount) = money_of(resolved) {
                conventions.write_money(out, amount, scratch);
            }
        }
        Format::Date => {
            if let Some(Shown::Record(fields)) = resolved
                && let Some((local, zone)) = local_and_zone(fields)
            {
                conventions.write_local_date_time(out, &local, &zone);
            } else if let Some(text) = text_of(resolved) {
                conventions.write_date(out, &text, scratch);
            }
        }
    }
}

fn is_number(resolved: Resolved<'_>) -> bool {
    matches!(
        resolved,
        Some(Shown::Number(_) | Shown::Integer(_) | Shown::Float(_))
    )
}
