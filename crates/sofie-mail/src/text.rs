//! The plain text of a rendered template: what a mail client without HTML shows.

const BLOCKS: &[&str] = &[
    "p",
    "div",
    "section",
    "article",
    "header",
    "footer",
    "table",
    "tr",
    "ul",
    "ol",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "blockquote",
    "hr",
];

/// Read by a mail client as markup, never shown: their contents are left out.
const HIDDEN: &[&str] = &["head", "script", "style", "title"];
/// Past the longest entity name, a `&` without its `;` is plain text.
const LONGEST_ENTITY: usize = 32;

pub(crate) fn from_html(html: &str) -> String {
    let mut text = Text::default();
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        text.words(&decode(&rest[..start]));
        let tag = &rest[start..];
        if let Some(comment) = tag.strip_prefix("<!--") {
            rest = comment.find("-->").map_or("", |end| &comment[end + 3..]);
            continue;
        }
        let Some(end) = tag_end(tag) else {
            text.words("<");
            rest = &tag[1..];
            continue;
        };
        let inner = &tag[1..end];
        rest = &tag[end + 1..];
        let name = tag_name(inner);
        if !inner.starts_with('/') && HIDDEN.contains(&name.as_str()) {
            rest = after_closing(rest, &name);
            continue;
        }
        text.tag(inner, &name);
    }
    text.words(&decode(rest));
    text.out.trim().to_owned()
}

fn tag_name(tag: &str) -> String {
    tag.trim_start_matches('/')
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

fn after_closing<'h>(html: &'h str, name: &str) -> &'h str {
    let mut rest = html;
    while let Some(start) = rest.find("</") {
        let after = &rest[start + 2..];
        let closes = after
            .get(..name.len())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(name));
        if closes {
            return after.find('>').map_or("", |end| &after[end + 1..]);
        }
        rest = after;
    }
    ""
}

/// Only `<` before a name, `/` or `!` opens a tag; a `>` inside a quoted value does not end one.
fn tag_end(tag: &str) -> Option<usize> {
    let opens = tag
        .chars()
        .nth(1)
        .is_some_and(|next| next.is_ascii_alphabetic() || matches!(next, '/' | '!'));
    if !opens {
        return None;
    }
    let mut quote = None;
    for (index, character) in tag.char_indices().skip(1) {
        match (quote, character) {
            (None, '"' | '\'') => quote = Some(character),
            (Some(open), _) if open == character => quote = None,
            (None, '>') => return Some(index),
            _ => {}
        }
    }
    None
}

#[derive(Default)]
struct Text {
    out: String,
    space: bool,
    link: Option<Link>,
}

struct Link {
    href: String,
    label_start: usize,
}

impl Text {
    fn words(&mut self, decoded: &str) {
        for character in decoded.chars() {
            if character.is_whitespace() {
                self.space = true;
                continue;
            }
            if self.space && !self.out.is_empty() && !self.out.ends_with('\n') {
                self.out.push(' ');
            }
            self.space = false;
            self.out.push(character);
        }
    }

    fn tag(&mut self, tag: &str, name: &str) {
        let closing = tag.starts_with('/');
        match (name, closing) {
            ("br", _) => {
                self.out.push('\n');
                self.space = false;
            }
            ("li", false) => {
                if !self.out.is_empty() && !self.out.ends_with('\n') {
                    self.out.push('\n');
                }
                self.out.push_str("- ");
                self.space = false;
            }
            ("a", false) => {
                self.link = attribute(tag, "href").map(|href| Link {
                    href,
                    label_start: self.out.len(),
                });
            }
            ("a", true) => self.close_link(),
            (block, _) if BLOCKS.contains(&block) => self.paragraph(),
            _ => {}
        }
    }

    fn close_link(&mut self) {
        let Some(link) = self.link.take() else {
            return;
        };
        let label = self.out[link.label_start..].trim();
        let shown = link.href.starts_with("http") || link.href.starts_with("mailto:");
        if shown && label != link.href {
            self.out.push_str(" (");
            self.out.push_str(&link.href);
            self.out.push(')');
        }
    }

    fn paragraph(&mut self) {
        self.space = false;
        while !self.out.is_empty() && !self.out.ends_with("\n\n") {
            self.out.push('\n');
        }
    }
}

/// Walked by whole name, so `data-href` is not `href`.
fn attribute(tag: &str, wanted: &str) -> Option<String> {
    let mut rest = tag.trim_start_matches(|character: char| !character.is_whitespace());
    loop {
        rest = rest.trim_start();
        let name_end = rest
            .find(|character: char| character.is_whitespace() || matches!(character, '=' | '/'))
            .unwrap_or(rest.len());
        if name_end == 0 {
            return None;
        }
        let name = &rest[..name_end];
        rest = rest[name_end..].trim_start();
        let Some(after_equals) = rest.strip_prefix('=') else {
            rest = rest.trim_start_matches('/');
            continue;
        };
        let (value, after) = attribute_value(after_equals.trim_start());
        if name.eq_ignore_ascii_case(wanted) {
            return Some(decode(value));
        }
        rest = after;
    }
}

fn attribute_value(text: &str) -> (&str, &str) {
    if let Some(quote @ ('"' | '\'')) = text.chars().next() {
        let value = &text[1..];
        let end = value.find(quote).unwrap_or(value.len());
        return (&value[..end], value.get(end + 1..).unwrap_or(""));
    }
    let end = text.find(char::is_whitespace).unwrap_or(text.len());
    (&text[..end], &text[end..])
}

/// The entities the engine escapes with, those authored markup most often holds, and numeric ones.
fn decode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let window = &after.as_bytes()[..after.len().min(LONGEST_ENTITY)];
        let decoded = window
            .iter()
            .position(|byte| *byte == b';')
            .and_then(|end| Some((entity(&after[1..end])?, end)));
        if let Some((character, end)) = decoded {
            out.push(character);
            rest = &after[end + 1..];
        } else {
            out.push('&');
            rest = &after[1..];
        }
    }
    out.push_str(rest);
    out
}

fn entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some('\u{a0}'),
        _ => {
            let number = name.strip_prefix('#')?;
            let code = match number.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => number.parse().ok()?,
            };
            let character = char::from_u32(code)?;
            Some(if hides_in_body(character) {
                '\u{FFFD}'
            } else {
                character
            })
        }
    }
}

/// A body is text, not a name, so soft hyphens and zero-width spaces stay; bidi overrides could
/// make a shown link read as another (CVE-2021-42574).
fn hides_in_body(character: char) -> bool {
    (character.is_control() && !character.is_whitespace())
        || matches!(character, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

#[cfg(test)]
mod tests {
    use super::from_html;

    #[test]
    fn paragraphs_breaks_and_items_become_lines() {
        let html = "<section><h1>Your order</h1>\n  <p>Thanks   for\n buying.</p>\
                    <ul><li>Mug</li><li>Tea</li></ul><p>Line<br>break</p></section>";

        assert_eq!(
            from_html(html),
            "Your order\n\nThanks for buying.\n\n- Mug\n- Tea\n\nLine\nbreak"
        );
    }

    #[test]
    fn a_link_keeps_its_address_and_entities_are_decoded() {
        let html = "<p>Open <a href=\"https://shop.test/r?a=1&amp;b=2\">your receipt</a> \
                    or <a href='https://shop.test/x'>https://shop.test/x</a>.</p>\
                    <p>Tom &amp; Jerry &lt;3 &#39;quoted&#39; &#x41;</p>";

        assert_eq!(
            from_html(html),
            "Open your receipt (https://shop.test/r?a=1&b=2) or https://shop.test/x.\n\n\
             Tom & Jerry <3 'quoted' A"
        );
    }

    #[test]
    fn hidden_elements_and_comments_are_left_out() {
        let html = "<html><head><title>Receipt</title><style>p { color: red }</style></head>\
                    <body><!-- don't show --><p>Shown</p><SCRIPT>alert(1)</SCRIPT></body></html>";

        assert_eq!(from_html(html), "Shown");
    }

    #[test]
    fn an_entity_cannot_bring_back_a_control_or_bidi_character() {
        assert_eq!(
            from_html("a&#0;b&#x202E;c&#27;d"),
            "a\u{FFFD}b\u{FFFD}c\u{FFFD}d"
        );
    }

    #[test]
    fn an_entity_for_a_soft_hyphen_zero_width_space_or_tab_is_kept_as_text() {
        assert_eq!(
            from_html("Ex&#173;ample&#x200B;/path&#9;end"),
            "Ex\u{AD}ample\u{200B}/path end"
        );
    }

    #[test]
    fn only_the_href_attribute_itself_names_a_link() {
        let html = "<a data-href=\"https://evil.test\" href = https://shop.test/x>shop</a>";

        assert_eq!(from_html(html), "shop (https://shop.test/x)");
    }

    #[test]
    fn text_that_only_looks_like_markup_is_kept() {
        assert_eq!(from_html("a < b and <b>bold</b>"), "a < b and bold");
        assert_eq!(from_html("1 &unknown; 2 & 3"), "1 &unknown; 2 & 3");
    }
}
