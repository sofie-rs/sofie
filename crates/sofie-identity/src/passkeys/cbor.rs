//! The subset of CBOR (RFC 8949) `WebAuthn` uses: definite lengths, integers, byte and text
//! strings, arrays, maps and simple values, read with a nesting bound.

const MAXIMUM_DEPTH: usize = 16;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Item<'a> {
    Integer(i128),
    Bytes(&'a [u8]),
    Text(&'a str),
    Array(Vec<Item<'a>>),
    Map(Vec<(Item<'a>, Item<'a>)>),
    Simple(u8),
}

impl<'a> Item<'a> {
    fn get(&self, key: &Item<'_>) -> Option<&Item<'a>> {
        let Item::Map(entries) = self else {
            return None;
        };
        entries
            .iter()
            .find(|(found, _)| found == key)
            .map(|(_, value)| value)
    }

    pub(super) fn by_text(&self, key: &str) -> Option<&Item<'a>> {
        self.get(&Item::Text(key))
    }

    pub(super) fn by_number(&self, key: i128) -> Option<&Item<'a>> {
        self.get(&Item::Integer(key))
    }

    pub(super) fn bytes(&self) -> Option<&'a [u8]> {
        match self {
            Item::Bytes(bytes) => Some(bytes),
            _ => None,
        }
    }

    pub(super) fn integer(&self) -> Option<i128> {
        match self {
            Item::Integer(value) => Some(*value),
            _ => None,
        }
    }
}

pub(super) fn read(input: &[u8]) -> Option<(Item<'_>, &[u8])> {
    let mut reader = Reader { input, depth: 0 };
    let item = reader.next()?;
    Some((item, reader.input))
}

struct Reader<'a> {
    input: &'a [u8],
    depth: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, length: usize) -> Option<&'a [u8]> {
        let (taken, rest) = self.input.split_at_checked(length)?;
        self.input = rest;
        Some(taken)
    }

    fn argument(&mut self, additional: u8) -> Option<u64> {
        let width = match additional {
            0..=23 => return Some(u64::from(additional)),
            24 => 1,
            25 => 2,
            26 => 4,
            27 => 8,
            _ => return None,
        };
        let bytes = self.take(width)?;
        Some(
            bytes
                .iter()
                .fold(0, |value, byte| (value << 8) | u64::from(*byte)),
        )
    }

    fn length(&mut self, additional: u8) -> Option<usize> {
        usize::try_from(self.argument(additional)?).ok()
    }

    fn next(&mut self) -> Option<Item<'a>> {
        let initial = *self.take(1)?.first()?;
        let (major, additional) = (initial >> 5, initial & 0x1f);
        match major {
            0 => Some(Item::Integer(i128::from(self.argument(additional)?))),
            1 => Some(Item::Integer(-1 - i128::from(self.argument(additional)?))),
            2 => {
                let length = self.length(additional)?;
                self.take(length).map(Item::Bytes)
            }
            3 => {
                let length = self.length(additional)?;
                std::str::from_utf8(self.take(length)?).ok().map(Item::Text)
            }
            4 | 5 => self.nested(major, additional),
            7 if additional < 24 => Some(Item::Simple(additional)),
            _ => None,
        }
    }

    fn nested(&mut self, major: u8, additional: u8) -> Option<Item<'a>> {
        self.depth += 1;
        if self.depth > MAXIMUM_DEPTH {
            return None;
        }
        let count = self.length(additional)?;
        // Each entry takes a byte at least, so a count beyond the input is a lie.
        if count > self.input.len() {
            return None;
        }
        let item = if major == 4 {
            let mut items = Vec::with_capacity(count);
            for _ in 0..count {
                items.push(self.next()?);
            }
            Item::Array(items)
        } else {
            let mut entries = Vec::with_capacity(count);
            for _ in 0..count {
                entries.push((self.next()?, self.next()?));
            }
            Item::Map(entries)
        };
        self.depth -= 1;
        Some(item)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_map_of_integers_text_and_bytes() {
        // {1: 2, 3: -7, "fmt": "none", -2: h'0102'}
        let input = [
            0xa4, 0x01, 0x02, 0x03, 0x26, 0x63, b'f', b'm', b't', 0x64, b'n', b'o', b'n', b'e',
            0x21, 0x42, 0x01, 0x02,
        ];
        let (item, rest) = read(&input).unwrap();

        assert_eq!(rest, &[] as &[u8]);
        assert_eq!(item.by_number(1).and_then(Item::integer), Some(2));
        assert_eq!(item.by_number(3).and_then(Item::integer), Some(-7));
        assert_eq!(item.by_text("fmt"), Some(&Item::Text("none")));
        assert_eq!(item.by_number(-2).and_then(Item::bytes), Some(&[1, 2][..]));
    }

    #[test]
    fn refuses_truncated_indefinite_and_overly_deep_input() {
        assert!(read(&[0x42, 0x01]).is_none());
        assert!(read(&[0x5f]).is_none());
        assert!(read(&[0x9b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]).is_none());
        assert!(read(&[0x81; 40]).is_none());
    }
}
