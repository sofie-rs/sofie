mod account_screen;
mod area;
mod commands;
mod declared_tables;
mod detail_screen;
pub(crate) mod edits;
mod events;
mod mail;
mod menu;
mod plan;
mod read_model;
mod route;
mod schema;
mod schema_migration;
mod search;
pub(crate) mod task;

use std::path::Path;

use sofie_model::naming::{contract_package, crate_name, snake_case};
use sofie_model::{App, Module};

use crate::fix_command::generated_export_lines;
use crate::gen_command::{GenError, GenMode, GenOutcome, gen_app};
use crate::source_edits::{
    Journal, path_dependency, read, toml_escaped, with_dependency_line, with_lines_appended,
};

pub use account_screen::AccountScreen;
pub use area::AreaAccess;
pub use commands::{NEW_COMMANDS, NewCommand};
use edits::Edits;
pub use edits::FileChange;
pub(crate) use edits::{dependency_line_start, workspace_declaration};
pub use menu::NewMenuEntry;
pub use route::NewRoute;

pub enum NewCode {
    /// One of sofie-identity's screens of a member's own account, mounted for a member area.
    AccountScreen {
        area: String,
        screen: AccountScreen,
    },
    /// An area defined in the module's contract, with a first page.
    Area {
        module: String,
        name: String,
        access: AreaAccess,
    },
    /// Signing in as a member of `member`, for a permitted principal of `permitted`.
    Impersonation {
        permitted: String,
        member: String,
    },
    /// Organizations acting in `organization`, listed and joined in `member`.
    Tenancy {
        organization: String,
        member: String,
    },
    /// A mail template and the function sending it; bulk mail of a list with one.
    Mail {
        module: String,
        name: String,
        list: Option<String>,
    },
    Migration {
        module: String,
        name: String,
    },
    /// `sofie gen migration`: what the aggregates declare beyond what the migrations create.
    SchemaMigration {
        module: String,
        name: String,
    },
    Aggregate {
        module: String,
        name: String,
    },
    Event {
        module: String,
        name: String,
    },
    Listener {
        module: String,
        event: String,
        handler: String,
    },
    MenuEntry(NewMenuEntry),
    /// A plan in the module's `src/plans.rs`, sofie-billing's catalog.
    Plan {
        module: String,
        name: String,
    },
    Route(NewRoute),
    Task {
        module: String,
        name: String,
    },
    /// Full-text search over an aggregate's searched fields.
    Search {
        module: String,
        aggregate: String,
    },
}

const KEEP: &str = ".gitkeep";

pub fn new_app(parent: &Path, name: &str) -> Result<Vec<GenOutcome>, GenError> {
    let name = checked_package_name(name)?;
    let root = parent.join(name);
    refuse_existing(&root)?;
    let crate_name = crate_name(name);
    let [build, library, main] = app_sources(&crate_name);
    let files = [
        ("Cargo.toml", workspace_manifest()),
        (".gitignore", "/target\n/.env\n".to_owned()),
        ("app/Cargo.toml", app_manifest(name)),
        build,
        library,
        main,
        (&format!("modules/{KEEP}"), String::new()),
    ];
    let mut journal = Journal::default();
    let written =
        write_all(&mut journal, &root, &files).and_then(|()| gen_app(&root, GenMode::Write));
    journal.finish(written)
}

/// The app crate's code besides its manifest, which only `sofie gen`'s wiring adds to.
pub(crate) fn app_sources(crate_name: &str) -> [(&'static str, String); 3] {
    [
        (
            "app/build.rs",
            "fn main() -> std::process::ExitCode {\n    \
             sofie_cli::check_in_build_script(std::path::Path::new(\"..\"))\n}\n"
                .to_owned(),
        ),
        ("app/src/lib.rs", APP_LIBRARY.to_owned()),
        (
            "app/src/main.rs",
            format!(
                "fn main() -> sofie::core::Result<()> {{\n    \
                 sofie::run({crate_name}::MODULES, {crate_name}::app, {crate_name}::worker)\n}}\n"
            ),
        ),
    ]
}

pub fn new_module(root: &Path, name: &str) -> Result<Vec<GenOutcome>, GenError> {
    let name = checked_package_name(name)?;
    let directory = root.join("modules").join(name);
    refuse_existing(&directory)?;
    let app = sofie_model::load(root).map_err(GenError::Model)?;
    if crate_name(&app.app.package) == name {
        return Err(GenError::Usage(format!(
            "`{name}` is the app's own package; choose another name for the module"
        )));
    }
    let crate_name = crate_name(name);
    let config = sofie_model::config_type_name(&crate_name);
    let files = [
        ("Cargo.toml", module_manifest(name)),
        ("contract/Cargo.toml", contract_manifest(name)),
        (
            "contract/src/lib.rs",
            format!("//! What other modules may use of module `{name}`.\n"),
        ),
        ("src/lib.rs", MODULE_LIBRARY.to_owned()),
        ("src/module.rs", module_declaration(&crate_name)),
        (
            "src/config.rs",
            format!("#[derive(sofie::Config)]\npub struct {config} {{}}\n"),
        ),
        (
            "src/areas/web/routes.rs",
            "use sofie::prelude::*;\n\npub fn routes(_routes: &mut Routes<Web>) {}\n".to_owned(),
        ),
        (&format!("src/areas/web/screens/{KEEP}"), String::new()),
        (&format!("migrations/{KEEP}"), String::new()),
        (&format!("lang/{KEEP}"), String::new()),
    ];
    let mut journal = Journal::default();
    let written = write_all(&mut journal, &directory, &files)
        .and_then(|()| include_contracts(&mut journal, &root.join("Cargo.toml")))
        .and_then(|()| {
            let manifest = root.join("app").join("Cargo.toml");
            let dependency = path_dependency(name, &format!("../modules/{name}"));
            journal.write(
                &manifest,
                with_dependency_line(&read(&manifest)?, &dependency),
            )
        })
        .and_then(|()| gen_app(root, GenMode::Write));
    journal.finish(written)
}

/// Writes only the sources; `sofie gen` and `sofie check` follow, as after any edit.
/// A failure part-way undoes what was written.
pub fn new_code(root: &Path, code: &NewCode) -> Result<Vec<FileChange>, GenError> {
    let app = sofie_model::load(root).map_err(GenError::Model)?;
    let mut edits = Edits::default();
    let mut generated = generate(&mut edits, &app, code);
    if matches!(code, NewCode::Route(_) | NewCode::Area { .. }) {
        generated = generated.and_then(|()| export_generated(&mut edits, root));
    }
    edits.finish(generated)
}

/// A route's builder and permission are written by `sofie gen` into the contract, which must export them.
fn export_generated(edits: &mut Edits, root: &Path) -> Result<(), GenError> {
    let app = sofie_model::load(root).map_err(GenError::Model)?;
    for module in &app.modules {
        let Some(contract) = &module.contract else {
            continue;
        };
        let lines = generated_export_lines(&app, module);
        if lines.is_empty() {
            continue;
        }
        let library = contract.path.join("src").join("lib.rs");
        edits.edit_rust(&library, |text| Ok(with_lines_appended(text, &lines)))?;
    }
    Ok(())
}

fn generate(edits: &mut Edits, app: &App, code: &NewCode) -> Result<(), GenError> {
    match code {
        NewCode::Migration { module, name } => schema::new_migration(
            edits,
            find_module(app, module)?,
            checked_name(name)?,
            "",
            |_| String::new(),
        ),
        NewCode::SchemaMigration { module, name } => schema_migration::new_schema_migration(
            edits,
            find_module(app, module)?,
            checked_name(name)?,
        ),
        NewCode::Aggregate { module, name } => {
            schema::new_aggregate(edits, find_module(app, module)?, checked_type_name(name)?)
        }
        NewCode::Event { module, name } => events::new_event(
            edits,
            app,
            find_module(app, module)?,
            checked_type_name(name)?,
        ),
        NewCode::Listener {
            module,
            event,
            handler,
        } => events::new_listener(
            edits,
            app,
            find_module(app, module)?,
            checked_type_name(event)?,
            checked_name(handler)?,
        ),
        NewCode::MenuEntry(entry) => menu::new_menu_entry(edits, app, entry),
        NewCode::Area {
            module,
            name,
            access,
        } => area::new_area(edits, app, module, name, *access),
        NewCode::AccountScreen { area, screen } => {
            account_screen::new_account_screen(edits, app, area, *screen)
        }
        NewCode::Impersonation { permitted, member } => {
            account_screen::new_impersonation(edits, app, permitted, member)
        }
        NewCode::Tenancy {
            organization,
            member,
        } => account_screen::new_tenancy(edits, app, organization, member),
        NewCode::Mail { module, name, list } => mail::new_mail(
            edits,
            find_module(app, module)?,
            checked_name(name)?,
            list.as_deref(),
        ),
        NewCode::Plan { module, name } => {
            plan::new_plan(edits, app, find_module(app, module)?, checked_name(name)?)
        }
        NewCode::Route(new_route) => route::new_route(edits, app, new_route),
        NewCode::Task { module, name } => {
            task::new_task(edits, app, find_module(app, module)?, checked_name(name)?)
        }
        NewCode::Search { module, aggregate } => search::new_search(
            edits,
            find_module(app, module)?,
            checked_type_name(aggregate)?,
        ),
    }
}

pub(crate) fn find_module<'app>(app: &'app App, name: &str) -> Result<&'app Module, GenError> {
    app.modules
        .iter()
        .find(|module| module.krate.crate_name == name || module.krate.package == name)
        .ok_or_else(|| {
            let known: Vec<&str> = app
                .modules
                .iter()
                .map(|module| module.krate.crate_name.as_str())
                .collect();
            GenError::Usage(format!(
                "the app has no module `{name}`; its modules: {}",
                known.join(", ")
            ))
        })
}

fn checked_type_name(name: &str) -> Result<&str, GenError> {
    let pascal = name.starts_with(|first: char| first.is_ascii_uppercase())
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric());
    if !pascal {
        return Err(GenError::Usage(format!(
            "`{name}` is not a type name `sofie new` takes: start with an uppercase letter and use \
             only letters and digits, as in `OrderPlaced`"
        )));
    }
    refuse_keyword(name, name)?;
    refuse_keyword(name, &snake_case(name))?;
    refuse_prelude_type(name)?;
    Ok(name)
}

fn refuse_prelude_type(name: &str) -> Result<(), GenError> {
    if !PRELUDE_TYPES.contains(&name) {
        return Ok(());
    }
    Err(GenError::Usage(format!(
        "`{name}` is a name a prelude brings into every file (`sofie::prelude` or Rust's), and the \
         generated code naming that one would reach the new type instead; choose another name"
    )))
}

/// Cargo refuses a member glob matching nothing, so contracts join the workspace only once a module has one.
fn include_contracts(journal: &mut Journal, workspace: &Path) -> Result<(), GenError> {
    const MODULES: &str = "\"modules/*\"";
    const CONTRACTS: &str = "\"modules/*/contract\"";
    if !workspace.is_file() {
        return Ok(());
    }
    let text = read(workspace)?;
    if text.contains(CONTRACTS) || !text.contains(MODULES) {
        return Ok(());
    }
    journal.write(
        workspace,
        text.replacen(MODULES, &format!("{MODULES}, {CONTRACTS}"), 1),
    )
}

/// Rust 2024's strict and reserved words: a generated `mod`, `fn` or crate cannot take one.
const RUST_KEYWORDS: [&str; 51] = [
    "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "crate",
    "do", "dyn", "else", "enum", "extern", "false", "final", "fn", "for", "gen", "if", "impl",
    "in", "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref",
    "return", "self", "Self", "static", "struct", "super", "trait", "true", "try", "type",
    "typeof", "unsafe", "unsized", "use", "virtual", "where", "while",
];
/// The types `sofie::prelude::*` and Rust's prelude bring into every generated file: a
/// generated type of the same name hides one the generated code names.
const PRELUDE_TYPES: [&str; 87] = [
    "Access",
    "Aggregate",
    "App",
    "Area",
    "Child",
    "Children",
    "Clock",
    "Currency",
    "Date",
    "Dep",
    "Email",
    "Enum",
    "Error",
    "Event",
    "Extensions",
    "Groups",
    "Id",
    "Inbox",
    "Input",
    "Instant",
    "Jobs",
    "Load",
    "LocalJobs",
    "Lookup",
    "Mailer",
    "Menu",
    "Meta",
    "Money",
    "Navigation",
    "Notify",
    "Page",
    "Paginated",
    "PersonName",
    "Preferences",
    "Principal",
    "Prune",
    "Read",
    "ReadModel",
    "Repeat",
    "Resources",
    "Result",
    "Routes",
    "Select",
    "Session",
    "SoftDelete",
    "SoftDeletes",
    "Timestamped",
    "Url",
    "Value",
    "Web",
    "AsMut",
    "AsRef",
    "Box",
    "Clone",
    "Copy",
    "Default",
    "DoubleEndedIterator",
    "Drop",
    "Eq",
    "Err",
    "ExactSizeIterator",
    "Extend",
    "Fn",
    "FnMut",
    "FnOnce",
    "From",
    "FromIterator",
    "Into",
    "IntoIterator",
    "Iterator",
    "None",
    "Ok",
    "Option",
    "Ord",
    "PartialEq",
    "PartialOrd",
    "Send",
    "Sized",
    "Some",
    "String",
    "Sync",
    "ToOwned",
    "ToString",
    "TryFrom",
    "TryInto",
    "Unpin",
    "Vec",
];
/// Crates every app reaches by name, which a package of the same name would shadow.
const TAKEN_CRATES: [&str; 6] = ["sofie", "std", "core", "alloc", "test", "proc_macro"];

pub(crate) fn checked_name(name: &str) -> Result<&str, GenError> {
    let plain = name.starts_with(|first: char| first.is_ascii_lowercase())
        && name.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        });
    if !plain {
        return Err(GenError::Usage(format!(
            "`{name}` is not a name `sofie new` takes: start with a lowercase letter and use only \
             lowercase letters, digits and `_`"
        )));
    }
    refuse_keyword(name, name)?;
    Ok(name)
}

/// A name that becomes a package: no keyword, and none of the crates every app links.
fn checked_package_name(name: &str) -> Result<&str, GenError> {
    let name = checked_name(name)?;
    if TAKEN_CRATES.contains(&name) {
        return Err(GenError::Usage(format!(
            "`{name}` names a crate every app links ({}); choose another name",
            TAKEN_CRATES.join(", ")
        )));
    }
    Ok(name)
}

fn refuse_keyword(name: &str, identifier: &str) -> Result<(), GenError> {
    if !RUST_KEYWORDS.contains(&identifier) {
        return Ok(());
    }
    Err(GenError::Usage(format!(
        "`{name}` would become the Rust keyword `{identifier}` in the generated code; choose \
         another name"
    )))
}

pub(crate) fn refuse_existing(directory: &Path) -> Result<(), GenError> {
    if !directory.exists() {
        return Ok(());
    }
    Err(GenError::Usage(format!(
        "{} exists already; choose another name or remove it",
        directory.display()
    )))
}

pub(crate) fn write_all(
    journal: &mut Journal,
    directory: &Path,
    files: &[(&str, String)],
) -> Result<(), GenError> {
    for (relative, text) in files {
        journal.write(&directory.join(relative), text)?;
    }
    Ok(())
}

/// Where a new app finds framework crate `package` until the framework is published: the
/// commit `cargo install --git` built this binary from, else this checkout by path.
pub(super) fn framework_source(package: &str) -> String {
    source_of(package, option_env!("SOFIE_CLI_COMMIT"))
}

fn source_of(package: &str, commit: Option<&str>) -> String {
    if let Some(commit) = commit {
        return format!(
            "git = \"{}\", rev = \"{commit}\"",
            env!("CARGO_PKG_REPOSITORY")
        );
    }
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let directory = if package == "sofie-framework" {
        "sofie"
    } else {
        package
    };
    let path = crates.join(directory).display().to_string();
    format!("path = \"{}\"", toml_escaped(&path))
}

fn workspace_manifest() -> String {
    format!(
        "[workspace]\nresolver = \"3\"\nmembers = [\"app\", \"modules/*\"]\n\n\
         [workspace.package]\nedition = \"2024\"\nversion = \"0.1.0\"\npublish = false\n\n\
         [workspace.dependencies]\nsofie = {{ package = \"sofie-framework\", {} }}\nsofie-cli = {{ {} }}\n\n\
         [workspace.lints.rust]\nunsafe_code = \"forbid\"\n\n\
         [workspace.lints.clippy]\nall = {{ level = \"warn\", priority = -1 }}\n\
         pedantic = {{ level = \"warn\", priority = -1 }}\nmissing_errors_doc = \"allow\"\n\
         missing_panics_doc = \"allow\"\n# Handlers are `async fn` by contract, whether or not \
         their body awaits.\nunused_async = \"allow\"\n\n\
         # What `sofie build` builds with: about half the binary and less CPU per request.\n\
         [profile.dist]\ninherits = \"release\"\nlto = \"fat\"\ncodegen-units = 1\nstrip = \"symbols\"\n",
        framework_source("sofie-framework"),
        framework_source("sofie-cli"),
    )
}

fn app_manifest(name: &str) -> String {
    format!(
        "[package]\nname = \"{name}\"\nedition.workspace = true\nversion.workspace = true\n\
         publish.workspace = true\n\n[dependencies]\nsofie.workspace = true\n\n\
         [build-dependencies]\nsofie-cli.workspace = true\n\n[lints]\nworkspace = true\n"
    )
}

fn module_manifest(name: &str) -> String {
    format!(
        "[package]\nname = \"{name}\"\nedition.workspace = true\nversion.workspace = true\n\
         publish.workspace = true\n\n[dependencies]\n\
         sofie = {{ workspace = true, features = [\"config\"] }}\n\n[lints]\nworkspace = true\n"
    )
}

fn contract_manifest(name: &str) -> String {
    format!(
        "[package]\nname = \"{}\"\nedition.workspace = true\n\
         version.workspace = true\npublish.workspace = true\n\n[lints]\nworkspace = true\n",
        contract_package(name)
    )
}

fn module_declaration(crate_name: &str) -> String {
    format!(
        "use sofie::{{Module, Tier}};\n\npub const MODULE: Module = Module {{\n    \
         name: \"{crate_name}\",\n    tier: Tier::Core,\n    context: None,\n    settings: &[],\n    \
         ..crate::wiring::GENERATED\n}};\n"
    )
}

const APP_LIBRARY: &str = "use sofie::db::Db;\nuse sofie::prelude::*;\n\nmod wiring;\n\n\
     pub use wiring::{MODULES, worker};\n\npub async fn app(db: Db) -> Result<App> {\n    \
     wiring::app(db)\n}\n";

const MODULE_LIBRARY: &str = "pub mod config;\npub mod module;\nmod wiring;\n\nmod areas {\n    \
     pub mod web {\n        pub mod routes;\n    }\n}\n";

#[cfg(test)]
mod tests {
    use super::{PRELUDE_TYPES, checked_name, checked_package_name, checked_type_name, source_of};

    #[test]
    fn refuses_names_that_become_rust_keywords() {
        for name in ["type", "fn", "self", "crate", "loop", "gen"] {
            let refused = checked_name(name).unwrap_err().to_string();
            assert!(refused.contains("Rust keyword"), "{name}: {refused}");
        }
        for name in [
            "Match", "Type", "Ref", "Impl", "Trait", "Loop", "Move", "Self",
        ] {
            let refused = checked_type_name(name).unwrap_err().to_string();
            assert!(refused.contains("Rust keyword"), "{name}: {refused}");
        }
        assert_eq!(checked_name("typed").unwrap(), "typed");
        assert_eq!(checked_type_name("Matched").unwrap(), "Matched");
    }

    #[test]
    fn refuses_type_names_a_prelude_brings_into_every_file() {
        for name in ["Meta", "Result", "Id", "Read", "Money", "Option", "String"] {
            let refused = checked_type_name(name).unwrap_err().to_string();
            assert!(refused.contains("prelude"), "{name}: {refused}");
        }
        assert_eq!(checked_type_name("Metadata").unwrap(), "Metadata");
    }

    #[test]
    fn the_prelude_types_hold_every_type_the_framework_prelude_exports() {
        let library = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../sofie/src/lib.rs");
        let file = syn::parse_file(&std::fs::read_to_string(library).unwrap()).unwrap();
        let prelude = file
            .items
            .iter()
            .find_map(|item| match item {
                syn::Item::Mod(module) if module.ident == "prelude" => module.content.as_ref(),
                _ => None,
            })
            .expect("sofie has a prelude");
        let mut exported = Vec::new();
        for item in &prelude.1 {
            if let syn::Item::Use(used) = item {
                exported_names(&used.tree, &mut exported);
            }
        }
        let types: Vec<&String> = exported
            .iter()
            .filter(|name| name.starts_with(|first: char| first.is_ascii_uppercase()))
            .collect();

        assert!(types.len() > 20, "{types:?}");
        let missing: Vec<_> = types
            .into_iter()
            .filter(|name| !PRELUDE_TYPES.contains(&name.as_str()))
            .collect();
        assert_eq!(missing, Vec::<&String>::new());
    }

    fn exported_names(tree: &syn::UseTree, names: &mut Vec<String>) {
        match tree {
            syn::UseTree::Path(path) => exported_names(&path.tree, names),
            syn::UseTree::Name(name) => names.push(name.ident.to_string()),
            syn::UseTree::Group(group) => {
                for item in &group.items {
                    exported_names(item, names);
                }
            }
            syn::UseTree::Rename(_) | syn::UseTree::Glob(_) => {}
        }
    }

    #[test]
    fn refuses_a_package_named_after_a_crate_every_app_links() {
        for name in ["sofie", "std", "core", "alloc", "test"] {
            let refused = checked_package_name(name).unwrap_err().to_string();
            assert!(
                refused.contains("names a crate every app links"),
                "{refused}"
            );
        }
        assert_eq!(checked_package_name("shop").unwrap(), "shop");
    }

    #[test]
    fn an_installed_cli_names_the_framework_by_its_commit_and_a_local_one_by_path() {
        let commit = "318950386cebddd99c40181813ae4f4e8da5c6a7";
        assert_eq!(
            source_of("sofie-framework", Some(commit)),
            format!("git = \"https://github.com/sofie-rs/sofie\", rev = \"{commit}\"")
        );
        let local = source_of("sofie-framework", None);
        assert!(local.starts_with("path = \""), "{local}");
        assert!(local.ends_with("crates/sofie\""), "{local}");
        assert!(source_of("sofie-cli", None).ends_with("crates/sofie-cli\""));
    }
}
