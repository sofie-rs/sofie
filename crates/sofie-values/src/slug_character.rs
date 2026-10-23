use std::ops::RangeInclusive;

use unicode_normalization::char::is_combining_mark;

/// Decomposed accents, which a slug drops as it spells Latin letters in ASCII.
pub const LATIN_COMBINING_ACCENTS: RangeInclusive<char> = '\u{300}'..='\u{36f}';

/// A letter, digit or nonspacing or spacing mark (L*, N*, Mn, Mc) that lowercasing leaves as it
/// is; anything else parts a slug's words, unassigned, private-use and format characters too.
#[must_use]
pub fn is_slug_character(character: char) -> bool {
    character.to_lowercase().eq([character])
        && (character.is_alphanumeric() || is_word_mark(character))
        && !LATIN_COMBINING_ACCENTS.contains(&character)
        && !sofie_core::misleads_display(character)
}

/// The enclosing marks (Me) draw a circle or a sign around what comes before them.
fn is_word_mark(character: char) -> bool {
    is_combining_mark(character)
        && !matches!(
            character,
            '\u{488}'..='\u{489}'
                | '\u{1abe}'
                | '\u{20dd}'..='\u{20e0}'
                | '\u{20e2}'..='\u{20e4}'
                | '\u{a670}'..='\u{a672}'
        )
}
