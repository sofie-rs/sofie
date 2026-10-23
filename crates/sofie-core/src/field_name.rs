#[must_use]
pub fn is_field_name(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_name_is_ascii_letters_digits_and_underscores() {
        for accepted in ["name", "line_2", "_", "A1"] {
            assert!(is_field_name(accepted), "{accepted}");
        }
        for refused in ["", "a-b", "a.b", "a b", "a[0]", "ä"] {
            assert!(!is_field_name(refused), "{refused}");
        }
    }
}
