/// FNV-1a 64: the same in every process and release, which stored migration checksums
/// and flag rollout buckets rely on; not collision-resistant.
#[derive(Clone, Copy)]
pub struct Fnv1a64(u64);

impl Fnv1a64 {
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    #[must_use]
    pub const fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    #[must_use]
    pub const fn write(self, bytes: &[u8]) -> Self {
        let mut hash = self.0;
        let mut index = 0;
        while index < bytes.len() {
            hash ^= bytes[index] as u64;
            hash = hash.wrapping_mul(Self::PRIME);
            index += 1;
        }
        Self(hash)
    }

    #[must_use]
    pub const fn finish(self) -> u64 {
        self.0
    }
}

impl Default for Fnv1a64 {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::Fnv1a64;

    fn hash(bytes: &[u8]) -> u64 {
        Fnv1a64::new().write(bytes).finish()
    }

    #[test]
    fn hashes_like_the_reference_fnv1a() {
        assert_eq!(hash(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(hash(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(hash(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn writing_in_pieces_hashes_like_writing_at_once() {
        let pieces = Fnv1a64::new().write(b"foo").write(b"").write(b"bar");
        assert_eq!(pieces.finish(), hash(b"foobar"));
    }
}
