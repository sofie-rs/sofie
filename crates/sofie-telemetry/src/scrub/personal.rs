use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::ops::Range;

use unicode_normalization::char::is_combining_mark;

use super::run_until;

/// `jane@example.com`, `josé@münchen.de`, also as a URL's user: the domain needs a dot and an
/// alphabetic or `xn--` top level.
pub(super) fn email_addresses(text: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut from = 0;
    while let Some(found) = text[from..].find('@') {
        let at = from + found;
        from = at + 1;
        let start = local_part_start(text, at);
        if let Some(end) = domain_end(text, at + 1).filter(|_| start < at) {
            ranges.push(start..end);
            from = end;
        }
    }
    ranges
}

/// Letters, digits and marks of any script, so a decomposed `é` stays inside its word.
fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || is_combining_mark(character)
}

/// `Email`'s local-part characters; symbols before the first letter or digit, such as a quote
/// or a URL's `//`, are left outside.
fn local_part_start(text: &str, at: usize) -> usize {
    let start = text[..at]
        .char_indices()
        .rev()
        .find(|(_, character)| !is_local_character(*character))
        .map_or(0, |(before, character)| before + character.len_utf8());
    text[start..at]
        .find(is_word_character)
        .map_or(at, |offset| start + offset)
}

fn is_local_character(character: char) -> bool {
    is_word_character(character) || "!#$%&'*+-/=?^_`{|}~.".contains(character)
}

fn domain_end(text: &str, from: usize) -> Option<usize> {
    let is_label_end = |character: char| !(is_word_character(character) || character == '-');
    let mut label_start = from;
    let mut labels = 0;
    loop {
        let label_end = text[label_start..]
            .find(is_label_end)
            .map_or(text.len(), |offset| label_start + offset);
        if label_end == label_start {
            return None;
        }
        labels += 1;
        let continues = text[label_end..]
            .strip_prefix('.')
            .and_then(|rest| rest.chars().next())
            .is_some_and(is_word_character);
        if !continues {
            let is_top_level = is_top_level(&text[label_start..label_end]);
            return (labels >= 2 && is_top_level).then_some(label_end);
        }
        label_start = label_end + 1;
    }
}

/// `com`, `рф`, or its ASCII spelling `xn--p1ai`; never digits, as in `v1.2`.
fn is_top_level(label: &str) -> bool {
    let is_punycode = label.len() > "xn--".len()
        && label.as_bytes()[.."xn--".len()].eq_ignore_ascii_case(b"xn--");
    is_punycode
        || (label.chars().nth(1).is_some()
            && label
                .chars()
                .all(|character| character.is_alphabetic() || is_combining_mark(character)))
}

/// Runs of hex digits, dots and colons standing alone as a word, checked by the standard parsers.
pub(super) fn ip_addresses(text: &str) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    if !bytes.iter().any(|byte| matches!(byte, b'.' | b':')) {
        return ranges;
    }
    let mut at = 0;
    while at < bytes.len() {
        if !is_address_byte(bytes[at]) {
            at += 1;
            continue;
        }
        let end = run_until(bytes, at, |byte| !is_address_byte(byte));
        let run = trimmed_run(bytes, at..end);
        let before = run.start.checked_sub(1).map(|previous| bytes[previous]);
        let stands_alone = !run.is_empty()
            && !before.is_some_and(is_word_byte)
            && !bytes.get(run.end).copied().is_some_and(is_word_byte);
        if stands_alone {
            push_addresses(text, run, &mut ranges);
        }
        at = end;
    }
    ranges
}

/// `ip:10.0.0.7` and `10.0.0.7:port` hold an address between separators; next to no word,
/// a separator may be the address's own, as in `[::1]` or `2001:db8::`.
fn trimmed_run(bytes: &[u8], run: Range<usize>) -> Range<usize> {
    let is_separator = |byte: &&u8| matches!(byte, b':' | b'.');
    let after_word = run
        .start
        .checked_sub(1)
        .is_some_and(|previous| is_word_byte(bytes[previous]));
    let before_word = bytes.get(run.end).copied().is_some_and(is_word_byte);
    let mut start = run.start;
    if after_word {
        start += bytes[run.clone()].iter().take_while(is_separator).count();
    }
    let mut end = run.end;
    if before_word {
        end -= bytes[start..end]
            .iter()
            .rev()
            .take_while(is_separator)
            .count();
    }
    start..end
}

fn is_address_byte(byte: u8) -> bool {
    byte.is_ascii_hexdigit() || byte == b'.' || byte == b':'
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn push_addresses(text: &str, run: Range<usize>, ranges: &mut Vec<Range<usize>>) {
    if let Some((end, address)) = ipv6_end(&text[run.clone()]) {
        if is_personal(IpAddr::V6(address)) {
            ranges.push(run.start..run.start + end);
        }
        return;
    }
    let mut piece_start = run.start;
    for piece in text[run].split(':') {
        let trimmed = piece.trim_end_matches('.');
        if ipv4(trimmed).is_some_and(|address| is_personal(IpAddr::V4(address))) {
            ranges.push(piece_start..piece_start + trimmed.len());
        }
        piece_start += piece.len() + 1;
    }
}

/// The server's own loopback or wildcard address says nothing about a person.
fn is_personal(address: IpAddr) -> bool {
    let address = address.to_canonical();
    !(address.is_loopback() || address.is_unspecified())
}

/// A trailing `.` or `:` ends the sentence; `12:30:45` never parses, and `::` alone holds no digit.
fn ipv6_end(run: &str) -> Option<(usize, Ipv6Addr)> {
    let colons = run.bytes().filter(|byte| *byte == b':').count();
    if colons < 2 || !run.bytes().any(|byte| byte.is_ascii_digit()) {
        return None;
    }
    [run.trim_end_matches('.'), run.trim_end_matches(['.', ':'])]
        .into_iter()
        .find_map(|candidate| Some((candidate.len(), candidate.parse().ok()?)))
}

fn ipv4(piece: &str) -> Option<Ipv4Addr> {
    let dots = piece.bytes().filter(|byte| *byte == b'.').count();
    (dots == 3).then(|| piece.parse().ok()).flatten()
}

#[cfg(test)]
mod tests {
    use super::super::scrubbed_text;

    fn clean(text: &str) -> String {
        scrubbed_text(text).unwrap_or_else(|| text.to_owned())
    }

    #[test]
    fn email_addresses_in_free_text_are_masked() {
        let cases = [
            (
                "sent the receipt to jane.doe+shop@example.co.uk.",
                "sent the receipt to ***.",
            ),
            (
                "from <a@b.io>, to x_y@mail.example.com",
                "from <***>, to ***",
            ),
            (
                "fetching https://jane@example.com:pw@host/feed",
                "fetching https://***:***@host/feed",
            ),
            ("o'brien@example.com bounced", "*** bounced"),
            ("sent to 'jane&co@example.com'", "sent to '***'"),
        ];
        for (text, expected) in cases {
            assert_eq!(clean(text), expected);
        }
    }

    #[test]
    fn email_addresses_in_any_script_are_masked_whole() {
        let cases = [
            ("sent to josé@example.com.", "sent to ***."),
            ("ayşe.yılmaz@gmail.com signed up", "*** signed up"),
            ("from jürgen@münchen.de", "from ***"),
            ("x@site.xn--p1ai bounced", "*** bounced"),
            ("to jose\u{301}@example.com now", "to *** now"),
            ("to иван@почта.рф now", "to *** now"),
            ("to राम@उदाहरण.भारत now", "to *** now"),
        ];
        for (text, expected) in cases {
            assert_eq!(clean(text), expected);
        }
    }

    #[test]
    fn ip_addresses_in_free_text_are_masked() {
        let cases = [
            ("refused 192.168.1.20 twice", "refused *** twice"),
            ("listening on 10.0.0.1:8080.", "listening on ***:8080."),
            ("peer [2001:db8::8a2e:370:7334]:443", "peer [***]:443"),
            (
                "from 2001:db8::1, then fe80::1%eth0",
                "from ***, then ***%eth0",
            ),
            ("mapped ::ffff:192.0.2.1 dropped", "mapped *** dropped"),
            ("client 10.1.2.3/24 blocked", "client ***/24 blocked"),
            ("ip:10.0.0.7", "ip:***"),
            ("host:2001:db8::1", "host:***"),
            ("client:10.0.0.7", "client:***"),
            ("10.0.0.7:port", "***:port"),
        ];
        for (text, expected) in cases {
            assert_eq!(clean(text), expected);
        }
    }

    #[test]
    fn text_shaped_like_an_address_but_not_one_is_left_alone() {
        for text in [
            "upgraded to 1.2.3 from v2.10.4",
            "at 2026-10-21T12:30:45.123Z took 1.50 ms",
            "ran at 12:30:45 for 3.5s",
            "order 550e8400-e29b-41d4-a716-446655440000 trace 4bf92f3577b34da6",
            "bound to :8080 and port 443",
            "256.1.1.1 and 1.2.3.4.5 are no addresses",
            "std::net::IpAddr and dead::beef",
            "mail jane@localhost or @scope/name",
            "mail jürgen@localhost, x@v1.2 or ü@xn--",
            "listening on 127.0.0.1:3000 and [::1]:3000",
            "bound to 0.0.0.0:8080, [::]:8080 and 127.8.9.10",
            "loopback ::ffff:127.0.0.1 and ::ffff:0.0.0.0",
            "ip:127.0.0.1 and host:::1 and 127.0.0.1:port",
        ] {
            assert_eq!(scrubbed_text(text), None, "{text}");
        }
    }
}
