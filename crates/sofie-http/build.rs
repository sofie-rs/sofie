use std::process::ExitCode;

/// Every `t(…)` key a generated resource screen writes, compiled with its source-language text
/// for an app built without `i18n`.
#[cfg(feature = "resources")]
const RESOURCE_TEXTS: &str = "src/gate/optional/resource/texts.html";

fn main() -> ExitCode {
    #[cfg(feature = "resources")]
    if let Err(error) = compile_resource_texts() {
        eprintln!("compiling sofie-http's resource texts: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

#[cfg(feature = "resources")]
fn compile_resource_texts() -> Result<(), Box<dyn std::error::Error>> {
    use sofie_engine::{Components, Translations, compile_template_with_texts};

    println!("cargo::rerun-if-changed={RESOURCE_TEXTS}");
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").ok_or("cargo set no OUT_DIR")?);
    let source = std::fs::read_to_string(RESOURCE_TEXTS)?;
    let texts = Translations::new(&["en"])?;
    let compiled = compile_template_with_texts(
        "sofie_http/resource_texts",
        &source,
        &Components::default(),
        &texts,
    )?;
    std::fs::write(out.join("resource_texts.sft"), compiled)?;
    Ok(())
}
