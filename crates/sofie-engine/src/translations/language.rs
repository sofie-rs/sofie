use std::sync::LazyLock;

use sofie_core::{DisplayZone, Shown};
use sofie_values::{Instant, Locale, Money, Present};

use super::conventions::{self, Conventions, NEUTRAL};
use super::{CountedKeys, Message, Part, Translations, Variant, negotiate};
use crate::message::{is_exactly_one, number_of};

static PLAIN: LazyLock<Translations> =
    LazyLock::new(|| Translations::new(&["en"]).expect("the engine's own translations parse"));

#[derive(Clone, Copy, Debug)]
pub struct Language<'a> {
    translations: &'a Translations,
    index: usize,
    conventions: &'static Conventions,
    plural_rule: PluralRule,
    #[cfg(feature = "gates")]
    pub(crate) gates: crate::rendering::GatesRef<'a>,
}

/// What a language's texts and formats are made of, apart from the viewer's gates.
#[cfg(feature = "clients")]
#[derive(Clone, Copy, Debug)]
pub(crate) struct Texts {
    generation: u64,
    index: usize,
    conventions: &'static Conventions,
}

#[cfg(feature = "clients")]
impl Texts {
    pub(crate) fn is_of_same_translations(self, other: Self) -> bool {
        self.generation == other.generation
    }
}

#[cfg(feature = "clients")]
impl PartialEq for Texts {
    fn eq(&self, other: &Self) -> bool {
        self.generation == other.generation
            && self.index == other.index
            && std::ptr::eq(self.conventions, other.conventions)
    }
}

impl Translations {
    /// The principal's choice first, then `Accept-Language` by weight, then the source language.
    #[must_use]
    pub fn negotiate(
        &self,
        preferred: Option<&Locale>,
        accept_language: Option<&str>,
    ) -> Language<'_> {
        let accepted = accept_language.map(negotiate::accepted).unwrap_or_default();
        let index = preferred
            .into_iter()
            .chain(&accepted)
            .find_map(|wanted| closest(&self.languages, wanted))
            .unwrap_or(0);
        let language = self.languages[index].language();
        Language {
            translations: self,
            index,
            conventions: conventions::for_language(language),
            plural_rule: PluralRule::for_language(language),
            #[cfg(feature = "gates")]
            gates: crate::rendering::GatesRef::default(),
        }
    }
}

fn filled(message: &Message, arguments: &[(&str, String)]) -> String {
    let mut text = String::new();
    for part in message.parts() {
        match part {
            Part::Text(literal) => text.push_str(literal),
            Part::Argument(name) => {
                let value = arguments
                    .iter()
                    .find(|(argument, _)| *argument == name.as_str());
                text.push_str(value.map_or("", |(_, value)| value));
            }
        }
    }
    text
}

fn closest(declared: &[Locale], wanted: &Locale) -> Option<usize> {
    declared
        .iter()
        .position(|language| language == wanted)
        .or_else(|| {
            declared
                .iter()
                .position(|language| language.language() == wanted.language())
        })
}

impl<'a> Language<'a> {
    /// So compiling the feature in changes nothing until the app declares languages.
    #[must_use]
    pub fn plain() -> Language<'static> {
        Language {
            translations: &PLAIN,
            index: 0,
            conventions: &NEUTRAL,
            plural_rule: PluralRule::OneIsOne,
            #[cfg(feature = "gates")]
            gates: crate::rendering::GatesRef::default(),
        }
    }

    #[must_use]
    pub fn locale(self) -> &'a Locale {
        &self.translations.languages[self.index]
    }

    #[must_use]
    pub fn field_message<'m>(self, message: &'m str) -> &'m str
    where
        'a: 'm,
    {
        self.text(message, message)
    }

    /// A value refusing in its own words (`validation.invalid`) keeps them, translated if a key.
    #[must_use]
    pub fn field_error(self, key: &str, arguments: &[(&str, String)], fallback: &str) -> String {
        if key == "validation.invalid" {
            return self.field_message(fallback).to_owned();
        }
        let Some(message) = self.message(key) else {
            return fallback.to_owned();
        };
        if !is_source_wording(key, arguments, fallback) {
            return self.field_message(fallback).to_owned();
        }
        filled(message, arguments)
    }

    pub(crate) fn text<'m>(self, key: &str, fallback: &'m str) -> &'m str
    where
        'a: 'm,
    {
        self.message(key)
            .and_then(Message::plain)
            .unwrap_or(fallback)
    }

    pub(crate) fn message(self, key: &str) -> Option<&'a Message> {
        self.translations
            .entries
            .get_or_default(key, self.index)
            .map(|definition| &definition.message)
    }

    /// Another language's `one` would write its singular here, so only this language's counts.
    pub(crate) fn own_message(self, key: &str) -> Option<&'a Message> {
        self.translations
            .entries
            .get(key, self.index)
            .map(|definition| &definition.message)
    }

    /// `key.one` where the count is one, then `key.other`, then the plain key.
    pub(crate) fn counted_message(
        self,
        key: &str,
        keys: &CountedKeys,
        count: Option<Shown<'_>>,
    ) -> Option<&'a Message> {
        let one = || self.own_message(&keys.one);
        self.plural_rule
            .is_one(count)
            .then(one)
            .flatten()
            .or_else(|| self.message(&keys.other))
            .or_else(|| self.message(key))
    }

    #[cfg(feature = "clients")]
    pub(crate) fn plural_rule(self) -> PluralRule {
        self.plural_rule
    }

    pub(crate) fn enumeration(self, name: &str) -> Option<&'a [Variant]> {
        self.translations.enums.get(name).map(Vec::as_slice)
    }

    pub(crate) fn conventions(self) -> &'static Conventions {
        self.conventions
    }

    #[cfg(feature = "clients")]
    pub(crate) fn texts(self) -> Texts {
        Texts {
            generation: self.translations.generation,
            index: self.index,
            conventions: self.conventions,
        }
    }

    pub(crate) fn write_money(self, out: &mut String, amount: Money, scratch: &mut String) {
        self.conventions.write_money(out, amount, scratch);
    }

    #[must_use]
    pub fn format(self, value: &impl Present) -> String {
        let mut out = String::new();
        value.write_localized(&mut out, &self);
        out
    }
}

/// A message other than the key's own source text was chosen by its writer
/// (`plan.unique(…, "is already registered")`), so a translation of the key must not replace it;
/// an empty one asks for the key's text.
fn is_source_wording(key: &str, arguments: &[(&str, String)], fallback: &str) -> bool {
    fallback.is_empty()
        || Language::plain()
            .message(key)
            .is_none_or(|message| filled(message, arguments) == fallback)
}

pub(crate) fn write_instant(
    language: Language<'_>,
    out: &mut String,
    instant: Instant,
    scratch: &mut String,
) {
    language
        .conventions
        .write_instant(out, instant, DisplayZone::current(), scratch);
}

impl sofie_values::Conventions for Language<'_> {
    fn write_number(&self, out: &mut String, plain: &str) {
        self.conventions.write_number(out, plain);
    }

    fn write_money(&self, out: &mut String, amount: Money) {
        self.conventions
            .write_money(out, amount, &mut String::new());
    }

    fn write_date(&self, out: &mut String, iso: &str) {
        self.conventions.write_date(out, iso, &mut String::new());
    }

    fn write_variant(&self, out: &mut String, enumeration: &str, variant: &str) {
        let registered = self
            .enumeration(enumeration)
            .and_then(|variants| variants.iter().find(|known| known.name == variant));
        match registered {
            Some(known) => out.push_str(self.text(&known.key, &known.humanized)),
            None => out.push_str(variant),
        }
    }
}

/// CLDR's plural rules for whole counts, `one` and `other` only: a language's further forms fall to
/// `other`. A language none of these lists names may have a `one` the table cannot say (`sl`'s
/// 101, `fil`'s 2), so it takes `other` and is not told its `one` is never shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PluralRule {
    OneIsOne,
    OneIsZeroOrOne,
    OneEndsInOneNotEleven,
    OtherOnly,
}

const ONE_IS_ONE: [&str; 131] = [
    "af", "an", "ar", "asa", "ast", "az", "bal", "bem", "bez", "bg", "brx", "ca", "ce", "cgg",
    "chr", "ckb", "cs", "cy", "da", "de", "dv", "ee", "el", "en", "eo", "es", "et", "eu", "fi",
    "fo", "fur", "fy", "ga", "gl", "gsw", "ha", "haw", "he", "hu", "ia", "io", "it", "iu", "jgo",
    "ji", "jmc", "ka", "kaj", "kcg", "kk", "kkj", "kl", "ks", "ksb", "ksh", "ku", "ky", "lag",
    "lb", "lg", "lij", "mas", "mgo", "ml", "mn", "mr", "mt", "nah", "naq", "nb", "nd", "ne", "nl",
    "nn", "nnh", "no", "nr", "ny", "nyn", "om", "or", "os", "pap", "pl", "ps", "rm", "ro", "rof",
    "rwk", "saq", "sat", "sc", "scn", "sd", "sdh", "se", "seh", "sk", "sma", "smi", "smj", "smn",
    "sms", "sn", "so", "sq", "ss", "ssy", "st", "sv", "sw", "syr", "ta", "te", "teo", "tig", "tk",
    "tn", "tr", "ts", "ug", "ur", "uz", "ve", "vec", "vo", "vun", "wae", "xh", "xog", "yi",
];

const ONE_IS_ZERO_OR_ONE: [&str; 27] = [
    "ak", "am", "as", "bho", "bn", "csw", "doi", "fa", "ff", "fr", "gu", "guw", "hi", "hy", "kab",
    "kn", "ln", "mg", "nso", "pa", "pcm", "pt", "shi", "si", "ti", "wa", "zu",
];

const ONE_ENDS_IN_ONE_NOT_ELEVEN: [&str; 11] = [
    "be", "bs", "hr", "is", "lt", "lv", "mk", "ru", "sh", "sr", "uk",
];

#[cfg(feature = "parse")]
const WITHOUT_ONE: [&str; 33] = [
    "bm", "bo", "dz", "hnj", "id", "ig", "ii", "in", "ja", "jbo", "jv", "jw", "kde", "kea", "km",
    "ko", "lkt", "lo", "ms", "my", "root", "sah", "ses", "sg", "su", "th", "to", "tpi", "vi", "wo",
    "yo", "yue", "zh",
];

impl PluralRule {
    pub(crate) fn for_language(language: &str) -> Self {
        if ONE_IS_ONE.contains(&language) {
            Self::OneIsOne
        // CLDR's `pt` is Brazilian, where 0 is singular as in French.
        } else if ONE_IS_ZERO_OR_ONE.contains(&language) {
            Self::OneIsZeroOrOne
        } else if ONE_ENDS_IN_ONE_NOT_ELEVEN.contains(&language) {
            Self::OneEndsInOneNotEleven
        } else {
            Self::OtherOnly
        }
    }

    #[cfg(feature = "parse")]
    pub(crate) fn has_no_one(language: &str) -> bool {
        WITHOUT_ONE.contains(&language)
    }

    /// Anything but a number is `other`.
    #[expect(
        clippy::float_cmp,
        reason = "a whole count's remainders are whole, so exact"
    )]
    pub(crate) fn is_one(self, count: Option<Shown<'_>>) -> bool {
        let absolute = number_of(count).map(f64::abs);
        match self {
            Self::OneIsOne => is_exactly_one(count),
            Self::OneIsZeroOrOne => absolute.is_some_and(|count| count < 2.0),
            Self::OneEndsInOneNotEleven => absolute.is_some_and(|count| {
                count.fract() == 0.0 && count % 10.0 == 1.0 && count % 100.0 != 11.0
            }),
            Self::OtherOnly => false,
        }
    }

    #[cfg(feature = "clients")]
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::OneIsOne => "one-is-one",
            Self::OneIsZeroOrOne => "one-is-zero-or-one",
            Self::OneEndsInOneNotEleven => "one-ends-in-one-not-eleven",
            Self::OtherOnly => "other-only",
        }
    }
}
