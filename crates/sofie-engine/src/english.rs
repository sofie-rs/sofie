include!(concat!(env!("OUT_DIR"), "/english.rs"));

/// An engine text in English, or the key itself where `lang/en.toml` lacks it, as a missing
/// translation is written.
#[must_use]
pub fn english(key: &str) -> &str {
    ENGLISH
        .binary_search_by(|(listed, _)| listed.cmp(&key))
        .map_or(key, |index| ENGLISH[index].1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_source_language_file_and_writes_a_missing_key_as_itself() {
        assert_eq!(
            english("notice.correct_fields"),
            "Please correct the {count} highlighted fields."
        );
        assert_eq!(
            english("validation.money.invalid"),
            "must be an amount such as 149.90"
        );
        assert_eq!(english("notice.nothing"), "notice.nothing");
    }
}
