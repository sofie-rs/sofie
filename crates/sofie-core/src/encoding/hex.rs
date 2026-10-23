//! Lowercase when written; either case when read.

const DIGITS: &[u8; 16] = b"0123456789abcdef";

#[must_use]
pub fn encode(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    push(&mut text, bytes);
    text
}

pub fn push(text: &mut String, bytes: &[u8]) {
    for byte in bytes {
        text.extend(digits(*byte).map(char::from));
    }
}

#[must_use]
pub fn decode(text: &str) -> Option<Vec<u8>> {
    let (pairs, odd) = text.as_bytes().as_chunks::<2>();
    if !odd.is_empty() {
        return None;
    }
    pairs
        .iter()
        .map(|[high, low]| byte_of(*high, *low))
        .collect()
}

pub(crate) fn digits(byte: u8) -> [u8; 2] {
    [
        DIGITS[usize::from(byte >> 4)],
        DIGITS[usize::from(byte & 0x0f)],
    ]
}

/// Unlike `u8::from_str_radix`, a sign is not a digit.
pub(crate) fn digit_value(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
    }
}

pub(crate) fn byte_of(high: u8, low: u8) -> Option<u8> {
    Some(digit_value(high)? << 4 | digit_value(low)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_lowercase_and_appends() {
        assert_eq!(encode(&[]), "");
        assert_eq!(encode(&[0x00, 0x0f, 0xa0, 0xff]), "000fa0ff");
        let mut text = String::from("x");
        push(&mut text, &[0xab]);
        assert_eq!(text, "xab");
    }

    #[test]
    fn decodes_either_case_and_pairs_of_digits_only() {
        assert_eq!(decode(""), Some(vec![]));
        assert_eq!(decode("00ff10"), Some(vec![0, 255, 16]));
        assert_eq!(decode("A0fF"), Some(vec![0xa0, 0xff]));
        assert_eq!(decode("0"), None);
        assert_eq!(decode("zz"), None);
        assert_eq!(decode("+f"), None);
        assert_eq!(decode("é0"), None);
    }

    #[test]
    fn every_byte_round_trips() {
        let bytes: Vec<u8> = (0..=u8::MAX).collect();
        assert_eq!(decode(&encode(&bytes)), Some(bytes));
    }
}
