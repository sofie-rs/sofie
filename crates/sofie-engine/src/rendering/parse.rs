use super::{Asks, Gate};
use crate::parse::cursor::Cursor;
use crate::parse::expression::parse_string;
use crate::syntax::{Expression, LocatedError};

/// The name is a literal, so what a screen asks is known without rendering it.
pub(crate) fn parse_gate(cursor: &mut Cursor<'_>) -> Result<Option<Expression>, LocatedError> {
    let mut ahead = cursor.clone();
    let asks = match ahead.take_identifier() {
        "can" => Asks::Permission,
        "enabled" => Asks::Flag,
        "entitled" => Asks::Entitlement,
        _ => return Ok(None),
    };
    if !ahead.eat("(") {
        return Ok(None);
    }
    *cursor = ahead;
    cursor.skip_whitespace();
    let position = cursor.position();
    let name = if cursor.peek() == Some(b'"') {
        parse_string(cursor)?
    } else {
        String::new()
    };
    if !is_gate_name(&name, asks) {
        return Err(LocatedError::new(position, expected(asks)));
    }
    cursor.skip_whitespace();
    cursor.expect(")", "to end the condition")?;
    Ok(Some(Expression::Gate(Gate { asks, name })))
}

fn is_gate_name(name: &str, asks: Asks) -> bool {
    let axes: Vec<&str> = name.split('.').collect();
    let count = match asks {
        Asks::Permission => 4,
        Asks::Flag | Asks::Entitlement => 2,
    };
    let is_axis = |axis: &str| {
        (asks == Asks::Permission && axis == "*")
            || (axis.starts_with(|first: char| first.is_ascii_lowercase())
                && axis
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'))
    };
    axes.len() == count && axes.into_iter().all(is_axis)
}

fn expected(asks: Asks) -> &'static str {
    match asks {
        Asks::Permission => {
            "`can` takes a quoted permission, `module.model.field.action`, such as `can(\"order.order.total.read\")`"
        }
        Asks::Flag => {
            "`enabled` takes a quoted flag, `module.flag`, such as `enabled(\"order.split_payments\")`"
        }
        Asks::Entitlement => {
            "`entitled` takes a quoted entitlement, `module.entitlement`, such as `entitled(\"project.export\")`"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Asks, is_gate_name};

    #[test]
    fn permissions_have_four_axes_and_flags_two() {
        assert!(is_gate_name("order.order.*.cancel", Asks::Permission));
        assert!(is_gate_name("order.split_payments", Asks::Flag));
        assert!(!is_gate_name("order.*", Asks::Flag));
        assert!(!is_gate_name("order.order.cancel", Asks::Permission));
        assert!(!is_gate_name("Order.order.*.read", Asks::Permission));
    }
}
