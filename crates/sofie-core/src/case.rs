/// How a type's name becomes a table, column or error field: `HTTPStatus` as `http_status`.
/// sofie-macros has no dependencies, so it keeps a copy its tests hold to this one.
#[must_use]
pub fn snake_case(name: &str) -> String {
    let characters: Vec<char> = name.chars().collect();
    let mut snake = String::with_capacity(name.len() + 4);
    for (index, &character) in characters.iter().enumerate() {
        if index > 0 && starts_word(&characters, index) {
            snake.push('_');
        }
        snake.extend(character.to_lowercase());
    }
    snake
}

fn starts_word(characters: &[char], index: usize) -> bool {
    let current = characters[index];
    if !current.is_uppercase() {
        return false;
    }
    let previous = characters[index - 1];
    let next_is_lowercase = characters
        .get(index + 1)
        .is_some_and(|next| next.is_lowercase());
    previous.is_lowercase()
        || previous.is_ascii_digit()
        || (previous.is_uppercase() && next_is_lowercase)
}

#[cfg(test)]
mod tests {
    use super::snake_case;

    #[test]
    fn an_acronym_is_one_word() {
        assert_eq!(snake_case("HTTPStatus"), "http_status");
        assert_eq!(snake_case("OrderLine"), "order_line");
        assert_eq!(snake_case("Line2Item"), "line2_item");
    }
}
