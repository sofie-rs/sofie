use super::cursor::Cursor;
use super::expression::{parse_braced, parse_expression, parse_string};
use super::is_tag_name_byte;
use super::scope::Scope;
use crate::syntax::{Attribute, Expression, LocatedError, Position};

pub(super) struct Attributes<'s> {
    pub(super) given: Vec<Attribute>,
    pub(super) is_self_closing: bool,
    pub(super) deferred: Option<Deferred<'s>>,
}

/// A value read once the names it may read are bound, such as a table row named by an `as`
/// written after it.
pub(super) struct Deferred<'s> {
    name: String,
    position: Position,
    value: Cursor<'s>,
}

impl Deferred<'_> {
    pub(super) fn parse(mut self, scope: &mut Scope) -> Result<Attribute, LocatedError> {
        let value = parse_value(&mut self.value, scope, &self.name, self.position)?;
        Ok(Attribute {
            name: self.name,
            value,
            position: self.position,
        })
    }
}

pub(super) fn parse_attributes(
    cursor: &mut Cursor<'_>,
    scope: &mut Scope,
    tag: &str,
    position: Position,
) -> Result<(Vec<Attribute>, bool), LocatedError> {
    let attributes = parse_attributes_deferring(cursor, scope, tag, position, None)?;
    Ok((attributes.given, attributes.is_self_closing))
}

pub(super) fn parse_attributes_deferring<'s>(
    cursor: &mut Cursor<'s>,
    scope: &mut Scope,
    tag: &str,
    position: Position,
    deferred_name: Option<&str>,
) -> Result<Attributes<'s>, LocatedError> {
    let mut attributes = Attributes {
        given: Vec::new(),
        is_self_closing: false,
        deferred: None,
    };
    loop {
        cursor.skip_whitespace();
        if cursor.eat("/>") {
            attributes.is_self_closing = true;
            return Ok(attributes);
        }
        if cursor.eat(">") {
            return Ok(attributes);
        }
        if cursor.is_at_end() {
            bail!(position, "`<{tag}` is never ended with `>`");
        }
        let attribute_position = cursor.position();
        let name = parse_name(cursor, tag)?;
        let is_given = attributes
            .given
            .iter()
            .any(|existing| written_name(&existing.name) == name)
            || attributes
                .deferred
                .as_ref()
                .is_some_and(|deferred| deferred.name == name);
        if is_given {
            bail!(attribute_position, "`{name}` is given twice on `<{tag}>`");
        }
        if deferred_name == Some(name) && cursor.eat("=") {
            cursor.skip_whitespace();
            attributes.deferred = Some(Deferred {
                name: name.to_owned(),
                position: attribute_position,
                value: cursor.clone(),
            });
            skip_value(cursor, name)?;
            continue;
        }
        let value = if cursor.eat("=") {
            cursor.skip_whitespace();
            if cursor.eat("{{") {
                let fields = parse_fields(cursor, scope, name)?;
                attributes.given.extend(fields);
                continue;
            }
            parse_value(cursor, scope, name, attribute_position)?
        } else {
            Expression::Authored(String::new())
        };
        attributes.given.push(Attribute {
            name: name.to_owned(),
            value,
            position: attribute_position,
        });
    }
}

fn parse_name<'s>(cursor: &mut Cursor<'s>, tag: &str) -> Result<&'s str, LocatedError> {
    let position = cursor.position();
    let name = cursor.take_while(|byte| is_tag_name_byte(byte) || byte == b'_');
    if name.is_empty() {
        return Err(cursor.unexpected(format_args!(
            "expected an attribute name or the end of `<{tag}>`"
        )));
    }
    if cursor.peek() == Some(b':') {
        bail!(
            position,
            "`{name}:` attribute names are not supported; SVG reads a plain `href` and HTML a plain `lang`"
        );
    }
    cursor.skip_whitespace();
    Ok(name)
}

/// `name={{field: value, …}}`, read as one attribute `name.field` per field; a written
/// attribute name never holds a `.`.
fn parse_fields(
    cursor: &mut Cursor<'_>,
    scope: &mut Scope,
    name: &str,
) -> Result<Vec<Attribute>, LocatedError> {
    let mut fields: Vec<Attribute> = Vec::new();
    loop {
        cursor.skip_whitespace();
        let position = cursor.position();
        let field = cursor.take_identifier();
        if field.is_empty() {
            return Err(cursor.unexpected(format_args!("expected a field name in `{name}`")));
        }
        let field_name = format!("{name}.{field}");
        if fields.iter().any(|given| given.name == field_name) {
            bail!(position, "`{name}` names the field `{field}` twice");
        }
        cursor.skip_whitespace();
        cursor.expect(":", "after the field name")?;
        let value = parse_expression(cursor, scope.names())?;
        scope.record(&value, position);
        fields.push(Attribute {
            name: field_name,
            value,
            position,
        });
        cursor.skip_whitespace();
        if !cursor.eat(",") {
            cursor.expect("}}", "after the fields")?;
            return Ok(fields);
        }
    }
}

/// `with` of a field `with.slot`.
fn written_name(name: &str) -> &str {
    name.split_once('.').map_or(name, |(written, _)| written)
}

/// Passes over a value as `parse_value` reads it, without resolving its names.
fn skip_value(cursor: &mut Cursor<'_>, name: &str) -> Result<(), LocatedError> {
    let Some(quote @ (b'"' | b'\'')) = cursor.peek() else {
        if cursor.peek() == Some(b'{') {
            return skip_braced(cursor);
        }
        return Err(cursor.unexpected(format_args!(
            "expected `\"`, `'` or `{{` to start the value of `{name}`"
        )));
    };
    let start = cursor.position();
    cursor.advance();
    loop {
        cursor.take_while(|byte| byte != quote && byte != b'{');
        match cursor.peek() {
            Some(byte) if byte == quote => break,
            Some(_) => skip_braced(cursor)?,
            None => bail!(
                start,
                "attribute value is never closed with `{}`",
                char::from(quote)
            ),
        }
    }
    cursor.advance();
    Ok(())
}

fn skip_braced(cursor: &mut Cursor<'_>) -> Result<(), LocatedError> {
    let start = cursor.position();
    cursor.advance();
    loop {
        match cursor.peek() {
            Some(b'"') => {
                parse_string(cursor)?;
            }
            Some(b'}') => break,
            Some(_) => cursor.advance(),
            None => bail!(start, "`{{` is never closed with `}}`"),
        }
    }
    cursor.advance();
    Ok(())
}

fn parse_value(
    cursor: &mut Cursor<'_>,
    scope: &mut Scope,
    name: &str,
    position: Position,
) -> Result<Expression, LocatedError> {
    match cursor.peek() {
        Some(quote @ (b'"' | b'\'')) => parse_quoted(cursor, scope, quote),
        Some(b'{') => {
            cursor.advance();
            let value = parse_braced(cursor, scope.names())?;
            scope.record(&value, position);
            Ok(value)
        }
        _ => Err(cursor.unexpected(format_args!(
            "expected `\"`, `'` or `{{` to start the value of `{name}`"
        ))),
    }
}

/// A value is written back in double quotes, so a single-quoted one has its `"` escaped.
fn parse_quoted(
    cursor: &mut Cursor<'_>,
    scope: &mut Scope,
    quote: u8,
) -> Result<Expression, LocatedError> {
    let start = cursor.position();
    cursor.advance();
    let mut parts = Vec::new();
    loop {
        let text = cursor.take_while(|byte| byte != quote && byte != b'{');
        if !text.is_empty() {
            parts.push(Expression::Authored(text.replace('"', "&quot;")));
        }
        match cursor.peek() {
            Some(byte) if byte == quote => break,
            Some(_) => {
                let position = cursor.position();
                cursor.advance();
                let part = parse_braced(cursor, scope.names())?;
                scope.record(&part, position);
                parts.push(part);
            }
            None => {
                bail!(
                    start,
                    "attribute value is never closed with `{}`",
                    char::from(quote)
                );
            }
        }
    }
    cursor.advance();
    Ok(quoted_value(parts))
}

fn quoted_value(mut parts: Vec<Expression>) -> Expression {
    match parts.as_slice() {
        [] => Expression::Authored(String::new()),
        [Expression::Authored(_)] => parts.remove(0),
        _ => Expression::Interpolated(parts),
    }
}
