use sofie_core::DisplayZone;
use sofie_values::{Instant, Money};

use crate::escape::push_escaped;
use crate::render::push_display;

/// Keeps an amount and its currency on one line.
const NO_BREAK_SPACE: char = '\u{a0}';

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

#[derive(Debug)]
pub(crate) struct Conventions {
    decimal: char,
    group: Option<char>,
    money: Placement,
    days: DayOrder,
}

#[derive(Debug)]
enum Placement {
    /// `1234.50 EUR`
    CodeAfter,
    /// `€1,234.50`
    SymbolBefore,
    /// `1.234,50 €`
    SymbolAfter,
}

#[derive(Debug)]
enum DayOrder {
    /// `2026-10-11`
    Iso,
    /// `Oct 11, 2026`
    MonthNameFirst,
    /// `11.10.2026`
    DayFirstDotted,
}

pub(crate) const NEUTRAL: Conventions = Conventions {
    decimal: '.',
    group: None,
    money: Placement::CodeAfter,
    days: DayOrder::Iso,
};

const ENGLISH: Conventions = Conventions {
    decimal: '.',
    group: Some(','),
    money: Placement::SymbolBefore,
    days: DayOrder::MonthNameFirst,
};

const GERMAN: Conventions = Conventions {
    decimal: ',',
    group: Some('.'),
    money: Placement::SymbolAfter,
    days: DayOrder::DayFirstDotted,
};

const TURKISH: Conventions = Conventions {
    decimal: ',',
    group: Some('.'),
    money: Placement::SymbolBefore,
    days: DayOrder::DayFirstDotted,
};

/// By language alone: regional differences within one are not modelled yet.
pub(crate) fn for_language(language: &str) -> &'static Conventions {
    match language {
        "en" => &ENGLISH,
        "de" => &GERMAN,
        "tr" => &TURKISH,
        _ => &NEUTRAL,
    }
}

impl Conventions {
    /// The same rules as data, which a client area's runtime formats with.
    #[cfg(feature = "clients")]
    pub(crate) fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "decimal": self.decimal.to_string(),
            "group": self.group.map(|group| group.to_string()),
            "money": match self.money {
                Placement::CodeAfter => "code-after",
                Placement::SymbolBefore => "symbol-before",
                Placement::SymbolAfter => "symbol-after",
            },
            "days": match self.days {
                DayOrder::Iso => "iso",
                DayOrder::MonthNameFirst => "month-name-first",
                DayOrder::DayFirstDotted => "day-first-dotted",
            },
        })
    }

    /// A number in exponent form is written as it is.
    pub(crate) fn write_number(&self, out: &mut String, plain: &str) {
        if plain.contains(['e', 'E']) {
            out.push_str(plain);
            return;
        }
        let (sign, digits) = split_sign(plain);
        out.push_str(sign);
        let (whole, fraction) = digits
            .split_once('.')
            .map_or((digits, None), |(whole, fraction)| (whole, Some(fraction)));
        self.write_grouped(out, whole);
        if let Some(fraction) = fraction {
            out.push(self.decimal);
            out.push_str(fraction);
        }
    }

    fn write_grouped(&self, out: &mut String, whole: &str) {
        let Some(group) = self.group else {
            out.push_str(whole);
            return;
        };
        for (index, digit) in whole.char_indices() {
            if index > 0 && (whole.len() - index).is_multiple_of(3) {
                out.push(group);
            }
            out.push(digit);
        }
    }

    /// A currency code is three uppercase letters by construction, so nothing here needs escaping.
    pub(crate) fn write_money(&self, out: &mut String, amount: Money, scratch: &mut String) {
        let currency = amount.currency();
        let code = currency.as_str();
        scratch.clear();
        amount.write_major(scratch);
        match self.money {
            Placement::CodeAfter => {
                self.write_number(out, scratch);
                out.push(' ');
                out.push_str(code);
            }
            Placement::SymbolBefore => {
                let (sign, digits) = split_sign(scratch);
                out.push_str(sign);
                if let Some(symbol) = symbol(code) {
                    out.push_str(symbol);
                } else {
                    out.push_str(code);
                    out.push(NO_BREAK_SPACE);
                }
                self.write_number(out, digits);
            }
            Placement::SymbolAfter => {
                self.write_number(out, scratch);
                out.push(NO_BREAK_SPACE);
                out.push_str(symbol(code).unwrap_or(code));
            }
        }
    }

    /// Only the text's shape is looked at; anything else is written escaped.
    pub(crate) fn write_date(&self, out: &mut String, text: &str, scratch: &mut String) {
        if is_day(text) {
            self.write_day(out, text);
            return;
        }
        if let Some((day, minute)) = wall_clock(text) {
            self.write_day_and_minute(out, day, minute);
            return;
        }
        let Ok(instant) = Instant::parse(text) else {
            push_escaped(out, text);
            return;
        };
        self.write_instant(out, instant, DisplayZone::current(), scratch);
    }

    pub(crate) fn write_instant(
        &self,
        out: &mut String,
        instant: Instant,
        zone: &DisplayZone,
        scratch: &mut String,
    ) {
        scratch.clear();
        push_display(scratch, zone.wall_clock(instant));
        let (Some(day), Some(minute)) = (scratch.get(..10), scratch.get(11..16)) else {
            out.push_str(scratch);
            return;
        };
        self.write_day_and_minute(out, day, minute);
        out.push(' ');
        if let Some(offset) = zone.repeated_offset(instant) {
            out.push_str(&offset);
            out.push(' ');
        }
        push_escaped(out, zone.name());
    }

    /// On its own wall clock, so its zone is named rather than UTC.
    pub(crate) fn write_local_date_time(&self, out: &mut String, local: &str, zone: &str) {
        match (local.get(..10), local.get(10..11), local.get(11..16)) {
            (Some(day), Some("T"), Some(minute)) if is_day(day) && is_minute(minute) => {
                self.write_day_and_minute(out, day, minute);
            }
            _ => push_escaped(out, local),
        }
        out.push(' ');
        push_escaped(out, zone);
    }

    fn write_day_and_minute(&self, out: &mut String, day: &str, minute: &str) {
        self.write_day(out, day);
        out.push_str(match self.days {
            DayOrder::MonthNameFirst => ", ",
            DayOrder::Iso | DayOrder::DayFirstDotted => " ",
        });
        out.push_str(minute);
    }

    fn write_day(&self, out: &mut String, iso: &str) {
        let parts = (iso.get(..4), iso.get(5..7), iso.get(8..10));
        let (Some(year), Some(month), Some(day), 10) = (parts.0, parts.1, parts.2, iso.len())
        else {
            out.push_str(iso);
            return;
        };
        match self.days {
            DayOrder::Iso => out.push_str(iso),
            DayOrder::MonthNameFirst => {
                let month_name = month
                    .parse::<usize>()
                    .ok()
                    .and_then(|number| MONTHS.get(number.checked_sub(1)?))
                    .unwrap_or(&"");
                out.push_str(month_name);
                out.push(' ');
                out.push_str(day.trim_start_matches('0'));
                out.push_str(", ");
                out.push_str(year);
            }
            DayOrder::DayFirstDotted => {
                for part in [day, ".", month, ".", year] {
                    out.push_str(part);
                }
            }
        }
    }
}

fn is_day(text: &str) -> bool {
    text.len() == 10
        && text.bytes().enumerate().all(|(index, byte)| match index {
            4 | 7 => byte == b'-',
            _ => byte.is_ascii_digit(),
        })
}

/// A date-time without an offset (a `LocalDateTime`'s `local`) names no instant, so it is
/// written as it reads rather than in the display zone.
fn wall_clock(text: &str) -> Option<(&str, &str)> {
    let (day, rest) = (text.get(..10)?, text.get(10..)?);
    let minute = rest.strip_prefix('T')?.get(..5)?;
    let seconds = rest.get(6..)?;
    let bare = seconds
        .bytes()
        .all(|byte| byte.is_ascii_digit() || byte == b':' || byte == b'.');
    (is_day(day) && is_minute(minute) && bare).then_some((day, minute))
}

fn is_minute(text: &str) -> bool {
    text.len() == 5
        && text.bytes().enumerate().all(|(index, byte)| match index {
            2 => byte == b':',
            _ => byte.is_ascii_digit(),
        })
}

fn split_sign(plain: &str) -> (&str, &str) {
    plain
        .strip_prefix('-')
        .map_or(("", plain), |digits| ("-", digits))
}

/// Only symbols that name one currency wherever they are read.
/// The currencies written with a symbol rather than their code.
pub const CURRENCY_SYMBOLS: [(&str, &str); 5] = [
    ("EUR", "€"),
    ("USD", "$"),
    ("GBP", "£"),
    ("JPY", "¥"),
    ("TRY", "₺"),
];

fn symbol(code: &str) -> Option<&'static str> {
    CURRENCY_SYMBOLS
        .iter()
        .find(|(known, _)| *known == code)
        .map(|(_, symbol)| *symbol)
}

#[cfg(test)]
mod tests {
    use sofie_core::DisplayZone;
    use sofie_values::{Currency, Instant, Money};

    use super::{Conventions, NEUTRAL, for_language};

    fn number(conventions: &Conventions, plain: &str) -> String {
        let mut out = String::new();
        conventions.write_number(&mut out, plain);
        out
    }

    fn money(language: &str, minor: i64, code: &str) -> String {
        let mut out = String::new();
        let amount = Money::new(minor, Currency::parse(code).unwrap());
        for_language(language).write_money(&mut out, amount, &mut String::new());
        out
    }

    fn date(language: &str, text: &str) -> String {
        let mut out = String::new();
        for_language(language).write_date(&mut out, text, &mut String::new());
        out
    }

    #[test]
    fn groups_and_separates_numbers_by_language() {
        assert_eq!(number(for_language("en"), "-1234567.25"), "-1,234,567.25");
        assert_eq!(number(for_language("de"), "1234.5"), "1.234,5");
        assert_eq!(number(for_language("tr"), "123"), "123");
        assert_eq!(number(&NEUTRAL, "1234.5"), "1234.5");
        assert_eq!(number(for_language("en"), "1e300"), "1e300");
    }

    #[test]
    fn places_the_currency_by_language() {
        assert_eq!(money("en", 123_450, "EUR"), "€1,234.50");
        assert_eq!(money("en", -5, "USD"), "-$0.05");
        assert_eq!(money("en", 1500, "CHF"), "CHF\u{a0}15.00");
        assert_eq!(money("de", 123_450, "EUR"), "1.234,50\u{a0}€");
        assert_eq!(money("tr", 123_450, "TRY"), "₺1.234,50");
        assert_eq!(money("fr", 123_450, "EUR"), "1234.50 EUR");
    }

    #[test]
    fn writes_days_and_instants_by_language() {
        assert_eq!(date("en", "2026-10-01"), "Oct 1, 2026");
        assert_eq!(date("de", "2026-10-01"), "01.10.2026");
        assert_eq!(date("fr", "2026-10-01"), "2026-10-01");
        assert_eq!(
            date("en", "2026-10-11T23:30:00+03:00"),
            "Oct 11, 2026, 20:30 UTC"
        );
        assert_eq!(date("tr", "2026-10-11T09:05:59.5Z"), "11.10.2026 09:05 UTC");
        assert_eq!(date("en", "soon <b>"), "soon &lt;b&gt;");
    }

    #[test]
    fn writes_an_instant_on_the_display_zone_s_wall_clock_naming_the_zone() {
        let zone = DisplayZone::named("Europe/Istanbul").unwrap();
        let instant = Instant::parse("2026-10-11T22:30:00Z").unwrap();
        let mut out = String::new();

        for_language("en").write_instant(&mut out, instant, &zone, &mut String::new());

        assert_eq!(out, "Oct 12, 2026, 01:30 Europe/Istanbul");
    }

    #[test]
    fn writes_the_offset_before_the_zone_where_the_wall_clock_repeats() {
        let zone = DisplayZone::named("Europe/Berlin").unwrap();
        let written = |at: &str| {
            let mut out = String::new();
            let instant = Instant::parse(at).unwrap();
            for_language("de").write_instant(&mut out, instant, &zone, &mut String::new());
            out
        };

        assert_eq!(
            written("2026-10-25T00:30:00Z"),
            "25.10.2026 02:30 +02:00 Europe/Berlin"
        );
        assert_eq!(
            written("2026-10-25T01:30:00Z"),
            "25.10.2026 02:30 +01:00 Europe/Berlin"
        );
        assert_eq!(
            written("2026-10-25T02:30:00Z"),
            "25.10.2026 03:30 Europe/Berlin"
        );
    }

    #[test]
    fn a_date_time_without_an_offset_is_written_on_its_own_wall_clock() {
        assert_eq!(date("en", "2026-10-20T09:30:00"), "Oct 20, 2026, 09:30");
        assert_eq!(date("de", "2026-10-20T09:30"), "20.10.2026 09:30");
    }
}
