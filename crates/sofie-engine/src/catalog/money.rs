use sofie_core::Shown;
use sofie_values::Currency;

use super::props::Props;
use crate::render::{Renderer, Resolved, fields_of, str_of};
use crate::syntax::{Expression, LocatedError};

#[derive(Debug)]
pub(crate) struct Money {
    pub value: Expression,
}

pub(super) fn build_money(props: &mut Props) -> Result<Money, LocatedError> {
    Ok(Money {
        value: props.required_path("value")?,
    })
}

pub(super) fn render_money<'a>(
    money: &'a Money,
    id: Option<&'a Expression>,
    renderer: &mut Renderer<'a>,
) {
    renderer.push_markup("<span class=\"sofie-money\"");
    renderer.write_id(id);
    renderer.push_markup(">");
    if let Some(amount) = money_of(renderer.resolve(&money.value)) {
        renderer.write_money(amount);
    }
    renderer.push_markup("</span>");
}

/// Money as sofie-values writes it: `minor` and `currency`, nothing else.
pub(crate) fn money_of(resolved: Resolved<'_>) -> Option<sofie_values::Money> {
    let fields = fields_of(resolved)?;
    let minor = match fields.get("minor")? {
        Shown::Integer(minor) => minor,
        Shown::Number(minor) => minor.as_i64()?,
        _ => return None,
    };
    let currency = Currency::parse(str_of(fields.get("currency"))?).ok()?;
    let is_money = fields.name(2).is_none();
    is_money.then(|| sofie_values::Money::new(minor, currency))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use sofie_core::Show;

    use super::money_of;
    use crate::language::Language;

    fn formatted(value: &serde_json::Value) -> Option<String> {
        let amount = money_of(Some(value.show()))?;
        let mut out = String::new();
        Language::plain().write_money(&mut out, amount, &mut String::new());
        Some(out)
    }

    #[test]
    fn formats_by_the_currency_minor_unit() {
        for (value, text) in [
            (json!({"minor": 14990, "currency": "EUR"}), "149.90 EUR"),
            (json!({"minor": 1500, "currency": "JPY"}), "1500 JPY"),
            (json!({"minor": 1005, "currency": "KWD"}), "1.005 KWD"),
            (json!({"minor": -5, "currency": "USD"}), "-0.05 USD"),
            (
                json!({"minor": i64::MIN, "currency": "EUR"}),
                "-92233720368547758.08 EUR",
            ),
        ] {
            assert_eq!(formatted(&value).as_deref(), Some(text), "{value}");
        }
    }

    #[test]
    fn rejects_other_shapes_and_invalid_currencies() {
        for value in [
            json!({"minor": 1.5, "currency": "EUR"}),
            json!({"minor": 1}),
            json!({"minor": 1, "currency": "<b>"}),
            json!({"minor": 1, "currency": "EUR", "note": "x"}),
            json!(5),
            json!("1.00 EUR"),
            json!(null),
        ] {
            assert_eq!(formatted(&value), None, "{value}");
        }
    }
}
