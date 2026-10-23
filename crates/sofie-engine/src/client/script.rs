//! Expressions as JavaScript over the client runtime `S`, which keeps the engine's meaning:
//! a path that leads nowhere is empty, and text is escaped by the framework, not here.

use crate::syntax::{Expression, Path, Root};

pub(super) fn value(expression: &Expression) -> String {
    match expression {
        Expression::Path(path) => path_value(path),
        Expression::Text(text) => literal(text),
        Expression::Integer(number) => number.to_string(),
        Expression::Authored(markup) => authored(markup),
        Expression::Interpolated(parts) => joined(parts.iter().map(value)),
        Expression::Call(call) => call_value(call),
        #[cfg(feature = "gates")]
        Expression::Gate(gate) => format!(
            "S.gate(view, {}, {})",
            literal(gate.asks.name()),
            literal(&gate.name)
        ),
    }
}

pub(super) fn text(expression: &Expression) -> String {
    format!("S.text({})", value(expression))
}

/// A URL as the engine builds one: written parts as they are, each `{…}` part percent-encoded.
pub(super) fn url_text(expression: &Expression) -> String {
    let Expression::Interpolated(parts) = expression else {
        return text(expression);
    };
    joined(parts.iter().map(|part| match part {
        Expression::Authored(markup) => authored(markup),
        _ => format!("S.urlPart({})", value(part)),
    }))
}

fn joined(parts: impl Iterator<Item = String>) -> String {
    format!("S.join({})", array(parts))
}

pub(super) fn array(items: impl Iterator<Item = String>) -> String {
    format!("[{}]", items.collect::<Vec<_>>().join(", "))
}

pub(super) fn slot(index: usize) -> String {
    format!("s{index}")
}

fn path_value(path: &Path) -> String {
    let mut code = match path.root {
        Root::Data => "data".to_owned(),
        Root::Slot(index) => slot(index),
    };
    for field in &path.fields {
        if field.starts_with(|first: char| first.is_ascii_alphabetic() || first == '_') {
            code.push_str("?.");
            code.push_str(field);
        } else {
            code.push_str("?.[");
            code.push_str(&literal(field));
            code.push(']');
        }
    }
    code
}

#[cfg(feature = "i18n")]
fn call_value(call: &crate::translations::Call) -> String {
    use crate::translations::{Call, Format};
    match call {
        Call::Translate { key, arguments, .. } => translation_value(key, arguments),
        Call::Format {
            format,
            value: formatted,
            ..
        } => {
            let function = match format {
                Format::Number => "number",
                Format::Money => "money",
                Format::Date => "date",
            };
            format!("S.{function}(view, {})", value(formatted))
        }
        Call::Label { key, humanized, .. } => {
            format!("S.label(view, {}, {})", literal(key), literal(humanized))
        }
    }
}

#[cfg(not(feature = "i18n"))]
fn call_value(call: &crate::language::Call) -> String {
    use crate::language::Call;
    match call {
        Call::Translate { key, arguments, .. } => translation_value(key, arguments),
        Call::Date {
            value: formatted, ..
        } => format!("S.date(view, {})", value(formatted)),
    }
}

fn translation_value(key: &str, arguments: &[crate::language::Argument]) -> String {
    let arguments: Vec<String> = arguments
        .iter()
        .map(|argument| {
            let written = value(argument.plain_value());
            format!("{}: {written}", literal(&argument.name))
        })
        .collect();
    format!("S.t(view, {}, {{{}}})", literal(key), arguments.join(", "))
}

pub(super) fn literal(text: &str) -> String {
    serde_json::to_string(text).expect("a string always serializes")
}

pub(super) fn authored(markup: &str) -> String {
    literal(&decode_html(markup))
}

/// Authored text is HTML: its entities become the characters they stand for, so the client
/// escapes them once. An entity this does not know is kept as written.
fn decode_html(markup: &str) -> String {
    let mut text = String::with_capacity(markup.len());
    let mut rest = markup;
    while let Some(start) = rest.find('&') {
        text.push_str(&rest[..start]);
        rest = &rest[start..];
        let decoded = rest
            .find(';')
            .filter(|end| *end <= 10)
            .and_then(|end| Some((entity(&rest[1..end])?, end)));
        if let Some((character, end)) = decoded {
            text.push(character);
            rest = &rest[end + 1..];
        } else {
            text.push('&');
            rest = &rest[1..];
        }
    }
    text.push_str(rest);
    text
}

fn entity(name: &str) -> Option<char> {
    if let Some(number) = name.strip_prefix('#') {
        let code = match number.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => number.parse().ok()?,
        };
        return char::from_u32(code);
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "hellip" => '…',
        "mdash" => '—',
        "ndash" => '–',
        "laquo" => '«',
        "raquo" => '»',
        "middot" => '·',
        "times" => '×',
        "euro" => '€',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::{decode_html, value};
    use crate::syntax::{Expression, Path, Root};

    #[test]
    fn paths_never_throw_on_a_missing_field() {
        let path = Expression::Path(Path {
            root: Root::Slot(1),
            fields: vec!["status_to".to_owned(), "0".to_owned()],
        });
        assert_eq!(value(&path), "s1?.status_to?.[\"0\"]");
    }

    #[test]
    fn authored_text_is_decoded_once_and_quoted() {
        assert_eq!(
            decode_html("Tom &amp; Jerry &#39;x&#x27; &bogus; & y"),
            "Tom & Jerry 'x' &bogus; & y"
        );
        assert_eq!(
            value(&Expression::Authored("a &quot;b&quot;".to_owned())),
            "\"a \\\"b\\\"\""
        );
    }
}
