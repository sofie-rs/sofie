use std::collections::BTreeMap;

const LANGUAGES: [(&str, &str); 3] = [
    ("en", include_str!("../lang/en.toml")),
    ("de", include_str!("../lang/de.toml")),
    ("tr", include_str!("../lang/tr.toml")),
];

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
fn every_language_has_the_source_language_s_keys_and_placeholders() {
    let source = keys(LANGUAGES[0].1);
    assert_eq!(
        source["tenancy.rule.last_owner_of"],
        ["organizations"],
        "the rule texts are keyed"
    );

    for (language, text) in &LANGUAGES[1..] {
        assert_eq!(keys(text), source, "{language}");
    }
}
