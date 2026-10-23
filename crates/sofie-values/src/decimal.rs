use std::fmt::Write as _;
use std::iter;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    NotANumber,
    Character(char),
    TooManyDecimals,
    OutOfRange,
}

impl Refusal {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::NotANumber => NOT_A_NUMBER,
            Self::TooManyDecimals => "has too many decimals",
            Self::Character(character) => unexpected_character_message(character),
            Self::OutOfRange => "is out of range",
        }
    }
}

/// Digits are accumulated as integers, never through a float.
pub(crate) fn parse_scaled(text: &str, digits: u8) -> Result<i64, Refusal> {
    let text = text.trim();
    let (is_negative, unsigned) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    if let Some(unexpected) = whole
        .chars()
        .chain(fraction.chars())
        .find(|character| !character.is_ascii_digit())
    {
        return Err(Refusal::Character(unexpected));
    }
    if whole.is_empty() || (fraction.is_empty() && unsigned.ends_with('.')) {
        return Err(Refusal::NotANumber);
    }
    if fraction.len() > usize::from(digits) {
        return Err(Refusal::TooManyDecimals);
    }
    magnitude(whole, fraction, digits)
        .and_then(|magnitude| signed(magnitude, is_negative))
        .ok_or(Refusal::OutOfRange)
}

pub(crate) fn scaled_text(scaled: i64, digits: u8) -> String {
    let mut text = String::new();
    write_scaled(&mut text, scaled, digits);
    text
}

pub(crate) fn write_scaled(out: &mut String, scaled: i64, digits: u8) {
    let sign = if scaled < 0 { "-" } else { "" };
    let divisor = 10_u64.pow(u32::from(digits));
    let magnitude = scaled.unsigned_abs();
    let whole = magnitude / divisor;
    // Writing to a `String` cannot fail.
    if digits == 0 {
        let _ = write!(out, "{sign}{whole}");
        return;
    }
    let fraction = magnitude % divisor;
    let width = usize::from(digits);
    let _ = write!(out, "{sign}{whole}.{fraction:0width$}");
}

/// Rounded half away from zero, as people expect of an amount.
pub(crate) fn divide_rounded(numerator: i128, denominator: i128) -> Option<i64> {
    if denominator <= 0 {
        return None;
    }
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    let rounded = if remainder.abs() * 2 >= denominator {
        quotient + numerator.signum()
    } else {
        quotient
    };
    i64::try_from(rounded).ok()
}

const NOT_A_NUMBER: &str = "must be a number such as 149.90";

fn unexpected_character_message(character: char) -> &'static str {
    match character {
        character if character.is_whitespace() => "must not contain spaces",
        'e' | 'E' => "must be written out, without an exponent",
        '+' | '-' => "may only have a leading `-` as its sign",
        '.' | ',' | '_' | '\'' => "must use one `.` before the decimals and no other separator",
        _ => NOT_A_NUMBER,
    }
}

fn magnitude(whole: &str, fraction: &str, digits: u8) -> Option<u64> {
    let padding = usize::from(digits).saturating_sub(fraction.len());
    whole
        .bytes()
        .chain(fraction.bytes())
        .chain(iter::repeat_n(b'0', padding))
        .try_fold(0_u64, |total, digit| {
            total.checked_mul(10)?.checked_add(u64::from(digit - b'0'))
        })
}

fn signed(magnitude: u64, is_negative: bool) -> Option<i64> {
    if is_negative {
        return 0_i64.checked_sub_unsigned(magnitude);
    }
    i64::try_from(magnitude).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refusal(text: &str) -> &'static str {
        match parse_scaled(text, 2) {
            Err(refusal) => refusal.message(),
            Ok(read) => panic!("{text:?} was read as {read}"),
        }
    }

    #[test]
    fn reads_whole_partial_and_full_decimals() {
        for (text, read) in [
            ("149.90", 14_990),
            ("149.9", 14_990),
            ("149", 14_900),
            ("0.05", 5),
            ("007.5", 750),
            (" 12.30\n", 1_230),
            ("-0.05", -5),
            ("-149", -14_900),
            ("-0", 0),
        ] {
            assert_eq!(parse_scaled(text, 2), Ok(read), "{text:?}");
        }
    }

    #[test]
    fn reads_up_to_the_given_decimals() {
        assert_eq!(parse_scaled("1500", 0), Ok(1_500));
        assert_eq!(parse_scaled("1.005", 3), Ok(1_005));
        assert_eq!(parse_scaled("1.5", 3), Ok(1_500));
        assert_eq!(parse_scaled("1500.0", 0), Err(Refusal::TooManyDecimals));
        assert_eq!(parse_scaled("1.0005", 3), Err(Refusal::TooManyDecimals));
    }

    #[test]
    fn reads_up_to_the_i64_boundaries() {
        assert_eq!(parse_scaled("92233720368547758.07", 2), Ok(i64::MAX));
        assert_eq!(parse_scaled("-92233720368547758.08", 2), Ok(i64::MIN));
        assert_eq!(parse_scaled("9223372036854775807", 0), Ok(i64::MAX));
        for (text, digits) in [
            ("92233720368547758.08", 2),
            ("-92233720368547758.09", 2),
            ("9223372036854775808", 0),
            ("99999999999999999999999999", 2),
        ] {
            assert_eq!(
                parse_scaled(text, digits),
                Err(Refusal::OutOfRange),
                "{text}"
            );
        }
    }

    #[test]
    fn refuses_signs_other_than_a_leading_minus() {
        for text in ["+5", "5-", "--5", "-+5", "5.0-"] {
            assert_eq!(
                refusal(text),
                "may only have a leading `-` as its sign",
                "{text}"
            );
        }
    }

    #[test]
    fn refuses_exponents_spaces_and_other_separators() {
        let separator = "must use one `.` before the decimals and no other separator";
        for (text, message) in [
            ("1e3", "must be written out, without an exponent"),
            ("1.5E2", "must be written out, without an exponent"),
            ("1 000", "must not contain spaces"),
            ("- 5", "must not contain spaces"),
            ("1\u{a0}000", "must not contain spaces"),
            ("149,90", separator),
            ("1,000.00", separator),
            ("1_000", separator),
            ("1'000", separator),
            ("1.2.3", separator),
        ] {
            assert_eq!(refusal(text), message, "{text:?}");
        }
    }

    #[test]
    fn refuses_what_is_not_a_number() {
        for text in [
            "", " ", "-", ".5", "5.", "-.5", "abc", "€5", "0x10", "NaN", "١٢",
        ] {
            assert_eq!(refusal(text), NOT_A_NUMBER, "{text:?}");
        }
    }

    #[test]
    fn formats_every_decimal() {
        for (value, digits, text) in [
            (14_990, 2, "149.90"),
            (5, 2, "0.05"),
            (-5, 2, "-0.05"),
            (0, 2, "0.00"),
            (1_500, 0, "1500"),
            (-1_500, 0, "-1500"),
            (1_005, 3, "1.005"),
            (i64::MIN, 2, "-92233720368547758.08"),
            (i64::MAX, 2, "92233720368547758.07"),
        ] {
            assert_eq!(scaled_text(value, digits), text);
        }
    }

    #[test]
    fn formatting_and_parsing_round_trip() {
        for value in [i64::MIN, -1, 0, 1, 14_990, i64::MAX] {
            for digits in [0, 2, 3, 7] {
                let text = scaled_text(value, digits);
                assert_eq!(parse_scaled(&text, digits), Ok(value), "{text}");
            }
        }
    }

    #[test]
    fn divides_rounding_half_away_from_zero() {
        for (numerator, denominator, rounded) in [
            (10, 4, 3),
            (-10, 4, -3),
            (9, 4, 2),
            (-9, 4, -2),
            (11, 4, 3),
            (1, 3, 0),
            (2, 3, 1),
            (0, 7, 0),
        ] {
            assert_eq!(
                divide_rounded(numerator, denominator),
                Some(rounded),
                "{numerator}/{denominator}"
            );
        }
        assert_eq!(divide_rounded(1, 0), None);
        assert_eq!(divide_rounded(1, -1), None);
        assert_eq!(divide_rounded(i128::from(i64::MAX) * 2, 1), None);
    }
}
