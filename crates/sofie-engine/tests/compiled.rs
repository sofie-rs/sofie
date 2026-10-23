use serde_json::json;
use sofie_engine::{compile_template, load_template};

const SOURCE: &str = r#"<p id="greeting">Hello {data.name}</p>"#;

#[test]
fn a_loaded_program_renders_as_the_parsed_one() {
    let compiled = compile_template("shop/web/hello", SOURCE).unwrap();
    let loaded = load_template("shop/web/hello", &compiled).unwrap();
    let data = json!({"name": "<Ada>"});

    assert_eq!(
        loaded.render(&data).unwrap(),
        "<p id=\"greeting\">Hello &lt;Ada&gt;</p>"
    );
    assert_eq!(
        loaded.render_out_of_band(&data, "greeting").unwrap(),
        "<p hx-swap-oob=\"true\" id=\"greeting\">Hello &lt;Ada&gt;</p>"
    );
}

#[test]
fn a_program_of_another_format_names_the_screen_and_asks_for_a_rebuild() {
    let mut compiled = compile_template("shop/web/hello", SOURCE).unwrap();
    compiled[3] = compiled[3].wrapping_add(1);

    let error = load_template("shop/web/hello", &compiled)
        .unwrap_err()
        .to_string();

    assert!(error.starts_with("shop/web/hello:1:1: "), "{error}");
    assert!(error.contains("compiled in format"), "{error}");
    assert!(error.contains("rebuild the app"), "{error}");
}

#[test]
fn damaged_or_foreign_bytes_are_refused() {
    let compiled = compile_template("shop/web/hello", SOURCE).unwrap();

    for bytes in [
        &compiled[..compiled.len() - 1],
        b"<p>text</p>".as_slice(),
        &[],
    ] {
        let error = load_template("shop/web/hello", bytes)
            .unwrap_err()
            .to_string();
        assert!(error.contains("rebuild the app"), "{error}");
    }
}
