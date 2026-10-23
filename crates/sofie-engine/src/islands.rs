//! Props travel as JSON in a `<script type="application/json">` child, which the browser never
//! runs, so they need no nonce and no attribute escaping.

use serde_json::{Map, Value};
use sofie_core::Shown;

use crate::asset::{Asset, CSS, JAVASCRIPT};
use crate::catalog::Kind;
use crate::escape::push_script_json;
use crate::render::{Resolved, each_field, items_of};
use crate::syntax::{Attribute, Expression, Op, Path, Position, Root, Written, is_lowercase_name};
use crate::{Template, TemplateError};

/// Names the HTML standard reserves although they contain a hyphen.
const RESERVED_NAMES: [&str; 8] = [
    "annotation-xml",
    "color-profile",
    "font-face",
    "font-face-src",
    "font-face-uri",
    "font-face-format",
    "font-face-name",
    "missing-glyph",
];

#[derive(Clone, Copy, Debug)]
pub struct IslandBundle {
    pub area: &'static str,
    pub script: Asset,
    pub stylesheet: Asset,
}

impl IslandBundle {
    #[must_use]
    pub const fn new(area: &'static str, script: &'static [u8], stylesheet: &'static [u8]) -> Self {
        Self {
            area,
            script: Asset::new(area, "islands.js", script, JAVASCRIPT),
            stylesheet: Asset::new(area, "islands.css", stylesheet, CSS),
        }
    }
}

pub use crate::page::IslandLinks;

impl IslandLinks {
    #[must_use]
    pub fn of(bundle: &IslandBundle) -> Self {
        Self {
            script: bundle.script.path(),
            stylesheet: bundle.stylesheet.path(),
        }
    }
}

/// Lowercase ASCII, as the island's file name is.
#[must_use]
pub fn is_island_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    let starts_with_letter = bytes.next().is_some_and(|byte| byte.is_ascii_lowercase());
    starts_with_letter
        && name.contains('-')
        && is_lowercase_name(name)
        && !RESERVED_NAMES.contains(&name)
}

/// `<Island name="{tag}" props={data} />`, built without the parser.
pub fn island_screen(key: &str, tag: &str) -> Result<Template, TemplateError> {
    let attribute = |name: &str, value| Attribute {
        name: name.to_owned(),
        value,
        position: Position::START,
    };
    let island = Written {
        kind: Kind::Island,
        attributes: vec![
            attribute("name", Expression::Authored(tag.to_owned())),
            attribute(
                "props",
                Expression::Path(Path {
                    root: Root::Data,
                    fields: Vec::new(),
                }),
            ),
        ],
        children: Vec::new(),
        position: Position::START,
    };
    crate::compiled::build(key, vec![Op::Component(island)])
}

pub(crate) fn push_props(html: &mut String, props: &Value) {
    html.push_str("<script type=\"application/json\">");
    push_script_json(html, props);
    html.push_str("</script>");
}

/// What a path leading nowhere gives is an empty object, so the island mounts with its defaults.
pub(crate) fn props_json(resolved: Resolved<'_>) -> Value {
    resolved.map_or_else(|| Value::Object(Map::new()), shown_json)
}

fn shown_json(shown: Shown<'_>) -> Value {
    match shown {
        Shown::Null => Value::Null,
        Shown::Bool(flag) => Value::Bool(flag),
        Shown::Integer(number) => Value::from(number),
        Shown::Float(number) => Value::from(number),
        Shown::Number(number) => Value::Number(number.clone()),
        Shown::Text(text) => Value::from(text),
        Shown::Uuid(uuid) => Value::String(uuid.to_string()),
        Shown::Instant(instant) => Value::String(instant.to_string()),
        Shown::Formatted(display) => Value::String(display.to_string()),
        Shown::List(_) => items_of(Some(shown)).map(shown_json).collect(),
        Shown::Record(fields) => each_field(fields)
            .map(|(name, value)| (name.to_owned(), shown_json(value)))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use sofie_core::Show;

    use super::*;

    #[test]
    fn props_are_the_json_of_what_they_show() {
        let props = json!({"rows": [{"n": 1, "f": 0.5, "s": "x", "b": true, "z": null}], "m": {}});

        assert_eq!(props_json(Some(props.show())), props);
        assert_eq!(props_json(Some(Shown::Formatted(&7))), json!("7"));
        assert_eq!(props_json(None), json!({}));
    }

    #[test]
    fn island_names_are_custom_element_names() {
        for name in ["order-chart", "a-1", "page-editor-2"] {
            assert!(is_island_name(name), "{name}");
        }
        for name in [
            "chart",
            "Order-chart",
            "1-chart",
            "-chart",
            "order_chart",
            "order-chärt",
            "font-face",
            "",
        ] {
            assert!(!is_island_name(name), "{name}");
        }
    }

    #[test]
    fn props_cannot_close_their_script_or_open_a_comment() {
        let props = json!({
            "note": "</script><script>alert(1)</script><!-- & \u{2028}",
            "count": 2
        });

        let mut html = String::new();
        push_props(&mut html, &props);

        assert_eq!(
            html,
            "<script type=\"application/json\">{\"count\":2,\"note\":\
             \"\\u003c/script\\u003e\\u003cscript\\u003ealert(1)\\u003c/script\\u003e\\u003c!-- \
             \\u0026 \\u2028\"}</script>"
        );
        let json = html
            .strip_prefix("<script type=\"application/json\">")
            .and_then(|rest| rest.strip_suffix("</script>"))
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(json).unwrap(), props);
    }

    #[test]
    fn a_page_loads_the_bundle_with_its_nonce() {
        let bundle = IslandBundle::new("member", b"export {};", b".a{}");
        let mut head = String::new();

        IslandLinks::of(&bundle).push_head(&mut head, "n0nce");

        let script = bundle.script.path();
        assert!(script.starts_with("/_sofie/member-") && script.ends_with(".islands.js"));
        assert_eq!(
            head,
            format!(
                "<link rel=\"stylesheet\" href=\"{}\" data-sofie-islands><script type=\"module\" \
                 src=\"{script}\" nonce=\"n0nce\" data-sofie-islands></script>",
                bundle.stylesheet.path()
            )
        );
    }
}
