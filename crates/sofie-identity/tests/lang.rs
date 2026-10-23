use std::collections::BTreeMap;

/// Each key with its sorted placeholders, read as the engine's TOML subset writes them.
fn keys(source: &str) -> BTreeMap<String, Vec<String>> {
    let mut table = String::new();
    let mut keys = BTreeMap::new();
    for line in source.lines().map(str::trim) {
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            table = format!("{name}.");
            continue;
        }
        let Some((name, text)) = line.split_once(" = ") else {
            continue;
        };
        let mut placeholders: Vec<String> = text
            .split('{')
            .skip(1)
            .filter_map(|rest| Some(rest.split_once('}')?.0.to_owned()))
            .collect();
        placeholders.sort();
        keys.insert(format!("{table}{name}"), placeholders);
    }
    keys
}

#[test]
fn every_language_shipped_has_the_source_language_s_screen_texts() {
    let files: BTreeMap<String, String> = sofie_identity::TRANSLATIONS
        .iter()
        .map(|file| (file.stem().to_owned(), file.text().unwrap().to_owned()))
        .collect();
    let source = keys(&files["en"]);
    assert!(source.contains_key("identity.action.sign_in"));

    for (language, text) in &files {
        let translated = keys(text);
        for (key, placeholders) in &source {
            assert_eq!(
                translated.get(key),
                Some(placeholders),
                "`{key}` in `{language}`"
            );
        }
    }
}
