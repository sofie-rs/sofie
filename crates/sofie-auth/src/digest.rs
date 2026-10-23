use ring::digest::{Context, SHA256};

use sofie_core::encoding::base64::{self, URL_SAFE};

/// Each part is length-prefixed, so moving bytes between parts changes the digest.
#[must_use]
pub fn digest(parts: &[&[u8]]) -> String {
    let mut context = Context::new(&SHA256);
    for part in parts {
        context.update(&(part.len() as u64).to_be_bytes());
        context.update(part);
    }
    base64::encode(&URL_SAFE, context.finish().as_ref())
}

#[cfg(test)]
mod tests {
    use super::digest;

    #[test]
    fn the_digest_tells_parts_apart_and_is_stable() {
        assert_eq!(digest(&[b"ab", b"c"]), digest(&[b"ab", b"c"]));
        assert_ne!(digest(&[b"ab", b"c"]), digest(&[b"a", b"bc"]));
        assert_eq!(digest(&[b""]).len(), 43);
    }
}
