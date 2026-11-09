use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use sofie_engine::{Components, Translations, compile_template_with_texts};

const MAILS: &str = "src/mails";
/// What each `t(…)` call is compiled with, so an app built without `i18n` writes its text.
const SOURCE_TEXTS: &str = "lang/en.toml";

fn main() -> ExitCode {
    println!("cargo::rerun-if-changed={MAILS}");
    println!("cargo::rerun-if-changed={SOURCE_TEXTS}");
    match compile_mails() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("compiling sofie-tenancy's mails: {error}");
            ExitCode::FAILURE
        }
    }
}

fn compile_mails() -> Result<(), Box<dyn Error>> {
    let out = PathBuf::from(std::env::var_os("OUT_DIR").ok_or("cargo set no OUT_DIR")?);
    let directory = out.join("mails/sofie_tenancy");
    std::fs::create_dir_all(&directory)?;
    let mut texts = Translations::new(&["en"])?;
    texts.add(
        "sofie_tenancy",
        "en",
        &std::fs::read_to_string(SOURCE_TEXTS)?,
    )?;
    for file in std::fs::read_dir(MAILS)? {
        let path = file?.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "html")
        {
            compile(&path, &texts, &directory)?;
        }
    }
    Ok(())
}

fn compile(path: &Path, texts: &Translations, directory: &Path) -> Result<(), Box<dyn Error>> {
    let name = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or("a mail's name is not Unicode")?;
    let key = format!("sofie_tenancy/{name}");
    let source = std::fs::read_to_string(path)?;
    let compiled = compile_template_with_texts(&key, &source, &Components::default(), texts)?;
    std::fs::write(directory.join(format!("{name}.sft")), compiled)?;
    Ok(())
}
