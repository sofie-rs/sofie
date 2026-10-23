/// How both sofie-http and `sofie gen` name a resource's path and list handler.
#[must_use]
pub fn plural(name: &str) -> String {
    if let Some(stem) = name.strip_suffix('y')
        && !stem.ends_with(['a', 'e', 'i', 'o', 'u'])
    {
        return format!("{stem}ies");
    }
    if name.ends_with('s')
        && !["ss", "us", "is", "as", "os"]
            .iter()
            .any(|end| name.ends_with(end))
    {
        return name.to_owned();
    }
    if name.ends_with(['s', 'x', 'z']) || name.ends_with("ch") || name.ends_with("sh") {
        return format!("{name}es");
    }
    format!("{name}s")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_takes_its_english_plural_ending() {
        for (name, expected) in [
            ("order", "orders"),
            ("order_line", "order_lines"),
            ("category", "categories"),
            ("day", "days"),
            ("address", "addresses"),
            ("box", "boxes"),
            ("batch", "batches"),
            ("opening_hours", "opening_hours"),
            ("settings", "settings"),
            ("status", "statuses"),
            ("bus", "buses"),
            ("alias", "aliases"),
            ("glass", "glasses"),
        ] {
            assert_eq!(plural(name), expected, "{name}");
        }
    }
}
