use sofie_core::{Instant, Shown};

use super::{Renderer, text_of, write_escaped};
use crate::escape::push_escaped;
use crate::language::{Argument, Call};
use crate::message::{Part, Recorded};
use crate::syntax::Expression;

pub(super) fn write_call<'a>(renderer: &mut Renderer<'a>, call: &'a Call) {
    match call {
        Call::Translate {
            key,
            arguments,
            recorded,
            ..
        } => write_recorded(renderer, key, arguments, recorded.as_ref()),
        Call::Date { value, .. } => write_date(renderer, value),
    }
}

/// Written as the key where the screen was compiled without its text, as a translation gap is.
fn write_recorded<'a>(
    renderer: &mut Renderer<'a>,
    key: &str,
    arguments: &'a [Argument],
    recorded: Option<&Recorded>,
) {
    let Some(recorded) = recorded else {
        push_escaped(&mut renderer.out, key);
        return;
    };
    let count = arguments
        .iter()
        .find(|argument| argument.name == "count")
        .and_then(|count| renderer.resolve(&count.value));
    for part in recorded.counted(count).parts() {
        match part {
            Part::Text(text) => push_escaped(&mut renderer.out, text),
            Part::Argument(name) => {
                let argument = arguments.iter().find(|argument| argument.name == *name);
                if let Some(argument) = argument {
                    renderer.write_text(&argument.value);
                }
            }
        }
    }
}

/// An instant, or text naming one with its offset, as a `Table` writes an instant cell; anything
/// else, such as a day, as it reads.
fn write_date<'a>(renderer: &mut Renderer<'a>, value: &'a Expression) {
    let resolved = renderer.resolve(value);
    let instant = match resolved {
        Some(Shown::Instant(instant)) => Some(instant),
        _ => text_of(resolved)
            .filter(|text| !Instant::is_wall_clock(text))
            .and_then(|text| Instant::parse(&text).ok()),
    };
    match instant {
        Some(instant) => renderer.write_instant(instant),
        None => write_escaped(&mut renderer.out, resolved),
    }
}
