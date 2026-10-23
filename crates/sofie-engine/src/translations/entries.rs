use std::collections::HashMap;

use super::toml::{self, Entry};
use super::{Message, TranslationError};

#[derive(Debug)]
pub(super) struct Entries {
    by_key: HashMap<String, Vec<Option<Definition>>>,
    language_count: usize,
}

#[derive(Debug)]
pub(super) struct Definition {
    pub message: Message,
    module: String,
    is_builtin: bool,
}

/// A framework crate's file loses to the app's own text.
pub(super) struct Origin<'o> {
    pub module: &'o str,
    pub file: &'o str,
    pub served: &'o [usize],
    pub is_builtin: bool,
}

impl Entries {
    pub(super) fn new(language_count: usize) -> Self {
        Self {
            by_key: HashMap::new(),
            language_count,
        }
    }

    pub(super) fn add_file(
        &mut self,
        origin: &Origin<'_>,
        source: &str,
    ) -> Result<(), TranslationError> {
        let entries = toml::parse(source).map_err(|syntax| {
            TranslationError::in_file(origin.file, syntax.line, syntax.column, syntax.message)
        })?;
        for entry in &entries {
            for &index in origin.served {
                self.define(origin, entry, index)?;
            }
        }
        Ok(())
    }

    fn define(
        &mut self,
        origin: &Origin<'_>,
        entry: &Entry,
        index: usize,
    ) -> Result<(), TranslationError> {
        let at = |message: String| {
            TranslationError::in_file(origin.file, entry.line, entry.column, message)
        };
        let message = Message::parse(&entry.text).map_err(at)?;
        let language_count = self.language_count;
        let slots = self
            .by_key
            .entry(entry.key.clone())
            .or_insert_with(|| (0..language_count).map(|_| None).collect());
        let Some(slot) = slots.get_mut(index) else {
            return Ok(());
        };
        if let Some(existing) = slot {
            match (existing.is_builtin, origin.is_builtin) {
                (false, true) => return Ok(()),
                (true, true) if existing.module == origin.module => return Ok(()),
                (true, false) => {}
                _ => {
                    return Err(at(format!(
                        "`{}` is already defined by module `{}`",
                        entry.key, existing.module
                    )));
                }
            }
        }
        *slot = Some(Definition {
            message,
            module: origin.module.to_owned(),
            is_builtin: origin.is_builtin,
        });
        Ok(())
    }

    pub(super) fn get(&self, key: &str, index: usize) -> Option<&Definition> {
        self.by_key.get(key)?.get(index)?.as_ref()
    }

    /// One lookup of the key.
    pub(super) fn get_or_default(&self, key: &str, index: usize) -> Option<&Definition> {
        let definitions = self.by_key.get(key)?;
        definitions
            .get(index)
            .and_then(Option::as_ref)
            .or_else(|| definitions.first()?.as_ref())
    }

    #[cfg(feature = "parse")]
    pub(super) fn contains(&self, key: &str) -> bool {
        self.by_key.contains_key(key)
    }

    pub(super) fn keys(&self) -> impl Iterator<Item = &str> {
        self.by_key.keys().map(String::as_str)
    }
}
