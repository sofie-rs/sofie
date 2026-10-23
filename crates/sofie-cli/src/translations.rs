use std::fs;
use std::path::{Path, PathBuf};

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use sofie_check::{Finding, Rule, Screens};
use sofie_engine::{KeyUse, Template, TranslationError, Translations, check_translations};
use sofie_model::App;
use sofie_model::naming::{
    IDENTITY_PACKAGE, SOURCE_LANGUAGE, crate_name, file_stem, key_constant, snake_case,
};

use crate::gen_command::{GenError, io_error, readable_screen};
use crate::sorted;
use crate::source_edits::files_with_extension;

pub(crate) struct AppLanguages {
    languages: Vec<String>,
    files: Vec<LanguageFile>,
    identity: Option<Identity>,
}

pub(crate) struct LanguageFile {
    module: String,
    language: String,
    path: PathBuf,
    source: String,
}

struct Identity {
    files: Vec<LanguageFile>,
    screens: Vec<PathBuf>,
}

impl AppLanguages {
    pub(crate) fn read(app: &App) -> Result<Option<Self>, GenError> {
        let mut files = Vec::new();
        for module in sorted::modules(app) {
            files.extend(language_files(&module.krate.crate_name, &module.languages)?);
        }
        if files.is_empty() {
            return Ok(None);
        }
        let identity = app
            .identity
            .as_ref()
            .map(|_| read_identity(app))
            .transpose()?;
        Ok(Some(Self::new(files, identity)))
    }

    fn new(files: Vec<LanguageFile>, identity: Option<Identity>) -> Self {
        let named = files.iter().map(|file| file.language.clone());
        let mut languages =
            crate::sorted::first_seen(std::iter::once(SOURCE_LANGUAGE.to_owned()).chain(named));
        languages[1..].sort();
        Self {
            languages,
            files,
            identity,
        }
    }

    fn load(&self) -> Result<Translations, TranslationError> {
        let languages: Vec<&str> = self.languages.iter().map(String::as_str).collect();
        let mut translations = Translations::new(&languages)?;
        for file in &self.files {
            translations.add(&file.module, &file.language, &file.source)?;
        }
        if let Some(identity) = &self.identity {
            let builtin: Vec<(&str, &str)> = identity
                .files
                .iter()
                .map(|file| (file.language.as_str(), file.source.as_str()))
                .collect();
            translations.add_builtin(&crate_name(IDENTITY_PACKAGE), &builtin)?;
        }
        Ok(translations)
    }

    pub(crate) fn findings(&self, app: &App, screens: &Screens) -> Result<Vec<Finding>, GenError> {
        let translations = match self.load() {
            Ok(translations) => translations,
            Err(error) => return Ok(vec![self.load_finding(&error)]),
        };
        let identity_templates = self.identity_templates()?;
        let identity_templates = identity_templates
            .iter()
            .map(|(file, template)| (file.as_path(), template));
        let mut findings = Vec::new();
        let mut uses: Vec<KeyUse> = Vec::new();
        for (file, template) in module_templates(app, screens)?
            .into_iter()
            .chain(identity_templates)
        {
            if let Err(error) = check_translations(template, &translations) {
                findings.push(finding(
                    file.to_path_buf(),
                    error.line,
                    error.message.clone(),
                ));
            }
            uses.extend(sofie_engine::translation_keys(template));
        }
        for gap in translations.gaps(&uses) {
            let file = self.file_for(&gap.language);
            findings.push(finding(file, 1, gap.to_string()));
        }
        findings.extend(enum_clashes(app));
        findings.extend(self.key_constant_clashes(app));
        let known = translations.keys();
        findings.extend(
            app.modules
                .iter()
                .flat_map(|module| &module.code.translation_keys)
                .filter(|literal| known.binary_search(&literal.key.as_str()).is_err())
                .map(|literal| {
                    let message = format!(
                        "translation key `{}` is in no `lang/{SOURCE_LANGUAGE}.toml`, so its text \
                         would be the key itself",
                        literal.key
                    );
                    finding(literal.file.clone(), literal.line, message)
                }),
        );
        Ok(findings)
    }

    /// Each key becomes a constant of the contract's `keys`, which must be an identifier of its own.
    fn key_constant_clashes(&self, app: &App) -> Vec<Finding> {
        let mut findings = Vec::new();
        for module in &app.modules {
            let name = &module.krate.crate_name;
            let keys = match self.module_keys(name) {
                Ok(keys) => keys,
                Err(error) => return vec![self.load_finding(&error)],
            };
            let file = || self.file_for_module(name);
            let mut seen: Vec<(String, &str)> = Vec::new();
            for key in &keys {
                let constant = key_constant(key);
                if syn::parse_str::<syn::Ident>(&constant).is_err() {
                    let message = format!(
                        "translation key `{key}` becomes the constant `keys::{constant}`, which is \
                         no Rust identifier; start each of its parts with a letter"
                    );
                    findings.push(finding(file(), 1, message));
                } else if let Some((_, first)) = seen.iter().find(|(taken, _)| *taken == constant) {
                    let message = format!(
                        "translation keys `{first}` and `{key}` both become the constant \
                         `keys::{constant}`; rename one"
                    );
                    findings.push(finding(file(), 1, message));
                } else {
                    seen.push((constant, key));
                }
            }
        }
        findings
    }

    fn file_for_module(&self, module: &str) -> PathBuf {
        self.files
            .iter()
            .find(|file| file.module == module && file.language == SOURCE_LANGUAGE)
            .map(|file| file.path.clone())
            .unwrap_or_default()
    }

    /// What `sofie gen` writes as a contract's `keys`: the module's own, in its source language.
    pub(crate) fn module_keys(&self, module: &str) -> Result<Vec<String>, TranslationError> {
        let Some(file) = self
            .files
            .iter()
            .find(|file| file.module == module && file.language == SOURCE_LANGUAGE)
        else {
            return Ok(Vec::new());
        };
        let shipped = Translations::new(&[SOURCE_LANGUAGE])?;
        let mut own = Translations::new(&[SOURCE_LANGUAGE])?;
        own.add(module, SOURCE_LANGUAGE, &file.source)?;
        let shipped = shipped.keys();
        Ok(own
            .keys()
            .into_iter()
            .filter(|key| shipped.binary_search(key).is_err())
            .map(str::to_owned)
            .collect())
    }

    fn identity_templates(&self) -> Result<Vec<(PathBuf, Template)>, GenError> {
        let mut templates = Vec::new();
        for file in self.identity.iter().flat_map(|identity| &identity.screens) {
            let source = fs::read_to_string(file).map_err(|source| io_error(file, source))?;
            if let Ok(template) = Template::parse(&file.display().to_string(), &source) {
                templates.push((file.clone(), template));
            }
        }
        Ok(templates)
    }

    fn load_finding(&self, error: &TranslationError) -> Finding {
        match error {
            TranslationError::File {
                file,
                line,
                message,
                ..
            } => {
                let path = self
                    .all_files()
                    .find(|known| &known.name() == file)
                    .map_or_else(|| PathBuf::from(file), |known| known.path.clone());
                finding(path, *line, message.clone())
            }
            TranslationError::Declaration(message) => {
                finding(self.file_for(SOURCE_LANGUAGE), 1, message.clone())
            }
        }
    }

    fn all_files(&self) -> impl Iterator<Item = &LanguageFile> {
        let identity = self.identity.iter().flat_map(|identity| &identity.files);
        self.files.iter().chain(identity)
    }

    fn file_for(&self, language: &str) -> PathBuf {
        self.files
            .iter()
            .find(|file| file.language == language)
            .or_else(|| self.files.first())
            .map(|file| file.path.with_file_name(format!("{language}.toml")))
            .unwrap_or_default()
    }

    pub(crate) fn translate_statement(&self, app: &App) -> TokenStream {
        let languages = &self.languages;
        let files = self.files.iter().map(|file| {
            let module = &file.module;
            let file = crate::wiring::embedded_file(app, &file.path);
            quote!((#module, #file))
        });
        let enums = app_enums(app).into_iter().map(|(name, variants)| {
            let variants = variants.into_iter().map(|(variant, tone)| {
                let tone = format_ident!("{tone}");
                quote!((#variant, sofie::engine::Tone::#tone))
            });
            quote!((#name, &[#(#variants),*]))
        });
        let builtin = self.identity.iter().map(|_| {
            let name = crate_name(IDENTITY_PACKAGE);
            let identity = crate::wiring::path(&name);
            quote!((#name, #identity::TRANSLATIONS))
        });
        quote! {
            screens.translate(sofie::http::translations(
                &[#(#languages),*],
                &[#(#files),*],
                &[#(#builtin),*],
                &[#(#enums),*],
            )?);
        }
    }
}

/// Every module's `#[derive(Enum)]`, named as `enum.<name>.<variant>` keys name it, with
/// each variant's tone.
fn app_enums(app: &App) -> Vec<(String, Vec<(String, String)>)> {
    let mut enums = Vec::new();
    for module in sorted::modules(app) {
        for declared in &module.code.enums {
            let variants = declared
                .stored_names
                .iter()
                .zip(&declared.variants)
                .map(|(stored, variant)| (stored.clone(), tone_variant(variant.tone.as_deref())))
                .collect();
            enums.push((snake_case(&declared.name), variants));
        }
    }
    enums
}

fn tone_variant(tone: Option<&str>) -> String {
    let mut letters = tone.unwrap_or("neutral").chars();
    letters
        .next()
        .map(|first| first.to_uppercase().chain(letters).collect())
        .unwrap_or_default()
}

/// Their `enum.*` keys are shared, so two modules' enums of one name would mix.
fn enum_clashes(app: &App) -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut seen: Vec<(String, &str)> = Vec::new();
    for module in sorted::modules(app) {
        for declared in &module.code.enums {
            let name = snake_case(&declared.name);
            if let Some((_, first)) = seen.iter().find(|(seen, _)| *seen == name) {
                findings.push(Finding {
                    rule: Rule::ScreenTemplate,
                    file: module.krate.path.join("src/lib.rs"),
                    line: 1,
                    message: format!(
                        "enum `{}` of module `{}` shares its `enum.{name}.*` keys with module \
                         `{first}`'s",
                        declared.name, module.krate.package
                    ),
                    fix: "rename one of the two enums".to_owned(),
                });
            } else {
                seen.push((name, &module.krate.package));
            }
        }
    }
    findings
}

fn finding(file: PathBuf, line: usize, message: String) -> Finding {
    Finding {
        rule: Rule::ScreenTemplate,
        file,
        line,
        message,
        fix: "add the key to each language's `lang/<language>.toml`, with the same placeholders"
            .to_owned(),
    }
}

impl LanguageFile {
    fn name(&self) -> String {
        format!("{}/lang/{}.toml", self.module, self.language)
    }
}

fn language_files(module: &str, paths: &[PathBuf]) -> Result<Vec<LanguageFile>, GenError> {
    paths
        .iter()
        .cloned()
        .map(|path| {
            let source = fs::read_to_string(&path).map_err(|source| io_error(&path, source))?;
            Ok(LanguageFile {
                module: module.to_owned(),
                language: file_stem(&path),
                path,
                source,
            })
        })
        .collect()
}

fn module_templates<'s>(
    app: &'s App,
    screens: &'s Screens,
) -> Result<Vec<(&'s Path, &'s Template)>, GenError> {
    let files = app.modules.iter().flat_map(|module| {
        module.areas.iter().flat_map(|area| {
            let overrides = area.overrides.iter().map(|replacement| &replacement.file);
            area.screens.iter().chain(overrides)
        })
    });
    let mut templates = Vec::new();
    for file in files {
        if let Ok(template) = &readable_screen(screens, file)?.template {
            templates.push((file.as_path(), template));
        }
    }
    Ok(templates)
}

fn read_identity(app: &App) -> Result<Identity, GenError> {
    let krate = sofie_model::dependency_crate(&app.root, IDENTITY_PACKAGE)
        .map_err(GenError::Model)?
        .ok_or_else(|| {
            GenError::Model(sofie_model::ModelError::in_file(
                &app.app.path.join("Cargo.toml"),
                "the app uses sofie-identity but cargo does not list it",
            ))
        })?;
    Ok(Identity {
        files: language_files(
            &crate_name(IDENTITY_PACKAGE),
            &files_with_extension(&krate.path.join("lang"), "toml")?,
        )?,
        screens: files_with_extension(&krate.path.join("src/screens"), "html")?,
    })
}

#[cfg(test)]
mod tests {
    use sofie_model::testing::{app, module};

    use super::*;

    fn file(language: &str, source: &str) -> LanguageFile {
        LanguageFile {
            module: "order".to_owned(),
            language: language.to_owned(),
            path: PathBuf::from(format!("shop/modules/order/lang/{language}.toml")),
            source: source.to_owned(),
        }
    }

    fn rendered(tokens: &TokenStream) -> String {
        crate::rustfmt::format_rust(&tokens.to_string(), std::path::Path::new("generated.rs"))
            .unwrap()
    }

    #[test]
    fn declares_the_source_language_then_each_file_s() {
        let languages = AppLanguages::new(vec![file("tr", ""), file("de", "")], None);
        let shop = app("shop", vec![module("order", "shop/modules/order")]);

        let statement = languages.translate_statement(&shop);
        let statement = rendered(&quote!(fn screens() { #statement }));

        assert!(
            statement.contains("&[\"en\", \"de\", \"tr\"]"),
            "{statement}"
        );
        assert!(
            statement.contains("(\"order\", sofie::db::embed!(\"../modules/order/lang/tr.toml\"))"),
            "{statement}"
        );
    }

    #[test]
    fn a_file_that_does_not_load_is_a_finding_at_that_file() {
        let languages = AppLanguages::new(vec![file("tr", "[orders\n")], None);

        let findings = languages
            .findings(&app("shop", Vec::new()), &Screens::read(&[]))
            .unwrap();

        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].file,
            PathBuf::from("shop/modules/order/lang/tr.toml")
        );
        assert_eq!(findings[0].line, 1);
    }

    #[test]
    fn a_key_one_language_lacks_is_a_gap() {
        let languages = AppLanguages::new(
            vec![
                file("de", "[orders]\ntitle = \"Bestellungen\"\n"),
                file("tr", "[label]\nstatus = \"Durum\"\n"),
            ],
            None,
        );

        let findings = languages
            .findings(&app("shop", Vec::new()), &Screens::read(&[]))
            .unwrap();
        let messages: Vec<String> = findings
            .iter()
            .map(|finding| finding.message.clone())
            .collect();

        assert!(
            messages.contains(&"`orders.title` has no `en` translation".to_owned()),
            "{messages:?}"
        );
        assert!(
            messages.contains(&"`label.status` has no `de` translation".to_owned()),
            "{messages:?}"
        );
    }

    /// `sofie gen` names each key as a constant of the contract's `keys`; one no identifier
    /// panicked gen and check, and two of one name broke the contract.
    #[test]
    fn a_key_whose_constant_is_no_identifier_or_taken_twice_is_a_finding() {
        let languages = AppLanguages::new(
            vec![file(
                "en",
                "orders_title = \"Orders\"\n[404]\ntitle = \"Not found\"\n[orders]\ntitle = \"Orders\"\n",
            )],
            None,
        );
        let shop = app("shop", vec![module("order", "shop/modules/order")]);

        let findings = languages.findings(&shop, &Screens::read(&[])).unwrap();
        let messages: Vec<&str> = findings
            .iter()
            .map(|finding| finding.message.as_str())
            .collect();

        assert_eq!(messages.len(), 2, "{messages:?}");
        assert!(messages[0].contains("`404.title`"), "{messages:?}");
        assert!(messages[1].contains("`keys::ORDERS_TITLE`"), "{messages:?}");
    }

    #[test]
    fn registers_each_module_enum_with_its_tones() {
        let languages = AppLanguages::new(vec![file("tr", "")], None);
        let mut order = module("order", "shop/modules/order");
        let mut status =
            sofie_model::testing::enumeration("OrderStatus", &[("Open", &[]), ("Paid", &[])]);
        status.variants[1].tone = Some("success".to_owned());
        order.code.enums.push(status);

        let statement = languages.translate_statement(&app("shop", vec![order]));
        let statement = rendered(&quote!(fn screens() { #statement }));

        assert!(
            statement.contains(
                "&[\n                (\"OPEN\", sofie::engine::Tone::Neutral),\n                \
                 (\"PAID\", sofie::engine::Tone::Success),\n            ],"
            ),
            "{statement}"
        );
    }

    #[test]
    fn a_tone_starting_with_a_letter_of_several_bytes_is_capitalized_without_panicking() {
        assert_eq!(tone_variant(Some("ünlü")), "Ünlü");
        assert_eq!(tone_variant(None), "Neutral");
    }

    #[test]
    fn a_key_rust_names_must_be_in_the_source_language() {
        let languages = AppLanguages::new(vec![file("en", "[order]\nlate = \"Late\"\n")], None);
        let mut order = module("order", "shop/modules/order");
        let literal = |key: &str, line| sofie_model::KeyLiteral {
            key: key.to_owned(),
            file: PathBuf::from("shop/modules/order/src/actions.rs"),
            line,
        };
        order.code.translation_keys = vec![
            literal("order.late", 3),
            literal("validation.max", 4),
            literal("order.early", 5),
        ];

        let findings = languages
            .findings(&app("shop", vec![order]), &Screens::read(&[]))
            .unwrap();

        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].line, 5);
        assert!(
            findings[0].message.contains("`order.early`"),
            "{}",
            findings[0].message
        );
    }
}
