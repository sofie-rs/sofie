pub struct Alphabet {
    symbols: &'static [u8; 64],
    values: [u8; 256],
}

const NOT_A_SYMBOL: u8 = u8::MAX;

impl Alphabet {
    const fn new(symbols: &'static [u8; 64]) -> Self {
        let mut values = [NOT_A_SYMBOL; 256];
        let mut value = 0;
        while value < symbols.len() {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "an alphabet has 64 symbols"
            )]
            let symbol_value = value as u8;
            values[symbols[value] as usize] = symbol_value;
            value += 1;
        }
        Self { symbols, values }
    }

    fn value_of(&self, symbol: u8) -> Option<u32> {
        let value = self.values[usize::from(symbol)];
        (value != NOT_A_SYMBOL).then_some(u32::from(value))
    }
}

/// RFC 4648 section 4: what key generators print.
pub static STANDARD: Alphabet =
    Alphabet::new(b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/");

/// RFC 4648 section 5: safe in a cookie and a URL.
pub static URL_SAFE: Alphabet =
    Alphabet::new(b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_");

#[must_use]
pub fn encode(alphabet: &Alphabet, bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len().div_ceil(3) * 4);
    push(&mut text, alphabet, bytes);
    text
}

/// Padded to a multiple of four, as MIME and SMTP expect.
#[must_use]
pub fn encode_padded(alphabet: &Alphabet, bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len().div_ceil(3) * 4);
    push_padded(&mut text, alphabet, bytes);
    text
}

pub fn push_padded(text: &mut String, alphabet: &Alphabet, bytes: &[u8]) {
    push(text, alphabet, bytes);
    let padding = (3 - bytes.len() % 3) % 3;
    text.extend(std::iter::repeat_n('=', padding));
}

fn push(text: &mut String, alphabet: &Alphabet, bytes: &[u8]) {
    for chunk in bytes.chunks(3) {
        let group = chunk.iter().enumerate().fold(0u32, |group, (index, byte)| {
            group | u32::from(*byte) << (16 - 8 * index)
        });
        for index in 0..=chunk.len() {
            let symbol = (group >> (18 - 6 * index)) & 0x3f;
            text.push(char::from(alphabet.symbols[symbol as usize]));
        }
    }
}

/// Canonical text only: padding absent or complete, and no bits set past the last byte,
/// so each byte string has exactly one accepted spelling.
#[must_use]
pub fn decode(alphabet: &Alphabet, text: &str) -> Option<Vec<u8>> {
    let symbols = text.trim_end_matches('=').as_bytes();
    let padding = text.len() - symbols.len();
    if symbols.len() % 4 == 1 || (padding != 0 && padding != (4 - symbols.len() % 4) % 4) {
        return None;
    }
    let mut bytes = Vec::with_capacity(symbols.len() * 3 / 4);
    for chunk in symbols.chunks(4) {
        let mut group = 0u32;
        for (index, symbol) in chunk.iter().enumerate() {
            group |= alphabet.value_of(*symbol)? << (18 - 6 * index);
        }
        let decoded = group.to_be_bytes();
        if decoded[chunk.len()..].iter().any(|byte| *byte != 0) {
            return None;
        }
        bytes.extend_from_slice(&decoded[1..chunk.len()]);
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_rfc_4648_test_vectors() {
        let vectors = [
            ("", ""),
            ("f", "Zg"),
            ("fo", "Zm8"),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg"),
            ("fooba", "Zm9vYmE"),
            ("foobar", "Zm9vYmFy"),
        ];
        for (plain, encoded) in vectors {
            assert_eq!(encode(&STANDARD, plain.as_bytes()), encoded);
            assert_eq!(decode(&STANDARD, encoded).unwrap(), plain.as_bytes());
        }
        assert_eq!(decode(&STANDARD, "Zm8=").unwrap(), b"fo");
    }

    #[test]
    fn the_alphabets_differ_only_in_their_last_two_symbols() {
        let bytes = [0xfb, 0xff];
        assert_eq!(encode(&STANDARD, &bytes), "+/8");
        assert_eq!(encode(&URL_SAFE, &bytes), "-_8");
        assert_eq!(decode(&URL_SAFE, "-_8").unwrap(), bytes);
        assert_eq!(decode(&URL_SAFE, "+/8"), None);
    }

    #[test]
    fn refuses_what_no_encoding_produces() {
        assert_eq!(decode(&STANDARD, "Z"), None);
        assert_eq!(decode(&STANDARD, "Zm9v!"), None);
        assert_eq!(decode(&STANDARD, "Zh"), None);
        assert_eq!(decode(&STANDARD, "Zm8=="), None);
        assert_eq!(decode(&STANDARD, "Zg="), None);
        assert_eq!(decode(&STANDARD, "Zg==").unwrap(), b"f");
        assert_eq!(decode(&STANDARD, "Zm9v===="), None);
        assert_eq!(decode(&STANDARD, "Zg======"), None);
        assert_eq!(decode(&STANDARD, "Zm9v="), None);
    }

    #[test]
    fn padded_encoding_matches_the_rfc_4648_test_vectors() {
        let vectors = [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ];
        for (plain, encoded) in vectors {
            assert_eq!(encode_padded(&STANDARD, plain.as_bytes()), encoded);
            assert_eq!(decode(&STANDARD, encoded).unwrap(), plain.as_bytes());
        }
        let mut text = String::from("x");
        push_padded(&mut text, &STANDARD, &[0xff, 0xfe]);
        assert_eq!(text, "x//4=");
    }
}
