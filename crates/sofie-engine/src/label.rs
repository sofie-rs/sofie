#[must_use]
pub fn humanize(name: &str) -> String {
    let spaced = name.replace('_', " ");
    let mut characters = spaced.trim().chars();
    characters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(characters).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::humanize;

    #[test]
    fn humanizes_snake_case_names() {
        assert_eq!(humanize("created_at"), "Created at");
        assert_eq!(humanize("orders"), "Orders");
        assert_eq!(humanize("_"), "");
    }
}
