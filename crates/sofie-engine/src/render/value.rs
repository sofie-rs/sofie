use std::borrow::Cow;
use std::fmt::{self, Display, Write};

use serde_json::Number;
use sofie_core::Shown;
use sofie_core::show::Fields;

use crate::escape::push_escaped;

/// `None` is a path that leads nowhere, which an island's props tell apart from `null`.
pub(crate) type Resolved<'a> = Option<Shown<'a>>;

pub(crate) fn is_truthy(resolved: Resolved<'_>) -> bool {
    match resolved {
        None | Some(Shown::Null) => false,
        Some(Shown::Bool(flag)) => flag,
        Some(Shown::Integer(number)) => number != 0,
        Some(Shown::Float(number)) => number.is_finite() && number.abs() > 0.0,
        Some(Shown::Number(number)) => !number.as_f64().is_some_and(|float| float.abs() <= 0.0),
        Some(Shown::Text(text)) => !text.is_empty(),
        Some(Shown::Uuid(_) | Shown::Instant(_)) => true,
        Some(Shown::Formatted(display)) => !display.to_string().is_empty(),
        Some(Shown::List(items)) => items.item(0).is_some(),
        Some(Shown::Record(fields)) => fields.name(0).is_some(),
    }
}

pub(crate) fn str_of(resolved: Resolved<'_>) -> Option<&str> {
    match resolved {
        Some(Shown::Text(text)) => Some(text),
        _ => None,
    }
}

/// Formatted values are written out, so they compare like the JSON string they become.
pub(crate) fn text_of(resolved: Resolved<'_>) -> Option<Cow<'_, str>> {
    match resolved {
        Some(Shown::Text(text)) => Some(Cow::Borrowed(text)),
        Some(Shown::Uuid(uuid)) => Some(Cow::Owned(uuid.to_string())),
        Some(Shown::Instant(instant)) => Some(Cow::Owned(instant.to_string())),
        Some(Shown::Formatted(display)) => Some(Cow::Owned(display.to_string())),
        _ => None,
    }
}

pub(crate) fn field_of<'a>(resolved: Resolved<'a>, name: &str) -> Resolved<'a> {
    fields_of(resolved)?.get(name)
}

pub(crate) fn items_of(resolved: Resolved<'_>) -> impl Iterator<Item = Shown<'_>> {
    let items = match resolved {
        Some(Shown::List(items)) => Some(items),
        _ => None,
    };
    (0..).map_while(move |index| items?.item(index))
}

pub(crate) fn fields_of(resolved: Resolved<'_>) -> Option<&dyn Fields> {
    match resolved {
        Some(Shown::Record(fields)) => Some(fields),
        _ => None,
    }
}

/// Names whose field is absent, such as `<field>_to` of a state that moves nowhere, are skipped.
#[cfg(any(feature = "islands", feature = "listing"))]
pub(crate) fn each_field(fields: &dyn Fields) -> impl Iterator<Item = (&str, Shown<'_>)> {
    fields
        .names()
        .filter_map(|name| Some((name, fields.get(name)?)))
}

/// Records and lists write nothing, so a template has to pick fields; a local date-time is
/// the one record read as a value.
pub(crate) fn write_plain(target: &mut String, resolved: Resolved<'_>) {
    match resolved {
        Some(Shown::Record(fields)) => {
            if let Some(text) = local_date_time(fields) {
                target.push_str(&text);
            }
        }
        Some(Shown::Number(number)) => write_number(target, number),
        Some(Shown::Float(float)) => Number::from_f64(float)
            .iter()
            .for_each(|number| write_number(target, number)),
        Some(Shown::Bool(flag)) => push_display(target, flag),
        Some(Shown::Integer(number)) => push_display(target, number),
        Some(Shown::Text(text)) => target.push_str(text),
        Some(Shown::Uuid(uuid)) => uuid.push_to(target),
        Some(Shown::Instant(instant)) => push_display(target, instant),
        Some(Shown::Formatted(display)) => push_display(target, display),
        None | Some(Shown::Null | Shown::List(_)) => {}
    }
}

/// `LocalDateTime`'s `{local, zone}`.
pub(crate) fn local_and_zone(fields: &dyn Fields) -> Option<(Cow<'_, str>, Cow<'_, str>)> {
    if fields.name(2).is_some() {
        return None;
    }
    Some((text_of(fields.get("local"))?, text_of(fields.get("zone"))?))
}

/// As `2026-10-20 09:30 Europe/Istanbul`.
fn local_date_time(fields: &dyn Fields) -> Option<String> {
    let (local, zone) = local_and_zone(fields)?;
    let minute = local
        .get(..16)
        .filter(|minute| minute.as_bytes().get(10) == Some(&b'T'))
        .unwrap_or(&local);
    Some(format!("{} {zone}", minute.replacen('T', " ", 1)))
}

pub(crate) fn write_escaped(out: &mut String, resolved: Resolved<'_>) {
    match resolved {
        Some(Shown::Text(text)) => push_escaped(out, text),
        Some(Shown::Uuid(uuid)) => uuid.push_to(out),
        Some(Shown::Formatted(display)) => push_display(&mut Escaping(out), display),
        Some(Shown::Record(fields)) => {
            if let Some(text) = local_date_time(fields) {
                push_escaped(out, &text);
            }
        }
        _ => write_plain(out, resolved),
    }
}

struct Escaping<'a>(&'a mut String);

impl Write for Escaping<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        push_escaped(self.0, text);
        Ok(())
    }
}

fn write_number(target: &mut String, number: &Number) {
    let start = target.len();
    push_display(target, number);
    if target
        .get(start..)
        .is_some_and(|written| written.ends_with(".0"))
    {
        target.truncate(target.len() - 2);
    }
}

pub(crate) fn push_display(target: &mut impl Write, value: impl Display) {
    // Every target writes into a `String`, which cannot fail.
    let _ = write!(target, "{value}");
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use sofie_core::{Show, Shown};

    use super::{is_truthy, str_of, text_of, write_escaped, write_plain};

    fn escaped(value: &serde_json::Value) -> String {
        let mut out = String::new();
        write_escaped(&mut out, Some(value.show()));
        out
    }

    #[test]
    fn renders_scalars_and_hides_structures() {
        assert_eq!(escaped(&json!(3.0)), "3");
        assert_eq!(escaped(&json!(2.5)), "2.5");
        assert_eq!(escaped(&json!(-7)), "-7");
        assert_eq!(escaped(&json!(true)), "true");
        assert_eq!(escaped(&json!(null)), "");
        assert_eq!(escaped(&json!({"a": 1})), "");
        assert_eq!(escaped(&json!([1])), "");
        assert_eq!(escaped(&json!("<b>")), "&lt;b&gt;");
    }

    #[test]
    fn a_local_date_time_reads_as_its_day_minute_and_zone() {
        let at = json!({"local": "2026-10-20T09:30:00", "zone": "Europe/Istanbul"});
        let odd = json!({"local": "<soon>", "zone": "UTC"});

        assert_eq!(escaped(&at), "2026-10-20 09:30 Europe/Istanbul");
        assert_eq!(escaped(&odd), "&lt;soon&gt; UTC");
        assert_eq!(
            escaped(&json!({"local": "x", "zone": "UTC", "more": 1})),
            ""
        );
    }

    #[test]
    fn an_id_reads_as_its_text_everywhere_a_string_of_it_would() {
        const TEXT: &str = "01234567-89ab-cdef-fedc-ba9876543210";
        let id = Some(Shown::Uuid(TEXT.parse().unwrap()));
        let (mut escaped, mut plain) = (String::new(), String::new());

        write_escaped(&mut escaped, id);
        write_plain(&mut plain, id);

        assert_eq!((escaped.as_str(), plain.as_str()), (TEXT, TEXT));
        assert_eq!(text_of(id).as_deref(), Some(TEXT));
        assert!(is_truthy(id));
        assert_eq!(
            str_of(id),
            None,
            "no borrowed text: it is written on demand"
        );
    }

    #[test]
    fn typed_values_write_as_their_json_does() {
        let mut out = String::new();
        write_escaped(&mut out, Some(Shown::Float(3.0)));
        write_escaped(&mut out, Some(Shown::Float(f64::NAN)));
        write_escaped(&mut out, Some(Shown::Formatted(&"<i>")));
        assert_eq!(out, "3&lt;i&gt;");
        assert!(!is_truthy(Some(Shown::Float(f64::INFINITY))));
    }

    #[cfg(any(feature = "islands", feature = "listing"))]
    #[test]
    fn a_large_json_object_is_walked_in_linear_time() {
        let object: serde_json::Map<String, serde_json::Value> = (0..40_000)
            .map(|index| (format!("key{index}"), serde_json::Value::from(index)))
            .collect();

        let last = super::each_field(&object).last().map(|(name, _)| name);

        assert_eq!(last, object.keys().next_back().map(String::as_str));
    }

    #[test]
    fn truthiness_follows_emptiness_and_zero() {
        for value in [
            json!(""),
            json!([]),
            json!({}),
            json!(0),
            json!(0.0),
            json!(false),
            json!(null),
        ] {
            assert!(!is_truthy(Some(value.show())), "{value}");
        }
        for value in [
            json!("a"),
            json!([0]),
            json!({"a": 0}),
            json!(-1),
            json!(0.5),
            json!(true),
        ] {
            assert!(is_truthy(Some(value.show())), "{value}");
        }
        assert!(!is_truthy(None));
    }
}
