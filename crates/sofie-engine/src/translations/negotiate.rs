use sofie_values::Locale;

/// Past this many tags nothing changes the choice, so the rest is not read.
const MAX_TAGS: usize = 16;

/// A preference, not input to refuse: `*`, malformed and zero-weight tags are left out.
pub(super) fn accepted(header: &str) -> Vec<Locale> {
    let mut weighted: Vec<(u16, usize, Locale)> = header
        .split(',')
        .take(MAX_TAGS)
        .enumerate()
        .filter_map(|(position, item)| {
            let mut parts = item.split(';');
            let locale = locale_of(parts.next().unwrap_or_default().trim())?;
            let weight = parts
                .find_map(|parameter| {
                    let (name, value) = parameter.trim().split_once('=')?;
                    name.eq_ignore_ascii_case("q").then_some(value)
                })
                .map_or(Some(1000), parse_weight)
                .filter(|weight| *weight > 0)?;
            Some((weight, position, locale))
        })
        .collect();
    weighted.sort_by(|left, right| right.0.cmp(&left.0).then(left.1.cmp(&right.1)));
    weighted.into_iter().map(|(_, _, locale)| locale).collect()
}

/// A variant or extension past the region (`de-DE-1996`) still names its language and region.
fn locale_of(tag: &str) -> Option<Locale> {
    let mut end = tag.len();
    loop {
        if let Ok(locale) = Locale::parse(&tag[..end]) {
            return Some(locale);
        }
        end = tag[..end].rfind(['-', '_'])?;
    }
}

fn parse_weight(text: &str) -> Option<u16> {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if fraction.len() > 3 || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let thousandths = fraction
        .bytes()
        .chain(std::iter::repeat(b'0'))
        .take(3)
        .fold(0, |sum, digit| sum * 10 + u16::from(digit - b'0'));
    match whole {
        "0" => Some(thousandths),
        "1" if thousandths == 0 => Some(1000),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{accepted, parse_weight};

    fn tags(header: &str) -> Vec<String> {
        accepted(header)
            .iter()
            .map(|locale| locale.as_str().to_owned())
            .collect()
    }

    #[test]
    fn orders_by_weight_then_position() {
        assert_eq!(
            tags("de;q=0.5, tr-TR, en;q=0.8, fr;q=0.8, *;q=0.1, es;q=0, x-bad"),
            ["tr-TR", "en", "fr", "de"]
        );
        assert_eq!(tags(""), Vec::<String>::new());
    }

    #[test]
    fn reads_a_weight_named_in_any_case_and_a_tag_past_its_region() {
        assert_eq!(tags("de;Q=0, en"), ["en"]);
        assert_eq!(
            tags("de-DE-1996, en-US-u-ca-gregory;q=0.5"),
            ["de-DE", "en-US"]
        );
    }

    #[test]
    fn reads_quality_values() {
        assert_eq!(parse_weight("0.125"), Some(125));
        assert_eq!(parse_weight("1.000"), Some(1000));
        assert_eq!(parse_weight("0"), Some(0));
        assert_eq!(parse_weight("1.5"), None);
        assert_eq!(parse_weight("0.1234"), None);
    }
}
