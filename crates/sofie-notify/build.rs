use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use sofie_engine::{Components, Translations, compile_template_with_texts};

const MAILS: &str = "src/mails";
const SCREENS: &str = "src/screens";
/// What each screen's `t(…)` call is compiled with, so an app built without `i18n` writes its text.
const SOURCE_TEXTS: &str = "lang/en.toml";

fn main() -> ExitCode {
    for watched in [MAILS, SCREENS, SOURCE_TEXTS] {
        println!("cargo::rerun-if-changed={watched}");
    }
    match compile_templates() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("compiling sofie-notify's templates: {error}");
            ExitCode::FAILURE
        }
    }
}

fn compile_templates() -> Result<(), Box<dyn Error>> {
    let out = PathBuf::from(std::env::var_os("OUT_DIR").ok_or("cargo set no OUT_DIR")?);
    let mut texts = Translations::new(&["en"])?;
    texts.add(
        "sofie_notify",
        "en",
        &std::fs::read_to_string(SOURCE_TEXTS)?,
    )?;
    let mails = out.join("mails/sofie_notify");
    for (path, name) in templates(MAILS)? {
        let key = format!("sofie_notify/{name}");
        let compiled = sofie_engine::compile_template(&key, &std::fs::read_to_string(&path)?)?;
        write(&mails, &name, &compiled)?;
    }
    let screens = out.join("screens");
    for (path, name) in templates(SCREENS)? {
        let key = format!("sofie_notify/{name}");
        let source = std::fs::read_to_string(&path)?;
        let compiled = compile_template_with_texts(&key, &source, &Components::default(), &texts)?;
        write(&screens, &name, &compiled)?;
    }
    Ok(())
}

fn templates(directory: &str) -> Result<Vec<(PathBuf, String)>, Box<dyn Error>> {
    let mut found = Vec::new();
    for file in std::fs::read_dir(directory)? {
        let path = file?.path();
        if path.extension().is_none_or(|extension| extension != "html") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or("a template's name is not Unicode")?
            .to_owned();
        found.push((path, name));
    }
    Ok(found)
}

fn write(directory: &Path, name: &str, compiled: &[u8]) -> Result<(), Box<dyn Error>> {
    std::fs::create_dir_all(directory)?;
    std::fs::write(directory.join(format!("{name}.sft")), compiled)?;
    Ok(())
}
