use std::marker::PhantomData;

use sofie_core::Uuid;

pub struct Key<T> {
    module: &'static str,
    name: &'static str,
    value: PhantomData<fn() -> T>,
}

impl<T> Key<T> {
    /// Through [`key!`], which names the calling crate; a name holding `:` or `%` could pass for
    /// a name and its parts, so it fails the build.
    #[doc(hidden)]
    #[must_use]
    pub const fn new(module: &'static str, name: &'static str) -> Self {
        assert!(
            is_plain(module),
            "a cache key's module holds `:`, `%` or whitespace"
        );
        assert!(
            is_plain(name),
            "a cache key's name holds `:`, `%` or whitespace; add the rest with `.part(…)`"
        );
        Self {
            module,
            name,
            value: PhantomData,
        }
    }

    pub(crate) fn text(&self) -> String {
        let mut text = String::with_capacity(self.module.len() + 1 + self.name.len() + 40);
        text.push_str(self.module);
        text.push(':');
        text.push_str(self.name);
        text
    }
}

const fn is_plain(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let mut index = 0;
    while index < bytes.len() {
        if matches!(bytes[index], b':' | b'%') || bytes[index].is_ascii_whitespace() {
            return false;
        }
        index += 1;
    }
    true
}

/// The calling crate is the module, so a module cannot write under another's keys.
#[macro_export]
macro_rules! key {
    ($name:literal) => {
        const { $crate::Key::new(env!("CARGO_CRATE_NAME"), $name) }
    };
}

pub(crate) fn aggregate_tag(module: &str, aggregate: &str) -> String {
    format!("{module}.{aggregate}")
}

pub(crate) fn record_tag(module: &str, aggregate: &str, id: Uuid) -> String {
    format!("{module}.{aggregate}/{id}")
}

/// Held by every entry on one record, dropped by a commit that cannot name the records it changed.
pub(crate) fn any_record_tag(module: &str, aggregate: &str) -> String {
    format!("{module}.{aggregate}/*")
}

#[cfg(feature = "memory")]
pub(crate) fn widened_tag(tag: &str) -> String {
    match tag.rsplit_once('/') {
        Some((aggregate, _)) => format!("{aggregate}/*"),
        None => tag.to_owned(),
    }
}

/// A part holding `:` cannot pass for two parts.
pub(crate) fn push_escaped_part(key: &mut String, part: &str) {
    for character in part.chars() {
        match character {
            '%' => key.push_str("%25"),
            ':' => key.push_str("%3A"),
            _ => key.push(character),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_namespaced_by_the_crate_declaring_it() {
        let key: Key<u32> = crate::key!("totals");

        assert_eq!(key.text(), "sofie_cache:totals");
    }

    #[test]
    fn parts_cannot_forge_a_separator() {
        let mut key = String::new();
        push_escaped_part(&mut key, "a:b%");
        assert_eq!(key, "a%3Ab%25");
    }

    #[test]
    fn a_name_that_could_pass_for_parts_is_refused() {
        assert!(is_plain("totals"));
        for name in ["totals:2024", "50%", "two words", ""] {
            assert!(!is_plain(name), "{name:?}");
        }
    }
}
