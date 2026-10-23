use std::convert::Infallible;

use super::component::each_expression;
use super::cursor::Cursor;
use super::expression::{parse_expression, parse_string};
use crate::language::{Argument, Call};
use crate::message::{is_argument_name, is_key};
use crate::syntax::{Expression, LocatedError, Op, Position, Written};
#[cfg(feature = "i18n")]
use crate::translations::Format;

pub(super) fn parse_call(
    cursor: &mut Cursor<'_>,
    scope: &[String],
) -> Result<Option<Expression>, LocatedError> {
    let mut ahead = cursor.clone();
    let start = ahead.position();
    let name = ahead.take_identifier();
    if ahead.peek() != Some(b'(') {
        return Ok(None);
    }
    *cursor = ahead;
    cursor.advance();
    cursor.skip_whitespace();
    let call = match name {
        "t" => parse_translation(cursor, scope, start)?,
        "date" => Call::date(parse_formatted(cursor, scope)?, start),
        #[cfg(feature = "i18n")]
        "number" => Call::Format {
            format: Format::Number,
            value: parse_formatted(cursor, scope)?,
            position: start,
        },
        #[cfg(feature = "i18n")]
        "money" => Call::Format {
            format: Format::Money,
            value: parse_formatted(cursor, scope)?,
            position: start,
        },
        #[cfg(not(feature = "i18n"))]
        "number" | "money" => {
            bail!(
                start,
                "`{name}(…)` formats for the viewer's language, which needs the `i18n` feature \
                 (`sofie/i18n`)"
            );
        }
        _ => {
            bail!(
                start,
                "unknown function `{name}`; functions are t, number, money and date"
            );
        }
    };
    cursor.skip_whitespace();
    cursor.expect(")", &format!("to end `{name}(…)`"))?;
    Ok(Some(Expression::Call(Box::new(call))))
}

fn parse_translation(
    cursor: &mut Cursor<'_>,
    scope: &[String],
    start: Position,
) -> Result<Call, LocatedError> {
    let key_position = cursor.position();
    if cursor.peek() != Some(b'"') {
        return Err(cursor.error("`t` takes its key first, quoted, such as `t(\"orders.title\")`"));
    }
    let key = parse_string(cursor)?;
    if !is_key(&key) {
        bail!(
            key_position,
            "`{key}` is not a translation key: lowercase names joined by `.`, such as `orders.title`"
        );
    }
    let mut arguments: Vec<Argument> = Vec::new();
    cursor.skip_whitespace();
    while cursor.eat(",") {
        cursor.skip_whitespace();
        let position = cursor.position();
        let name = cursor.take_identifier();
        if !is_argument_name(name) || arguments.iter().any(|argument| argument.name == name) {
            bail!(
                position,
                "expected a new lowercase argument name such as `count`, found `{name}`"
            );
        }
        cursor.skip_whitespace();
        cursor.expect("=", "after the argument name")?;
        let value = parse_expression(cursor, scope)?;
        arguments.push(Argument {
            name: name.to_owned(),
            value,
        });
        cursor.skip_whitespace();
    }
    Ok(Call::translate(key, arguments, start, None))
}

fn parse_formatted(cursor: &mut Cursor<'_>, scope: &[String]) -> Result<Expression, LocatedError> {
    let value_position = cursor.position();
    let value = parse_expression(cursor, scope)?;
    if matches!(value, Expression::Call(_)) {
        bail!(
            value_position,
            "a format takes a value such as `data.total`, not another call"
        );
    }
    Ok(value)
}

pub(crate) fn each_call(ops: &mut [Op<Written>], visit: &mut impl FnMut(&mut Call)) {
    let Ok(()) = each_expression::<Infallible>(ops, &mut |expression| {
        visit_calls(expression, visit);
        Ok(())
    });
}

fn visit_calls(expression: &mut Expression, visit: &mut impl FnMut(&mut Call)) {
    match expression {
        Expression::Interpolated(parts) => {
            for part in parts {
                visit_calls(part, visit);
            }
        }
        Expression::Call(call) => {
            for argument in call.arguments_mut() {
                visit_calls(argument, visit);
            }
            visit(call);
        }
        _ => {}
    }
}
