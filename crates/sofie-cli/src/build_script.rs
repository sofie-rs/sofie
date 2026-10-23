use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use sofie_check::{Screens, report};
use sofie_engine::{Components, TranslationError, Translations, compile_template_with_texts};
use sofie_model::App;
use sofie_model::naming::{SOURCE_LANGUAGE, file_stem};

use crate::cargo::Diagnostics;
use crate::check_command::check_loaded_app;
use crate::clients::{build_clients, client_sources, has_clients};
use crate::gen_command::{GenError, readable_screen};
use crate::islands::{IslandMode, build_islands, has_islands, island_sources};
use crate::source_edits::{read, write};
use crate::wiring::{embedded_mails, embedded_screens};

#[must_use]
pub fn check_in_build_script(root: &Path) -> ExitCode {
    for path in watched_paths(root) {
        println!("cargo::rerun-if-changed={}", path.display());
    }
    match build(root) {
        Ok(()) => return ExitCode::SUCCESS,
        Err(GenError::Check(findings)) => eprint!("{}", report(root, &findings)),
        Err(error) => eprintln!("{error}"),
    }
    ExitCode::FAILURE
}

fn build(root: &Path) -> Result<(), GenError> {
    let app = sofie_model::load(root).map_err(GenError::Model)?;
    println!("{}", app_crate_variable(&app));
    let screens = Screens::read(&app.modules);
    let findings = check_loaded_app(root, &app, &screens)?;
    if !findings.is_empty() {
        return Err(GenError::Check(findings));
    }
    let Some(out) = env::var_os("OUT_DIR").map(PathBuf::from) else {
        return Err(GenError::Usage(
            "the screens compile in a build script, which Cargo gives OUT_DIR".to_owned(),
        ));
    };
    let texts = source_texts(&app)?;
    compile_screens(&app, &screens, &texts, &out.join("screens"))?;
    compile_mails(&app, &texts, &out.join("mails"))?;
    let mode = match env::var("PROFILE").as_deref() {
        Ok("release") => IslandMode::Release,
        _ => IslandMode::Development,
    };
    build_islands_into(&app, &out.join("islands"), mode)?;
    build_clients_into(&app, &screens, &out.join("clients"), mode)
}

/// `#[test_app]` names the app's crate by it, which `[lib] name` may set apart from the package.
fn app_crate_variable(app: &App) -> String {
    format!("cargo::rustc-env=SOFIE_APP_CRATE={}", app.app.crate_name)
}

/// Each `t(…)` call is compiled with its text, which an app built without `i18n` writes.
fn source_texts(app: &App) -> Result<Translations, GenError> {
    let refused = |error: TranslationError| {
        GenError::App(format!(
            "reading the app's `lang/{SOURCE_LANGUAGE}.toml` files: {error}"
        ))
    };
    let mut texts = Translations::new(&[SOURCE_LANGUAGE]).map_err(refused)?;
    for module in &app.modules {
        let sources = module
            .languages
            .iter()
            .filter(|path| file_stem(path) == SOURCE_LANGUAGE);
        for path in sources {
            texts
                .add(&module.krate.crate_name, SOURCE_LANGUAGE, &read(path)?)
                .map_err(refused)?;
        }
    }
    Ok(texts)
}

fn compile_screens(
    app: &App,
    screens: &Screens,
    texts: &Translations,
    out: &Path,
) -> Result<(), GenError> {
    for (key, file) in app.modules.iter().flat_map(embedded_screens) {
        let source = &readable_screen(screens, file)?.source;
        let compiled = compile_template_with_texts(&key, source, screens.components(), texts)
            .map_err(|error| GenError::Screen {
                path: file.to_path_buf(),
                message: error.to_string(),
            })?;
        write(&out.join(format!("{key}.sft")), &compiled)?;
    }
    Ok(())
}

fn compile_mails(app: &App, texts: &Translations, out: &Path) -> Result<(), GenError> {
    for mail in embedded_mails(&app.modules) {
        let source = read(&mail.file)?;
        let compiled =
            compile_template_with_texts(&mail.key, &source, &Components::default(), texts)
                .map_err(|error| GenError::Screen {
                    path: mail.file.clone(),
                    message: error.to_string(),
                })?;
        write(&out.join(format!("{}.sft", mail.key)), &compiled)?;
    }
    Ok(())
}

fn build_islands_into(app: &App, out: &Path, mode: IslandMode) -> Result<(), GenError> {
    if !has_islands(app) {
        return Ok(());
    }
    for source in island_sources(app) {
        println!("cargo::rerun-if-changed={}", source.display());
    }
    build_islands(app, out, mode, Diagnostics::Terminal).map(drop)
}

fn build_clients_into(
    app: &App,
    screens: &Screens,
    out: &Path,
    mode: IslandMode,
) -> Result<(), GenError> {
    if !has_clients(app) {
        return Ok(());
    }
    for source in client_sources() {
        println!("cargo::rerun-if-changed={}", source.display());
    }
    build_clients(app, screens, out, mode, Diagnostics::Terminal).map(drop)
}

/// Cargo watches a directory by every file beneath it: module sources,
/// templates, migrations and manifests are all under `modules/`.
fn watched_paths(root: &Path) -> Vec<PathBuf> {
    let app = root.join("app");
    let candidates = [
        root.join("modules"),
        app.join("Cargo.toml"),
        app.join("src"),
        app.join("tests"),
        root.join(sofie_check::ALLOW_FILE),
    ];
    // A watched path that does not exist would make Cargo rerun every build.
    candidates
        .into_iter()
        .filter(|path| path.exists())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use sofie_engine::{Components, compile_template_with_texts, load_template};
    use sofie_model::testing::{TemporaryDirectory, app, module};

    use super::{app_crate_variable, source_texts};

    #[test]
    fn the_build_script_names_the_app_s_crate_for_test_app() {
        let shop = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/shop");
        let app = sofie_model::load(&shop).unwrap();

        assert_eq!(
            app_crate_variable(&app),
            "cargo::rustc-env=SOFIE_APP_CRATE=shop"
        );
    }

    #[test]
    fn screens_are_compiled_with_the_source_language_s_texts_only() {
        let directory = TemporaryDirectory::new("sofie-source-texts");
        let mut order = module("order", directory.path("modules/order"));
        order.languages = vec![
            directory.write(
                "modules/order/lang/en.toml",
                "[orders]\ntitle = \"Your orders\"\n",
            ),
            directory.write(
                "modules/order/lang/tr.toml",
                "[orders]\ntitle = \"Siparişleriniz\"\n",
            ),
        ];
        let shop = app(directory.root(), vec![order]);

        let texts = source_texts(&shop).unwrap();
        let compiled = compile_template_with_texts(
            "order/web/orders",
            "<h1>{t(\"orders.title\")}</h1>",
            &Components::default(),
            &texts,
        )
        .unwrap();

        let loaded = load_template("order/web/orders", &compiled).unwrap();
        assert_eq!(
            loaded.render(&serde_json::json!({})).unwrap(),
            "<h1>Your orders</h1>"
        );
    }
}
