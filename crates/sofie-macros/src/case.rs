pub(crate) fn snake_case(name: &str) -> String {
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

pub(crate) fn screaming_snake_case(name: &str) -> String {
    snake_case(name).to_uppercase()
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
    use super::*;

    #[test]
    fn snake_case_splits_words_and_acronyms() {
        assert_eq!(snake_case("Order"), "order");
        assert_eq!(snake_case("OrderLine"), "order_line");
        assert_eq!(snake_case("HTTPRequest"), "http_request");
        assert_eq!(snake_case("Line2Item"), "line2_item");
    }

    #[test]
    fn screaming_snake_case_upper_cases_the_words() {
        assert_eq!(screaming_snake_case("PartlyShipped"), "PARTLY_SHIPPED");
        assert_eq!(screaming_snake_case("Paid"), "PAID");
    }
}
