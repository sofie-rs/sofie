use memchr::{memchr2, memchr3};

/// An allowlist: `javascript:`, `data:` and any scheme invented later fall to `#`.
const SAFE_SCHEMES: [&str; 4] = ["http", "https", "mailto", "tel"];

const UNSAFE_URL_REPLACEMENT: &str = "#";

pub fn push_escaped(out: &mut String, text: &str) {
    let bytes = text.as_bytes();
    let markup = memchr3(b'&', b'<', b'>', bytes);
    let Some(first) = markup.into_iter().chain(memchr2(b'"', b'\'', bytes)).min() else {
        out.push_str(text);
        return;
    };
    out.push_str(text.get(..first).unwrap_or_default());
    push_escaped_bytewise(out, text.get(first..).unwrap_or_default());
}

fn push_escaped_bytewise(out: &mut String, text: &str) {
    let mut chunk_start = 0;
    for (index, byte) in text.bytes().enumerate() {
        let replacement = match byte {
            b'&' => "&amp;",
            b'<' => "&lt;",
            b'>' => "&gt;",
            b'"' => "&quot;",
            b'\'' => "&#39;",
            _ => continue,
        };
        out.push_str(text.get(chunk_start..index).unwrap_or_default());
        out.push_str(replacement);
        chunk_start = index + 1;
    }
    out.push_str(text.get(chunk_start..).unwrap_or_default());
}

pub(crate) fn push_url(out: &mut String, url: &str, is_authored: bool) {
    if !is_safe_url(url) {
        out.push_str(UNSAFE_URL_REPLACEMENT);
    } else if is_authored {
        out.push_str(url);
    } else {
        push_escaped(out, url);
    }
}

/// Browsers ignore control characters and spaces in a scheme (`java\tscript:`).
fn is_safe_url(url: &str) -> bool {
    let Some(end) = url.find([':', '/', '?', '#']) else {
        return true;
    };
    if !url.get(end..).is_some_and(|rest| rest.starts_with(':')) {
        return true;
    }
    let scheme = url
        .get(..end)
        .unwrap_or_default()
        .bytes()
        .filter(|byte| *byte > b' ')
        .map(|byte| byte.to_ascii_lowercase());
    SAFE_SCHEMES
        .iter()
        .any(|safe| scheme.clone().eq(safe.bytes()))
}

/// `<`, `>`, `&`, U+2028 and U+2029 are `\u`-escaped, so no text can close the element.
#[cfg(any(feature = "islands", feature = "clients"))]
pub(crate) fn push_script_json(html: &mut String, props: &serde_json::Value) {
    let json = props.to_string();
    let mut run_start = 0;
    for (index, character) in json.char_indices() {
        let replacement = match character {
            '<' => "\\u003c",
            '>' => "\\u003e",
            '&' => "\\u0026",
            '\u{2028}' => "\\u2028",
            '\u{2029}' => "\\u2029",
            _ => continue,
        };
        html.push_str(json.get(run_start..index).unwrap_or_default());
        html.push_str(replacement);
        run_start = index + character.len_utf8();
    }
    html.push_str(json.get(run_start..).unwrap_or_default());
}

#[cfg(test)]
mod tests {
    use super::{is_safe_url, push_escaped};

    #[cfg(any(feature = "islands", feature = "clients"))]
    #[test]
    fn script_json_escapes_what_could_end_the_element_and_keeps_the_rest() {
        let mut html = String::from("x");
        super::push_script_json(&mut html, &serde_json::json!("é</script>\u{2028}ü&"));
        assert_eq!(html, "x\"é\\u003c/script\\u003e\\u2028ü\\u0026\"");
    }

    #[test]
    fn escapes_the_five_html_characters() {
        let mut out = String::from("x");
        push_escaped(&mut out, "<a href=\"1\">'é' & b</a>");
        assert_eq!(
            out,
            "x&lt;a href=&quot;1&quot;&gt;&#39;é&#39; &amp; b&lt;/a&gt;"
        );
    }

    #[test]
    fn escapes_from_the_first_special_character_of_either_kind() {
        for (text, escaped) in [
            ("plain text, nothing to do", "plain text, nothing to do"),
            ("it's", "it&#39;s"),
            ("a \"b\" <c>", "a &quot;b&quot; &lt;c&gt;"),
            ("é & ü", "é &amp; ü"),
            ("", ""),
        ] {
            let mut out = String::new();
            push_escaped(&mut out, text);
            assert_eq!(out, escaped, "{text:?}");
        }
    }

    #[test]
    fn allows_relative_and_web_urls() {
        for url in [
            "/orders/1",
            "orders?a=b:c",
            "#top",
            "https://x.test",
            "MAILTO:a@b.test",
            "",
        ] {
            assert!(is_safe_url(url), "{url}");
        }
    }

    #[test]
    fn rejects_script_and_data_urls_in_any_disguise() {
        for url in [
            "javascript:alert(1)",
            " JaVaScRiPt:x",
            "java\tscript:x",
            "\u{1}javascript:x",
            "data:text/html,x",
            "vbscript:x",
        ] {
            assert!(!is_safe_url(url), "{url:?}");
        }
    }
}
