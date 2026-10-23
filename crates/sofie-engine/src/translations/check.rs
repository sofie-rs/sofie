use std::collections::BTreeMap;
use std::fmt;

use super::language::PluralRule;
use super::{KeyKind, KeyUse, Translations};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gap {
    pub key: String,
    pub language: String,
    pub kind: GapKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GapKind {
    Missing,
    Arguments {
        expected: Vec<String>,
        found: Vec<String>,
    },
    NeverPicked,
}

impl fmt::Display for Gap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            key,
            language,
            kind,
        } = self;
        match kind {
            GapKind::Missing => write!(f, "`{key}` has no `{language}` translation"),
            GapKind::Arguments { expected, found } => write!(
                f,
                "`{key}` in `{language}` has placeholders {found:?} where the others have {expected:?}"
            ),
            GapKind::NeverPicked => write!(
                f,
                "`{key}` in `{language}` is never shown: a count there always picks `other`"
            ),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Required {
    Everywhere,
    /// The source language may humanize the name instead.
    BesidesSource,
}

impl Required {
    fn of(key: &str) -> Self {
        if is_humanized(key) {
            Self::BesidesSource
        } else {
            Self::Everywhere
        }
    }
}

impl Translations {
    /// Checked in every declared language; sorted by key.
    #[must_use]
    pub fn gaps(&self, uses: &[KeyUse]) -> Vec<Gap> {
        let mut required: BTreeMap<&str, Required> = BTreeMap::new();
        for key in self.entries.keys().filter(|key| !self.is_one_form(key)) {
            required.insert(key, Required::of(key));
        }
        for variant in self.enums.values().flatten() {
            required
                .entry(&variant.key)
                .or_insert(Required::BesidesSource);
        }
        for key_use in uses {
            match key_use.kind {
                KeyKind::Message { .. } if self.has_forms(&key_use.key) => {}
                KeyKind::Message { .. } => {
                    required.insert(&key_use.key, Required::of(&key_use.key));
                }
                KeyKind::Label => {
                    required
                        .entry(&key_use.key)
                        .or_insert(Required::BesidesSource);
                }
                KeyKind::Enum => {}
            }
        }
        let mut gaps = Vec::new();
        for (key, requirement) in required {
            gaps_of(self, key, requirement, &mut gaps);
        }
        gaps.extend(self.one_form_gaps());
        gaps.sort_by(|left, right| left.key.cmp(&right.key));
        gaps
    }

    /// A `one` form is wanted where the language's rule picks it, and only there.
    fn one_form_gaps(&self) -> Vec<Gap> {
        let mut gaps = Vec::new();
        for key in self.entries.keys().filter(|key| self.is_one_form(key)) {
            for (index, language) in self.languages.iter().enumerate() {
                let tag = language.language();
                let picks_one = PluralRule::for_language(tag) != PluralRule::OtherOnly;
                let kind = match (picks_one, self.entries.get(key, index).is_some()) {
                    (true, false) => GapKind::Missing,
                    (false, true) if PluralRule::has_no_one(tag) => GapKind::NeverPicked,
                    _ => continue,
                };
                gaps.push(Gap {
                    key: key.to_owned(),
                    language: language.as_str().to_owned(),
                    kind,
                });
            }
        }
        gaps
    }

    fn has_forms(&self, key: &str) -> bool {
        self.entries.contains(&format!("{key}.other"))
    }

    /// A language without a `one` form leaves it out, and it may omit the count.
    fn is_one_form(&self, key: &str) -> bool {
        key.strip_suffix(".one")
            .is_some_and(|stem| self.has_forms(stem))
    }
}

fn gaps_of(translations: &Translations, key: &str, requirement: Required, gaps: &mut Vec<Gap>) {
    let defined: Vec<_> = (0..translations.languages.len())
        .map(|index| translations.entries.get(key, index))
        .collect();
    let expected = defined
        .iter()
        .flatten()
        .next()
        .map(|first| first.message.arguments());
    for (index, definition) in defined.iter().enumerate() {
        let kind = match (definition, &expected) {
            (None, _) if index == 0 && requirement == Required::BesidesSource => continue,
            (None, _) => GapKind::Missing,
            (Some(definition), Some(expected)) => {
                let found = definition.message.arguments();
                if found == *expected {
                    continue;
                }
                GapKind::Arguments {
                    expected: owned(expected),
                    found: owned(&found),
                }
            }
            (Some(_), None) => continue,
        };
        gaps.push(Gap {
            key: key.to_owned(),
            language: translations.languages[index].as_str().to_owned(),
            kind,
        });
    }
}

pub(super) fn check_use(translations: &Translations, key_use: &KeyUse) -> Result<(), String> {
    let key = key_use.key.as_str();
    let arguments = match &key_use.kind {
        KeyKind::Label => return Ok(()),
        KeyKind::Enum if translations.enums.contains_key(key) => return Ok(()),
        KeyKind::Enum => return Err(format!("`{key}` is not a registered enum")),
        KeyKind::Message { arguments } => arguments,
    };
    let one = format!("{key}.one");
    let other = format!("{key}.other");
    let is_defined = |key: &str| translations.entries.get(key, 0).is_some();
    if !is_defined(key) && !is_defined(&other) && !is_humanized(key) {
        return Err(format!(
            "translation key `{key}` has no text in `{}`, the source language",
            translations.languages[0].as_str()
        ));
    }
    if !is_defined(key) && is_defined(&other) && !arguments.iter().any(|name| name == "count") {
        return Err(format!(
            "translation key `{key}` has only plural forms, so the call needs a `count`"
        ));
    }
    for index in 0..translations.languages.len() {
        check_placeholders(translations, key, index, arguments, |found| {
            found == arguments
        })?;
        for form in [&one, &other] {
            check_placeholders(translations, form, index, arguments, |found| {
                found
                    .iter()
                    .all(|name| arguments.iter().any(|given| given == name))
            })?;
        }
    }
    Ok(())
}

fn check_placeholders(
    translations: &Translations,
    key: &str,
    index: usize,
    arguments: &[String],
    fits: impl Fn(&[&str]) -> bool,
) -> Result<(), String> {
    let Some(definition) = translations.entries.get(key, index) else {
        return Ok(());
    };
    let placeholders = definition.message.arguments();
    if fits(&placeholders) {
        return Ok(());
    }
    Err(format!(
        "`{key}` in `{}` has placeholders {placeholders:?}, but this call gives {arguments:?}",
        translations.languages[index].as_str()
    ))
}

fn owned(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

/// The source language may leave these out: the humanized name is the text there.
const HUMANIZED_PREFIXES: [&str; 2] = ["label.", "enum."];

fn is_humanized(key: &str) -> bool {
    HUMANIZED_PREFIXES
        .iter()
        .any(|prefix| key.starts_with(prefix))
}
