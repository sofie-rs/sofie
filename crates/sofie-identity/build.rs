use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use sofie_engine::{Components, Translations, compile_template_with_texts};

/// What each `t(…)` call is compiled with, so an app built without `i18n` writes its text.
const SOURCE_TEXTS: &str = "lang/en.toml";

/// Every template is compiled whatever the features, so the sources need no list beside them.
const FOLDERS: [Folder; 2] = [
    Folder {
        source: "src/screens",
        target: "screens/sofie_identity/web",
        keys: "sofie_identity/web",
    },
    Folder {
        source: "src/mails",
        target: "mails/sofie_identity",
        keys: "sofie_identity",
    },
];

/// Templates elsewhere in `src` are shipped screens, each compiled beside its path under `shipped`.
const SHIPPED_TARGET: &str = "shipped";

/// Copies of the generated resource screens, with `{prefix}` where an area's path goes.
const ROLE_SCREENS: &str = "role_";

struct Folder {
    source: &'static str,
    target: &'static str,
    keys: &'static str,
}

fn main() -> ExitCode {
    println!("cargo::rerun-if-changed=src");
    println!("cargo::rerun-if-changed={SOURCE_TEXTS}");
    match compile_templates() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("compiling sofie-identity's templates: {error}");
            ExitCode::FAILURE
        }
    }
}

fn compile_templates() -> Result<(), Box<dyn Error>> {
    let out = PathBuf::from(std::env::var_os("OUT_DIR").ok_or("cargo set no OUT_DIR")?);
    let mut texts = Translations::new(&["en"])?;
    texts.add(
        "sofie_identity",
        "en",
        &std::fs::read_to_string(SOURCE_TEXTS)?,
    )?;
    for folder in FOLDERS {
        let directory = out.join(folder.target);
        std::fs::create_dir_all(&directory)?;
        for file in std::fs::read_dir(folder.source)? {
            let path = file?.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "html")
            {
                compile(&path, folder.keys, &texts, &directory)?;
            }
        }
    }
    compile_shipped(Path::new("src"), &texts, &out.join(SHIPPED_TARGET))
}

fn compile_shipped(
    directory: &Path,
    texts: &Translations,
    target: &Path,
) -> Result<(), Box<dyn Error>> {
    if FOLDERS
        .iter()
        .any(|folder| directory == Path::new(folder.source))
    {
        return Ok(());
    }
    for file in std::fs::read_dir(directory)? {
        let path = file?.path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if path.is_dir() {
            compile_shipped(&path, texts, &target.join(name))?;
        } else if path
            .extension()
            .is_some_and(|extension| extension == "html")
            && !name.starts_with(ROLE_SCREENS)
        {
            std::fs::create_dir_all(target)?;
            compile(&path, "sofie_identity/shipped", texts, target)?;
        }
    }
    Ok(())
}

fn compile(
    path: &Path,
    keys: &str,
    texts: &Translations,
    directory: &Path,
) -> Result<(), Box<dyn Error>> {
    let name = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or("a template's name is not Unicode")?;
    let key = format!("{keys}/{name}");
    let source = std::fs::read_to_string(path)?;
    let compiled = compile_template_with_texts(&key, &source, &Components::default(), texts)?;
    std::fs::write(directory.join(format!("{name}.sft")), compiled)?;
    Ok(())
}
