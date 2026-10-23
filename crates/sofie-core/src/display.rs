/// They reorder or hide text where it is shown (CVE-2021-42574). Unicode 16.0's format (Cf) list,
/// written out to need no dependency, keeps ZWJ and ZWNJ, which Persian and Devanagari spell with.
#[must_use]
pub fn misleads_display(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{00AD}'
                | '\u{0600}'..='\u{0605}'
                | '\u{061C}'
                | '\u{06DD}'
                | '\u{070F}'
                | '\u{0890}'..='\u{0891}'
                | '\u{08E2}'
                | '\u{115F}'..='\u{1160}'
                | '\u{180E}'
                | '\u{200B}'
                | '\u{200E}'..='\u{200F}'
                | '\u{2028}'..='\u{202E}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{206F}'
                | '\u{3164}'
                | '\u{FEFF}'
                | '\u{FFA0}'
                | '\u{FFF9}'..='\u{FFFB}'
                | '\u{110BD}'
                | '\u{110CD}'
                | '\u{13430}'..='\u{1343F}'
                | '\u{1BCA0}'..='\u{1BCA3}'
                | '\u{1D173}'..='\u{1D17A}'
                | '\u{E0001}'
                | '\u{E0020}'..='\u{E007F}'
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_reordering_and_hiding_but_keeps_joiners() {
        for refused in [
            '\u{0}', '\n', '\u{1B}', '\u{202E}', '\u{2067}', '\u{200B}', '\u{FEFF}', '\u{200F}',
            '\u{061C}', '\u{2028}',
        ] {
            assert!(misleads_display(refused), "{refused:?}");
        }
        for kept in ['a', 'é', 'ş', '\u{200C}', '\u{200D}', ' '] {
            assert!(!misleads_display(kept), "{kept:?}");
        }
    }

    #[test]
    fn refuses_every_format_character_and_the_hangul_fillers() {
        for invisible in [
            '\u{00AD}',
            '\u{3164}',
            '\u{E0067}',
            '\u{2062}',
            '\u{180E}',
            '\u{206A}',
            '\u{FFFB}',
            '\u{0601}',
            '\u{1BCA0}',
            '\u{1D173}',
        ] {
            assert!(misleads_display(invisible), "{invisible:?}");
        }
    }
}
