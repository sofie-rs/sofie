//! The protobuf wire format, for the OTLP messages the exporter sends and no others.
//!
//! Callers skip a proto3 field that holds its default value, as the reference encoders do.

#[derive(Clone, Copy)]
enum WireType {
    Varint = 0,
    Fixed64 = 1,
    Delimited = 2,
    Fixed32 = 5,
}

#[derive(Default)]
pub(crate) struct Encoder {
    bytes: Vec<u8>,
}

impl Encoder {
    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    pub(crate) fn varint(&mut self, field: u32, value: u64) {
        self.key(field, WireType::Varint);
        self.raw_varint(value);
    }

    /// Negative numbers take ten bytes, as protobuf's `int64` does.
    pub(crate) fn int64(&mut self, field: u32, value: i64) {
        self.varint(field, value.cast_unsigned());
    }

    pub(crate) fn bool(&mut self, field: u32, value: bool) {
        self.varint(field, u64::from(value));
    }

    pub(crate) fn fixed32(&mut self, field: u32, value: u32) {
        self.key(field, WireType::Fixed32);
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn fixed64(&mut self, field: u32, value: u64) {
        self.key(field, WireType::Fixed64);
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn double(&mut self, field: u32, value: f64) {
        self.fixed64(field, value.to_bits());
    }

    pub(crate) fn bytes(&mut self, field: u32, value: &[u8]) {
        self.key(field, WireType::Delimited);
        self.raw_varint(value.len() as u64);
        self.bytes.extend_from_slice(value);
    }

    pub(crate) fn string(&mut self, field: u32, value: &str) {
        self.bytes(field, value.as_bytes());
    }

    #[cfg(feature = "metrics")]
    pub(crate) fn packed_fixed64(&mut self, field: u32, values: &[u64]) {
        self.key(field, WireType::Delimited);
        self.raw_varint(values.len() as u64 * 8);
        for value in values {
            self.bytes.extend_from_slice(&value.to_le_bytes());
        }
    }

    #[cfg(feature = "metrics")]
    pub(crate) fn packed_double(&mut self, field: u32, values: &[f64]) {
        self.key(field, WireType::Delimited);
        self.raw_varint(values.len() as u64 * 8);
        for value in values {
            self.bytes.extend_from_slice(&value.to_le_bytes());
        }
    }

    /// The length goes before the body, so it is written in place once the body is: one byte
    /// reserved, the rest shifted in when the body passes 127 bytes.
    pub(crate) fn message(&mut self, field: u32, body: impl FnOnce(&mut Self)) {
        self.key(field, WireType::Delimited);
        let start = self.bytes.len();
        self.bytes.push(0);
        body(self);
        let length = (self.bytes.len() - start - 1) as u64;
        let mut prefix = Self::default();
        prefix.raw_varint(length);
        self.bytes.splice(start..=start, prefix.bytes);
    }

    fn key(&mut self, field: u32, wire_type: WireType) {
        self.raw_varint(u64::from((field << 3) | wire_type as u32));
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "each byte cast holds seven bits of the value"
    )]
    fn raw_varint(&mut self, mut value: u64) {
        while value >= 0x80 {
            self.bytes.push((value & 0x7f) as u8 | 0x80);
            value >>= 7;
        }
        self.bytes.push(value as u8);
    }
}

#[cfg(test)]
mod tests {
    use super::Encoder;

    fn encoded(write: impl FnOnce(&mut Encoder)) -> Vec<u8> {
        let mut encoder = Encoder::default();
        write(&mut encoder);
        encoder.into_bytes()
    }

    #[test]
    fn scalars_follow_the_wire_format() {
        assert_eq!(
            encoded(|encoder| encoder.varint(1, 150)),
            [0x08, 0x96, 0x01]
        );
        assert_eq!(
            encoded(|encoder| encoder.int64(3, -1)),
            [
                0x18, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01
            ]
        );
        assert_eq!(
            encoded(|encoder| encoder.fixed32(16, 0x0301)),
            [0x85, 0x01, 0x01, 0x03, 0x00, 0x00]
        );
        assert_eq!(
            encoded(|encoder| encoder.double(4, 1.5)),
            [0x21, 0, 0, 0, 0, 0, 0, 0xf8, 0x3f]
        );
        assert_eq!(
            encoded(|encoder| encoder.string(1, "hi")),
            [0x0a, 2, b'h', b'i']
        );
    }

    #[test]
    fn a_long_nested_message_gets_a_two_byte_length() {
        let bytes = encoded(|encoder| {
            encoder.message(2, |inner| inner.bytes(1, &[7; 200]));
        });

        // 203 bytes of body: the inner key, its two-byte length 200, then 200 bytes.
        assert_eq!(bytes[..6], [0x12, 0xcb, 0x01, 0x0a, 0xc8, 0x01]);
        assert_eq!(bytes.len(), 3 + 203);
    }
}
