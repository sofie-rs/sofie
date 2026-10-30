use super::program::URL_ATTRIBUTES;
use crate::escape::check_authored_url;
use crate::syntax::{Attribute, LocatedError, Position, is_lowercase_name};

/// The rest of HTML's void elements are forbidden below.
const VOID_ELEMENTS: [&str; 9] = [
    "area", "br", "col", "hr", "img", "input", "source", "track", "wbr",
];

/// Scripts and styles evade HTML escaping, `meta`/`base`/`link` change the whole page (a bound
/// `refresh` is an open redirect), embedded documents escape the page's policy, and SVG
/// animations can set any attribute, a link's `href` included, past the URL check.
const FORBIDDEN_ELEMENTS: [&str; 11] = [
    "script",
    "style",
    "meta",
    "base",
    "link",
    "iframe",
    "object",
    "embed",
    "animate",
    "set",
    "animatemotion",
];

/// htmx also reads `data-hx-*`, so both spellings belong to components only.
const HTMX_PREFIXES: [&str; 2] = ["hx-", "data-hx-"];

pub(super) fn is_void(tag: &str) -> bool {
    VOID_ELEMENTS.contains(&tag)
}

pub(super) fn check_element_name(tag: &str, position: Position) -> Result<(), LocatedError> {
    if !is_lowercase_name(tag) {
        bail!(
            position,
            "element name `{tag}` must be lowercase; capitalized names are catalog components"
        );
    }
    if FORBIDDEN_ELEMENTS.contains(&tag) {
        bail!(
            position,
            "`<{tag}>` is not allowed in templates; the page's head, scripts and styles come from the engine, anything richer is an island"
        );
    }
    Ok(())
}

pub(super) fn check_element_attribute(
    tag: &str,
    attribute: &Attribute,
) -> Result<(), LocatedError> {
    let name = attribute.name.as_str();
    let problem = if HTMX_PREFIXES.iter().any(|prefix| name.starts_with(prefix)) {
        format!(
            "`{name}` is not allowed on `<{tag}>`: htmx attributes come only from catalog components; use a component such as `Action`"
        )
    } else if name
        .get(..2)
        .is_some_and(|start| start.eq_ignore_ascii_case("on"))
    {
        format!(
            "event handler attribute `{name}` is not allowed; behaviour belongs in the engine or an island"
        )
    } else if let Some((written, _)) = name.split_once('.') {
        format!(
            "`{written}={{{{…}}}}` is not allowed on `<{tag}>`: posted fields belong on an `Action`"
        )
    } else if name == "style" {
        "`style` is not allowed: the Content-Security-Policy blocks inline styles; use a class from the stylesheet".to_owned()
    } else if !is_lowercase_name(name) {
        format!("attribute name `{name}` must be lowercase letters, digits and `-`")
    } else if URL_ATTRIBUTES.contains(&name) {
        return check_authored_url(attribute);
    } else {
        return Ok(());
    };
    Err(LocatedError::new(attribute.position, problem))
}

#[cfg(test)]
mod tests {
    use super::{check_element_attribute, check_element_name, is_void};
    use crate::syntax::{Attribute, Expression, Position};

    const START: Position = Position { line: 1, column: 1 };

    fn attribute(name: &str) -> Attribute {
        Attribute {
            name: name.to_owned(),
            value: Expression::Authored(String::new()),
            position: START,
        }
    }

    #[test]
    fn rejects_htmx_event_handler_and_style_attributes() {
        for name in [
            "hx-post",
            "data-hx-get",
            "onclick",
            "ONload",
            "style",
            "Class",
        ] {
            assert!(
                check_element_attribute("div", &attribute(name)).is_err(),
                "{name}"
            );
        }
        assert!(check_element_attribute("div", &attribute("data-id")).is_ok());
    }

    #[test]
    fn rejects_page_level_embedding_and_mixed_case_elements() {
        for tag in [
            "script", "style", "meta", "base", "link", "iframe", "object", "embed",
        ] {
            assert!(check_element_name(tag, START).is_err(), "{tag}");
        }
        assert!(check_element_name("dIv", START).is_err());
        assert!(check_element_name("my-island", START).is_ok());
        assert!(is_void("br") && !is_void("div"));
    }

    #[test]
    fn rejects_svg_animations_that_could_set_a_url() {
        for tag in ["animate", "set", "animatemotion"] {
            assert!(check_element_name(tag, START).is_err(), "{tag}");
        }
    }

    #[test]
    fn knows_every_allowed_void_element() {
        for tag in [
            "area", "br", "col", "hr", "img", "input", "source", "track", "wbr",
        ] {
            assert!(is_void(tag), "{tag}");
        }
    }
}
