use super::cursor::{Cursor, is_identifier_start};
use crate::syntax::{Expression, LocatedError, Path, Root};

const ROOT_NAME: &str = "data";

/// `scope` holds the loop names in binding-slot order.
pub(super) fn parse_expression(
    cursor: &mut Cursor<'_>,
    scope: &[String],
) -> Result<Expression, LocatedError> {
    cursor.skip_whitespace();
    match cursor.peek() {
        Some(b'"') => parse_string(cursor).map(Expression::Text),
        Some(byte) if byte.is_ascii_digit() || byte == b'-' => {
            parse_integer(cursor).map(Expression::Integer)
        }
        Some(byte) if is_identifier_start(byte) => {
            #[cfg(feature = "gates")]
            if let Some(gate) = crate::rendering::parse_gate(cursor)? {
                return Ok(gate);
            }
            if let Some(call) = super::call::parse_call(cursor, scope)? {
                return Ok(call);
            }
            parse_path(cursor, scope).map(Expression::Path)
        }
        _ => Err(cursor.unexpected(format_args!(
            "expected an expression (a path like `data.field`, a string or an integer)"
        ))),
    }
}

pub(super) fn parse_braced(
    cursor: &mut Cursor<'_>,
    scope: &[String],
) -> Result<Expression, LocatedError> {
    let expression = parse_expression(cursor, scope)?;
    cursor.skip_whitespace();
    cursor.expect("}", "after the expression")?;
    Ok(expression)
}

pub(crate) fn parse_string(cursor: &mut Cursor<'_>) -> Result<String, LocatedError> {
    let start = cursor.position();
    cursor.advance();
    let mut text = String::new();
    let mut chunk_start = cursor.offset();
    loop {
        match cursor.peek() {
            None => return Err(LocatedError::new(start, "unclosed string literal")),
            Some(b'"') => break,
            Some(b'\\') => {
                text.push_str(cursor.slice(chunk_start, cursor.offset()));
                cursor.advance();
                text.push(parse_escape(cursor)?);
                chunk_start = cursor.offset();
            }
            Some(_) => cursor.advance(),
        }
    }
    text.push_str(cursor.slice(chunk_start, cursor.offset()));
    cursor.advance();
    Ok(text)
}

fn parse_escape(cursor: &mut Cursor<'_>) -> Result<char, LocatedError> {
    let escaped = match cursor.peek() {
        Some(b'"') => '"',
        Some(b'\\') => '\\',
        _ => {
            return Err(cursor.error(format!(
                "unsupported escape `\\{}` in a string literal; only `\\\"` and `\\\\` are allowed",
                cursor.describe_next().trim_matches('`')
            )));
        }
    };
    cursor.advance();
    Ok(escaped)
}

fn parse_integer(cursor: &mut Cursor<'_>) -> Result<i64, LocatedError> {
    let start = cursor.position();
    let begin = cursor.offset();
    cursor.eat("-");
    let digits = cursor.take_while(|byte| byte.is_ascii_digit());
    let literal = cursor.slice(begin, cursor.offset());
    if digits.is_empty() {
        return Err(LocatedError::new(start, "expected digits after `-`"));
    }
    literal.parse().map_err(|_| {
        LocatedError::new(
            start,
            format!("integer literal `{literal}` does not fit in 64 bits"),
        )
    })
}

fn parse_path(cursor: &mut Cursor<'_>, scope: &[String]) -> Result<Path, LocatedError> {
    let start = cursor.position();
    let name = cursor.take_identifier();
    let root = resolve_root(name, scope)
        .ok_or_else(|| LocatedError::new(start, unknown_name_message(name, scope)))?;
    let mut fields = Vec::new();
    while cursor.eat(".") {
        let field = cursor.take_identifier();
        if field.is_empty() {
            return Err(cursor.unexpected(format_args!("expected a field name after `.`")));
        }
        fields.push(field.to_owned());
    }
    Ok(Path { root, fields })
}

fn resolve_root(name: &str, scope: &[String]) -> Option<Root> {
    if let Some(slot) = scope.iter().rposition(|bound| bound == name) {
        return Some(Root::Slot(slot));
    }
    (name == ROOT_NAME).then_some(Root::Data)
}

fn unknown_name_message(name: &str, scope: &[String]) -> String {
    let in_scope: Vec<&str> = std::iter::once(ROOT_NAME)
        .chain(scope.iter().map(String::as_str))
        .collect();
    format!(
        "unknown name `{name}`; names in scope: {}",
        in_scope.join(", ")
    )
}

pub(super) fn is_root_name(name: &str) -> bool {
    name == ROOT_NAME
}

#[cfg(test)]
mod tests {
    use super::{Cursor, parse_braced};
    use crate::syntax::{Expression, Root};

    fn parse(source: &str, scope: &[&str]) -> Result<Expression, String> {
        let scope: Vec<String> = scope.iter().map(|name| (*name).to_owned()).collect();
        parse_braced(&mut Cursor::new(source), &scope).map_err(|error| error.message)
    }

    #[test]
    fn resolves_loop_names_to_their_innermost_slot() {
        let Ok(Expression::Path(path)) = parse("row.total}", &["row", "index", "row"]) else {
            panic!("expected a path");
        };
        assert_eq!(path.root, Root::Slot(2));
        assert_eq!(path.fields, ["total"]);
    }

    #[test]
    fn parses_literals() {
        assert!(matches!(parse(r#""a\"b"}"#, &[]), Ok(Expression::Text(text)) if text == "a\"b"));
        assert!(matches!(parse("-42}", &[]), Ok(Expression::Integer(-42))));
    }

    #[test]
    fn rejects_everything_else() {
        assert!(parse("data.a + 1}", &[]).is_err_and(|message| message.contains("`+`")));
        assert!(parse("orders}", &[]).is_err_and(|message| message.contains("unknown name")));
        assert!(parse("99999999999999999999}", &[]).is_err());
        assert!(parse("data.}", &[]).is_err());
        assert!(parse(r#""\n"}"#, &[]).is_err());
    }
}
