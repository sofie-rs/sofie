mod call;
#[cfg(feature = "parse")]
mod check;
mod conventions;
pub use conventions::CURRENCY_SYMBOLS;
mod entries;
mod language;
#[cfg(feature = "parse")]
mod localized;
mod negotiate;
#[cfg(feature = "parse")]
mod record;
mod toml;
#[cfg(feature = "parse")]
mod uses;

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use sofie_values::Locale;

pub(crate) use self::call::{Argument, Call, CountedKeys, Format, label};
#[cfg(feature = "parse")]
pub use self::check::{Gap, GapKind};
use self::entries::{Entries, Origin};
pub use self::language::Language;
#[cfg(feature = "clients")]
pub(crate) use self::language::Texts;
pub(crate) use self::language::write_instant;
#[cfg(feature = "parse")]
pub use self::localized::{check_translations, translation_keys};
#[cfg(feature = "parse")]
pub(crate) use self::record::record;
#[cfg(feature = "parse")]
pub(crate) use self::uses::key_uses;
#[cfg(feature = "parse")]
pub use self::uses::{KeyKind, KeyUse};
use crate::label::humanize;
pub(crate) use crate::message::{Message, Part};

const ENGINE_MODULE: &str = "sofie_engine";

const ENGINE_TRANSLATIONS: &[(&str, &str)] = &[
    ("en", include_str!("../../lang/en.toml")),
    ("de", include_str!("../../lang/de.toml")),
    ("tr", include_str!("../../lang/tr.toml")),
];

#[derive(Debug)]
pub struct Translations {
    /// Changes with every addition, so what was derived from earlier texts is not reused.
    generation: u64,
    languages: Vec<Locale>,
    entries: Entries,
    /// Kept here because their variants' labels are translation keys.
    enums: HashMap<String, Vec<Variant>>,
    /// An app's enum of the same name replaces these, as its texts replace a framework crate's.
    builtin_enums: HashSet<String>,
}

#[derive(Debug)]
pub(crate) struct Variant {
    pub name: String,
    pub tone: Tone,
    pub key: String,
    pub humanized: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Info,
    Success,
    Warning,
    Danger,
}

impl Tone {
    pub(crate) fn class(self) -> &'static str {
        match self {
            Self::Neutral => "sofie-badge sofie-tone-neutral",
            Self::Info => "sofie-badge sofie-tone-info",
            Self::Success => "sofie-badge sofie-tone-success",
            Self::Warning => "sofie-badge sofie-tone-warning",
            Self::Danger => "sofie-badge sofie-tone-danger",
        }
    }
}

fn next_generation() -> u64 {
    static GENERATIONS: AtomicU64 = AtomicU64::new(0);
    GENERATIONS.fetch_add(1, Ordering::Relaxed)
}

impl Translations {
    pub fn new(languages: &[&str]) -> Result<Self, TranslationError> {
        let mut locales: Vec<Locale> = Vec::new();
        for tag in languages {
            let locale = Locale::parse(tag).map_err(|error| {
                TranslationError::Declaration(format!("`{tag}` is not a language tag: {error:#}"))
            })?;
            if locales.contains(&locale) {
                return Err(TranslationError::Declaration(format!(
                    "language `{tag}` is declared twice"
                )));
            }
            locales.push(locale);
        }
        if locales.is_empty() {
            return Err(TranslationError::Declaration(
                "an app declares at least one language, the one its names are written in first"
                    .to_owned(),
            ));
        }
        let mut translations = Self {
            generation: next_generation(),
            entries: Entries::new(locales.len()),
            languages: locales,
            enums: HashMap::new(),
            builtin_enums: HashSet::new(),
        };
        translations.add_builtin(ENGINE_MODULE, ENGINE_TRANSLATIONS)?;
        translations.add_weekdays()?;
        Ok(translations)
    }

    /// Its texts replace a framework crate's; two app modules may not define one key.
    pub fn add(
        &mut self,
        module: &str,
        language: &str,
        source: &str,
    ) -> Result<(), TranslationError> {
        self.generation = next_generation();
        let file = file_name(module, language);
        let locale = parse_language(&file, language)?;
        let index = self
            .languages
            .iter()
            .position(|declared| *declared == locale)
            .ok_or_else(|| {
                TranslationError::in_file(
                    &file,
                    1,
                    1,
                    format!(
                        "the app does not declare language `{language}`; it declares {}",
                        language_list(&self.languages)
                    ),
                )
            })?;
        let origin = Origin {
            module,
            file: &file,
            served: &[index],
            is_builtin: false,
        };
        self.entries.add_file(&origin, source)
    }

    /// A language the app does not declare is not offered, and the app's own texts win.
    pub fn add_builtin(
        &mut self,
        module: &str,
        files: &[(&str, &str)],
    ) -> Result<(), TranslationError> {
        self.generation = next_generation();
        for (language, source) in files {
            let file = file_name(module, language);
            let locale = parse_language(&file, language)?;
            let served: Vec<usize> = (0..self.languages.len())
                .filter(|&index| {
                    let declared = &self.languages[index];
                    *declared == locale || declared.language() == locale.as_str()
                })
                .collect();
            let origin = Origin {
                module,
                file: &file,
                served: &served,
                is_builtin: true,
            };
            self.entries.add_file(&origin, source)?;
        }
        Ok(())
    }

    pub fn add_enum(
        &mut self,
        name: &str,
        variants: &[(&str, Tone)],
    ) -> Result<(), TranslationError> {
        self.generation = next_generation();
        let refuse = |message: String| Err(TranslationError::Declaration(message));
        if !is_name(name) {
            return refuse(format!(
                "enum name `{name}` is not a lowercase name such as `order_status`"
            ));
        }
        if variants.is_empty()
            || (self.enums.contains_key(name) && !self.builtin_enums.remove(name))
        {
            return refuse(format!(
                "enum `{name}` is registered twice or without variants"
            ));
        }
        let mut registered = Vec::new();
        for (variant, tone) in variants {
            let lowercase = variant.to_ascii_lowercase();
            if !is_name(&lowercase) {
                return refuse(format!(
                    "`{variant}` of enum `{name}` is not a variant name such as `PAID`"
                ));
            }
            registered.push(Variant {
                name: (*variant).to_owned(),
                tone: *tone,
                key: format!("enum.{name}.{lowercase}"),
                humanized: humanize(&lowercase),
            });
        }
        self.enums.insert(name.to_owned(), registered);
        Ok(())
    }

    /// `sofie_values::Weekday` is labelled like an app's enum, by `enum.weekday.<day>`.
    fn add_weekdays(&mut self) -> Result<(), TranslationError> {
        let days: Vec<(&str, Tone)> = <sofie_values::Weekday as sofie_core::Variants>::VARIANTS
            .iter()
            .map(|(name, _)| (*name, Tone::Neutral))
            .collect();
        self.add_enum("weekday", &days)?;
        self.builtin_enums.insert("weekday".to_owned());
        Ok(())
    }

    #[must_use]
    pub fn languages(&self) -> &[Locale] {
        &self.languages
    }

    #[must_use]
    pub fn keys(&self) -> Vec<&str> {
        let mut keys: Vec<&str> = self.entries.keys().collect();
        keys.sort_unstable();
        keys
    }
}

fn language_list(languages: &[Locale]) -> String {
    let tags: Vec<&str> = languages.iter().map(Locale::as_str).collect();
    tags.join(", ")
}

fn file_name(module: &str, language: &str) -> String {
    format!("{module}/lang/{language}.toml")
}

fn parse_language(file: &str, language: &str) -> Result<Locale, TranslationError> {
    Locale::parse(language).map_err(|error| {
        TranslationError::in_file(
            file,
            1,
            1,
            format!("`{language}` is not a language tag: {error:#}"),
        )
    })
}

fn is_name(text: &str) -> bool {
    !text.is_empty() && text.chars().all(toml::is_key_character)
}

#[derive(Debug, PartialEq, Eq)]
pub enum TranslationError {
    File {
        file: String,
        line: usize,
        column: usize,
        message: String,
    },
    Declaration(String),
}

impl TranslationError {
    pub(super) fn in_file(file: &str, line: usize, column: usize, message: String) -> Self {
        Self::File {
            file: file.to_owned(),
            line,
            column,
            message,
        }
    }
}

impl fmt::Display for TranslationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File {
                file,
                line,
                column,
                message,
            } => write!(f, "{file}:{line}:{column}: {message}"),
            Self::Declaration(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for TranslationError {}
