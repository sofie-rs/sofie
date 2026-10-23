use crate::asset::{Asset, CLIENT_SCRIPT, ENGINE_SCRIPT, STYLESHEET};
use crate::escape::push_escaped;
use crate::language::Language;

/// `allowEval` off removes eval paths, indicator styles off avoid an inline `<style>` the CSP
/// blocks, and a zero history cache keeps personal data out of `localStorage`. Only a 5xx is an
/// error htmx logs: the server sends 4xx as notices placed by `HX-Retarget`.
const HTMX_CONFIG: &str = r#"<meta name="htmx-config" content='{"allowEval":false,"allowScriptTags":false,"selfRequestsOnly":true,"includeIndicatorStyles":false,"historyCacheSize":0,"responseHandling":[{"code":"204","swap":false},{"code":"[234]..","swap":true},{"code":"5..","swap":true,"error":true}]}'>"#;

macro_rules! notice_region_open {
    ($($attribute:literal)?) => {
        concat!("<div id=\"sofie-notice\" role=\"status\" aria-live=\"polite\"", $(" ", $attribute,)?)
    };
}

pub const NOTICE_TARGET: &str = "#sofie-notice";

const TITLE_OPEN: &str = "<title>";
const TITLE_CLOSE: &str = "</title>";

/// Around what `<Head>` writes, which the page moves into `<head>`.
#[cfg(feature = "content")]
pub(crate) const HEAD_START: &str = "<!--sofie-head-->";
#[cfg(feature = "content")]
pub(crate) const HEAD_END: &str = "<!--/sofie-head-->";

/// So a notice from an earlier failure does not outlive the next success.
pub(crate) const CLEARED_NOTICE: &str =
    concat!(notice_region_open!("hx-swap-oob=\"true\""), "></div>");

/// engine.js's own notices: the attribute it reads, the key, and its English fallback.
const SCRIPT_NOTICES: [(&str, &str, &str); 5] = [
    (
        "data-unreachable",
        "notice.unreachable",
        "The server could not be reached. Try again.",
    ),
    (
        "data-failed",
        "notice.failed",
        "Something went wrong. Try again.",
    ),
    (
        "data-passkey-refused",
        "notice.passkey_refused",
        "The passkey was not accepted. Try again.",
    ),
    (
        "data-passkey-unsupported",
        "notice.passkey_unsupported",
        "This browser cannot use passkeys.",
    ),
    (
        "data-passkey-unused",
        "notice.passkey_unused",
        "The passkey was not used. Try again.",
    ),
];

#[derive(Debug)]
pub struct Page<'a> {
    pub title: &'a str,
    pub lang: &'a str,
    pub body: &'a str,
    /// Such as why a form posted without JavaScript was refused.
    pub notice: &'a [String],
    pub nonce: &'a str,
    pub islands: Option<&'a IslandLinks>,
    pub script_notices: ScriptNotices<'a>,
}

/// What engine.js says when a request fails, in the page's language.
#[derive(Clone, Copy, Debug)]
pub struct ScriptNotices<'a>(Language<'a>);

impl ScriptNotices<'static> {
    #[must_use]
    pub fn plain() -> Self {
        Self(Language::plain())
    }
}

#[cfg(feature = "i18n")]
impl<'a> ScriptNotices<'a> {
    #[must_use]
    pub fn new(language: crate::Language<'a>) -> Self {
        Self(language)
    }
}

impl ScriptNotices<'_> {
    /// English is the script's own, so a page in English carries none.
    fn push_attributes(self, html: &mut String) {
        for (attribute, key, english) in SCRIPT_NOTICES {
            let text = self.0.text(key, english);
            if text != english {
                html.push(' ');
                html.push_str(attribute);
                html.push_str("=\"");
                push_escaped(html, text);
                html.push('"');
            }
        }
    }
}

/// Outside the `islands` feature, so a `Page` has the same fields whichever crates enable it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IslandLinks {
    pub script: String,
    pub stylesheet: String,
}

impl IslandLinks {
    pub(crate) fn push_head(&self, html: &mut String, nonce: &str) {
        html.push_str("<link rel=\"stylesheet\" href=\"");
        push_escaped(html, &self.stylesheet);
        html.push_str("\" data-sofie-islands><script type=\"module\" src=\"");
        push_escaped(html, &self.script);
        html.push_str("\" nonce=\"");
        push_escaped(html, nonce);
        html.push_str("\" data-sofie-islands></script>");
    }
}

impl Page<'_> {
    #[must_use]
    pub fn render(&self) -> String {
        let (head, before, after) = self.split_head();
        let (title, body) = take_title([before, after]);
        let mut html = String::with_capacity(self.body.len() + 900);
        html.push_str("<!doctype html><html lang=\"");
        push_escaped(&mut html, self.lang);
        html.push_str(
            "\"><head><meta charset=\"utf-8\">\
             <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">",
        );
        if let Some(title) = title {
            html.push_str(title);
        } else if !head.starts_with(TITLE_OPEN) {
            html.push_str(TITLE_OPEN);
            push_escaped(&mut html, self.title);
            html.push_str(TITLE_CLOSE);
        }
        html.push_str(head);
        html.push_str(HTMX_CONFIG);
        html.push_str("<link rel=\"stylesheet\" href=\"");
        STYLESHEET.push_path(&mut html);
        html.push_str("\">");
        self.push_script(&mut html, CLIENT_SCRIPT);
        self.push_script(&mut html, ENGINE_SCRIPT);
        if let Some(islands) = self.islands {
            islands.push_head(&mut html, self.nonce);
        }
        html.push_str("</head><body>");
        html.push_str(notice_region_open!());
        self.script_notices.push_attributes(&mut html);
        html.push('>');
        push_notice(&mut html, self.notice);
        html.push_str("</div>");
        for part in body {
            html.push_str(part);
        }
        html.push_str("</body></html>");
        html
    }

    /// What a `<Head>` wrote, and the body before and after it.
    #[cfg(feature = "content")]
    fn split_head(&self) -> (&str, &str, &str) {
        let body = self.body;
        let found = body.find(HEAD_START).and_then(|start| {
            let end = start + body[start..].find(HEAD_END)?;
            Some((start, end))
        });
        let Some((start, end)) = found else {
            return ("", body, "");
        };
        (
            &body[start + HEAD_START.len()..end],
            &body[..start],
            &body[end + HEAD_END.len()..],
        )
    }

    #[cfg(not(feature = "content"))]
    fn split_head(&self) -> (&str, &str, &str) {
        ("", self.body, "")
    }

    /// An asset path is the engine's own text and needs no escaping.
    fn push_script(&self, html: &mut String, script: Asset) {
        html.push_str("<script src=\"");
        script.push_path(html);
        html.push_str("\" nonce=\"");
        push_escaped(html, self.nonce);
        html.push_str("\" defer></script>");
    }
}

/// A template's `<title>` and the body around it. Data is escaped and the parser lets a template
/// write one `<title>` outside an `<svg>`, at its top level, so the first outside one is the
/// template's; one inside names its image.
fn take_title([before, after]: [&str; 2]) -> (Option<&str>, [&str; 3]) {
    if let Some((title, start, end)) = split_title(before) {
        return (Some(title), [start, end, after]);
    }
    if let Some((title, start, end)) = split_title(after) {
        return (Some(title), [before, start, end]);
    }
    (None, [before, after, ""])
}

fn split_title(body: &str) -> Option<(&str, &str, &str)> {
    let start = first_title_outside_svg(body)?;
    let end = start + body[start..].find(TITLE_CLOSE)? + TITLE_CLOSE.len();
    Some((&body[start..end], &body[..start], &body[end..]))
}

fn first_title_outside_svg(body: &str) -> Option<usize> {
    let mut svg_depth = 0_isize;
    for (start, _) in body.match_indices('<') {
        let tag = &body[start..];
        if tag.starts_with("<svg") {
            svg_depth += 1;
        } else if tag.starts_with("</svg>") {
            svg_depth -= 1;
        } else if svg_depth == 0 && tag.starts_with(TITLE_OPEN) {
            return Some(start);
        }
    }
    None
}

#[must_use]
pub fn with_cleared_notice(mut fragment: String) -> String {
    fragment.push_str(CLEARED_NOTICE);
    fragment
}

#[must_use]
pub fn notice(messages: &[impl AsRef<str>]) -> String {
    let mut html = String::new();
    push_notice(&mut html, messages);
    html
}

fn push_notice(html: &mut String, messages: &[impl AsRef<str>]) {
    for message in messages {
        html.push_str("<p>");
        push_escaped(html, message.as_ref());
        html.push_str("</p>");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_notice_region_and_its_target_name_the_same_element() {
        let id = NOTICE_TARGET.trim_start_matches('#');
        assert!(notice_region_open!().contains(&format!("id=\"{id}\"")));
        assert!(CLEARED_NOTICE.contains(&format!("id=\"{id}\"")));
    }

    fn page(script_notices: ScriptNotices<'_>) -> String {
        Page {
            title: "Orders",
            lang: "tr",
            body: "",
            notice: &[],
            nonce: "n",
            islands: None,
            script_notices,
        }
        .render()
    }

    #[test]
    fn an_english_page_leaves_the_script_s_notices_to_it() {
        assert!(page(ScriptNotices::plain()).contains("aria-live=\"polite\"></div>"));
    }

    #[cfg(feature = "i18n")]
    #[test]
    fn the_script_s_notices_are_written_in_the_page_s_language() {
        let translations = crate::Translations::new(&["en", "tr"]).unwrap();
        let turkish = translations.negotiate(None, Some("tr"));

        let html = page(ScriptNotices::new(turkish));

        assert!(
            html.contains(
                "<div id=\"sofie-notice\" role=\"status\" aria-live=\"polite\" \
                 data-unreachable=\"Sunucuya ulaşılamadı. Lütfen yeniden deneyin.\" \
                 data-failed=\"Bir şeyler ters gitti. Lütfen yeniden deneyin.\""
            ),
            "{html}"
        );
        assert!(html.contains("data-passkey-unused=\"Geçiş anahtarı kullanılmadı."));
    }

    #[test]
    fn the_title_is_the_first_one_outside_every_svg() {
        let body = "<svg><title>Icon</title><svg><title>Inner</title></svg></svg>\
                    <title>Orders</title><svg><title>Row</title></svg>";

        let (title, before, after) = split_title(body).unwrap();

        assert_eq!(title, "<title>Orders</title>");
        assert!(before.ends_with("</svg></svg>"));
        assert_eq!(after, "<svg><title>Row</title></svg>");
    }

    #[test]
    fn a_title_only_inside_svgs_is_no_page_title() {
        assert!(split_title("<svg><title>Icon</title></svg><p>Text</p>").is_none());
    }

    #[test]
    fn a_fragment_carries_an_out_of_band_empty_notice() {
        let html = with_cleared_notice("<table></table>".to_owned());
        assert!(html.starts_with("<table></table><div id=\"sofie-notice\""));
        assert!(html.ends_with("hx-swap-oob=\"true\"></div>"));
    }
}
