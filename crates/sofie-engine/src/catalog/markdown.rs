//! A strict subset of `CommonMark`, enough for content pages: headings, paragraphs,
//! lists, quotes, fenced code, rules, emphasis, code spans, links and images.
//! Raw HTML is shown as text, and a link or image whose scheme is not on the
//! allowlist goes nowhere, so an author cannot run script.

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::escape::{push_escaped, push_url};

#[cfg(test)]
fn to_html(markdown: &str) -> String {
    let mut out = String::new();
    push_html(&mut out, markdown);
    out
}

pub(crate) fn push_html(out: &mut String, markdown: &str) {
    let mut lines = markdown.lines().peekable();
    let mut anchors = HashMap::new();
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(fence) = trimmed.strip_prefix("```") {
            push_code_block(out, fence.trim(), &mut lines);
        } else if let Some((level, text)) = heading(trimmed) {
            push_heading(out, level, text, &mut anchors);
        } else if is_rule(trimmed) {
            out.push_str("<hr>\n");
        } else if let Some(quoted) = quote(trimmed) {
            let mut text = quoted.to_owned();
            while let Some(more) = lines.peek().and_then(|next| quote(next.trim())) {
                text.push(' ');
                text.push_str(more);
                lines.next();
            }
            out.push_str("<blockquote><p>");
            push_inline(out, &text);
            out.push_str("</p></blockquote>\n");
        } else if let Some((ordered, item)) = list_item(trimmed) {
            push_list(out, ordered, item, &mut lines);
        } else {
            let mut text = trimmed.to_owned();
            while let Some(next) = lines.peek().map(|next| next.trim()) {
                if next.is_empty() || starts_block(next) {
                    break;
                }
                text.push(' ');
                text.push_str(next);
                lines.next();
            }
            out.push_str("<p>");
            push_inline(out, &text);
            out.push_str("</p>\n");
        }
    }
}

fn starts_block(line: &str) -> bool {
    line.starts_with("```")
        || heading(line).is_some()
        || is_rule(line)
        || quote(line).is_some()
        || list_item(line).is_some()
}

fn heading(line: &str) -> Option<(usize, &str)> {
    let level = line.bytes().take_while(|byte| *byte == b'#').count();
    let text = line.get(level..)?;
    ((1..=6).contains(&level) && text.starts_with(' ')).then(|| (level, text.trim()))
}

fn is_rule(line: &str) -> bool {
    let Some(mark @ (b'-' | b'*')) = line.bytes().next() else {
        return false;
    };
    line.bytes().all(|byte| byte == mark || byte == b' ')
        && line.bytes().filter(|byte| *byte == mark).count() >= 3
}

fn quote(line: &str) -> Option<&str> {
    line.strip_prefix('>').map(str::trim)
}

/// `- item`, `* item` or `1. item`.
fn list_item(line: &str) -> Option<(bool, &str)> {
    if let Some(item) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
        return Some((false, item.trim()));
    }
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    let item = line.get(digits..)?.strip_prefix(". ")?;
    (digits > 0).then(|| (true, item.trim()))
}

/// A heading of no letters or digits has no anchor: an empty `id` is invalid HTML. The `h-` keeps
/// a heading from taking an id the engine or the app uses, such as `sofie-notice` or `history`.
fn push_heading(out: &mut String, level: usize, text: &str, anchors: &mut HashMap<String, usize>) {
    let _ = write!(out, "<h{level}");
    let anchor = anchor(text);
    if !anchor.is_empty() {
        let anchor = unique(&anchor, anchors);
        out.push_str(" id=\"h-");
        out.push_str(&anchor);
        out.push('"');
    }
    out.push('>');
    push_inline(out, text);
    let _ = writeln!(out, "</h{level}>");
}

/// Lowercase letters and digits joined by `-`, so a heading can be linked to.
fn anchor(text: &str) -> String {
    let mut anchor = String::new();
    for character in text.chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() {
            anchor.push(character);
        } else if !anchor.is_empty() && !anchor.ends_with('-') {
            anchor.push('-');
        }
    }
    anchor.trim_end_matches('-').to_owned()
}

/// A repeated heading is numbered as GitHub numbers it, `intro`, `intro-1`, `intro-2`, skipping
/// a number another heading already took. Each taken id keeps the next number to try after it,
/// so n equal headings cost n steps, not n².
fn unique(anchor: &str, anchors: &mut HashMap<String, usize>) -> String {
    let Some(next) = anchors.get(anchor).copied() else {
        anchors.insert(anchor.to_owned(), 1);
        return anchor.to_owned();
    };
    let mut number = next;
    let mut candidate = format!("{anchor}-{number}");
    while anchors.contains_key(&candidate) {
        number += 1;
        candidate = format!("{anchor}-{number}");
    }
    anchors.insert(anchor.to_owned(), number + 1);
    anchors.insert(candidate.clone(), 1);
    candidate
}

fn push_code_block<'a>(
    out: &mut String,
    language: &str,
    lines: &mut impl Iterator<Item = &'a str>,
) {
    out.push_str("<pre><code");
    let end = language
        .find(|character: char| !character.is_ascii_alphanumeric() && character != '-')
        .unwrap_or(language.len());
    let language = &language[..end];
    if !language.is_empty() {
        out.push_str(" class=\"language-");
        out.push_str(language);
        out.push('"');
    }
    out.push('>');
    for line in lines.by_ref() {
        if line.trim() == "```" {
            break;
        }
        push_escaped(out, line);
        out.push('\n');
    }
    out.push_str("</code></pre>\n");
}

fn push_list<'a>(
    out: &mut String,
    ordered: bool,
    first: &str,
    lines: &mut std::iter::Peekable<impl Iterator<Item = &'a str>>,
) {
    let tag = if ordered { "ol" } else { "ul" };
    let _ = writeln!(out, "<{tag}>");
    let mut item = first.to_owned();
    while let Some(next) = lines.peek() {
        let continued = next.starts_with([' ', '\t']) && !next.trim().is_empty();
        if continued && list_item(next.trim()).is_none() {
            item.push(' ');
            item.push_str(next.trim());
            lines.next();
            continue;
        }
        match list_item(next.trim()) {
            Some((same, text)) if same == ordered => {
                push_item(out, &item);
                text.clone_into(&mut item);
                lines.next();
            }
            _ => break,
        }
    }
    push_item(out, &item);
    let _ = writeln!(out, "</{tag}>");
}

fn push_item(out: &mut String, text: &str) {
    out.push_str("<li>");
    push_inline(out, text);
    out.push_str("</li>\n");
}

/// A `[` after the last `](…)` cannot open a link, so it is not searched from: a run of them
/// would make rendering quadratic.
fn push_inline(out: &mut String, text: &str) {
    let links_end = text
        .rfind(')')
        .and_then(|close| text[..close].rfind("]("))
        .unwrap_or(0);
    let mut rest = text;
    while let Some(character) = rest.chars().next() {
        let can_link = text.len() - rest.len() < links_end;
        let consumed = match character {
            '\\' => escaped_character(out, rest),
            '`' => code_span(out, rest),
            '*' => emphasis(out, rest, character),
            '_' if !ends_in_word(&text[..text.len() - rest.len()]) => {
                emphasis(out, rest, character)
            }
            '!' if can_link && rest.starts_with("![") => {
                link(out, &rest[1..], true).map(|used| used + 1)
            }
            '[' if can_link => link(out, rest, false),
            _ => None,
        };
        let used = consumed.unwrap_or_else(|| {
            push_escaped(out, &rest[..character.len_utf8()]);
            character.len_utf8()
        });
        rest = &rest[used..];
    }
}

fn escaped_character(out: &mut String, text: &str) -> Option<usize> {
    let next = text
        .get(1..2)
        .filter(|next| next.bytes().all(|byte| byte.is_ascii_punctuation()))?;
    push_escaped(out, next);
    Some(2)
}

fn code_span(out: &mut String, text: &str) -> Option<usize> {
    let end = text[1..].find('`')? + 1;
    out.push_str("<code>");
    push_escaped(out, &text[1..end]);
    out.push_str("</code>");
    Some(end + 1)
}

fn ends_in_word(text: &str) -> bool {
    text.chars().next_back().is_some_and(char::is_alphanumeric)
}

/// `**strong**` or `*emphasis*` (also with `_`), closed on the same line. As in `CommonMark`,
/// `_` neither opens nor closes inside a word, so `snake_case` stays as written.
fn emphasis(out: &mut String, text: &str, marker: char) -> Option<usize> {
    let (delimiter, tag) = if text[1..].starts_with(marker) {
        (&text[..2], "strong")
    } else {
        (&text[..1], "em")
    };
    let inner = &text[delimiter.len()..];
    let end = inner.find(delimiter)?;
    let after = &inner[end + delimiter.len()..];
    let closes_inside_word =
        marker == '_' && after.chars().next().is_some_and(char::is_alphanumeric);
    if end == 0 || inner.starts_with(' ') || closes_inside_word {
        return None;
    }
    let _ = write!(out, "<{tag}>");
    push_inline(out, &inner[..end]);
    let _ = write!(out, "</{tag}>");
    Some(delimiter.len() * 2 + end)
}

/// `[text](url)` or, after `!`, `![alt](src)`.
fn link(out: &mut String, text: &str, image: bool) -> Option<usize> {
    let label_end = text.find("](")?;
    let label = &text[1..label_end];
    let target_start = label_end + 2;
    let target_end = target_start + text[target_start..].find(')')?;
    let url = text[target_start..target_end].trim();
    if image {
        out.push_str("<img src=\"");
        push_url(out, url, false);
        out.push_str("\" alt=\"");
        push_escaped(out, label);
        out.push_str("\">");
    } else {
        out.push_str("<a href=\"");
        push_url(out, url, false);
        out.push_str("\">");
        push_inline(out, label);
        out.push_str("</a>");
    }
    Some(target_end + 1)
}

#[cfg(test)]
mod tests {
    use super::to_html;

    #[test]
    fn renders_the_blocks_of_a_page() {
        let markdown = "# Our story\n\nWe started\nin 2026.\n\n- one\n- two\n  continued\n\n\
                        1. first\n2. second\n\n> quoted\n> on.\n\n---\n\n```rust\nlet a = 1 < 2;\n```\n";

        assert_eq!(
            to_html(markdown),
            "<h1 id=\"h-our-story\">Our story</h1>\n<p>We started in 2026.</p>\n\
             <ul>\n<li>one</li>\n<li>two continued</li>\n</ul>\n\
             <ol>\n<li>first</li>\n<li>second</li>\n</ol>\n\
             <blockquote><p>quoted on.</p></blockquote>\n<hr>\n\
             <pre><code class=\"language-rust\">let a = 1 &lt; 2;\n</code></pre>\n"
        );
    }

    #[test]
    fn a_heading_id_cannot_take_an_id_the_page_uses() {
        assert_eq!(
            to_html("# Sofie notice"),
            "<h1 id=\"h-sofie-notice\">Sofie notice</h1>\n"
        );
    }

    #[test]
    fn a_heading_without_letters_or_digits_has_no_id() {
        assert_eq!(to_html("## !?"), "<h2>!?</h2>\n");
        assert_eq!(
            to_html("# Ünal & Co."),
            "<h1 id=\"h-ünal-co\">Ünal &amp; Co.</h1>\n"
        );
    }

    #[test]
    fn a_repeated_heading_gets_a_numbered_id() {
        assert_eq!(
            to_html("# Intro\n## Intro\n# Intro-1\n### Intro"),
            "<h1 id=\"h-intro\">Intro</h1>\n<h2 id=\"h-intro-1\">Intro</h2>\n\
             <h1 id=\"h-intro-1-1\">Intro-1</h1>\n<h3 id=\"h-intro-2\">Intro</h3>\n"
        );
    }

    #[test]
    fn many_repeated_headings_are_numbered_in_linear_time() {
        let markdown = "# a\n".repeat(30_000);

        let html = to_html(&markdown);

        assert!(html.ends_with("<h1 id=\"h-a-29999\">a</h1>\n"));
    }

    #[test]
    fn renders_inline_marks_links_and_images() {
        assert_eq!(
            to_html(
                "Read **the *full* guide**, `code` and [docs](/docs) ![logo](https://x.test/a.png)"
            ),
            "<p>Read <strong>the <em>full</em> guide</strong>, <code>code</code> and \
             <a href=\"/docs\">docs</a> <img src=\"https://x.test/a.png\" alt=\"logo\"></p>\n"
        );
    }

    #[test]
    fn an_underscore_inside_a_word_is_not_emphasis() {
        assert_eq!(
            to_html("snake_case_name, _em_ and in*ner*most"),
            "<p>snake_case_name, <em>em</em> and in<em>ner</em>most</p>\n"
        );
        assert_eq!(to_html("_a_b"), "<p>_a_b</p>\n");
    }

    #[test]
    fn a_run_of_brackets_renders_in_linear_time() {
        let brackets = "[".repeat(1_000_000);

        assert_eq!(
            to_html(&format!("[a](/x) {brackets}) ![b")),
            format!("<p><a href=\"/x\">a</a> {brackets}) ![b</p>\n")
        );
    }

    #[test]
    fn shows_html_as_text_and_sends_unsafe_links_nowhere() {
        assert_eq!(
            to_html("<script>alert(1)</script> [x](javascript:alert(1)) \\*not em\\*"),
            "<p>&lt;script&gt;alert(1)&lt;/script&gt; <a href=\"#\">x</a>) *not em*</p>\n"
        );
    }
}
