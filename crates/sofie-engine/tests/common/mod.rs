use sofie_engine::{Template, compile_template, load_template};

/// Every test renders the compiled program, which must be the parsed one.
pub fn loaded(name: &str, source: &str) -> Template {
    let parsed = Template::parse(name, source).unwrap_or_else(|error| panic!("{error}"));
    let compiled = compile_template(name, source).unwrap_or_else(|error| panic!("{error}"));
    let loaded = load_template(name, &compiled).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(format!("{loaded:?}"), format!("{parsed:?}"));
    loaded
}
