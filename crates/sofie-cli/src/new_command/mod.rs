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
mod topic;
mod workspace;

use std::path::{Path, PathBuf};

use sofie_model::naming::{contract_package, crate_name, relative_path, snake_case};
use sofie_model::{App, Module};

use crate::fix_command::generated_export_lines;
use crate::gen_command::{GenError, GenMode, GenOutcome, gen_app, regenerated};
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
        /// For an organization area: the member area listing and entering organizations.
        member: Option<String>,
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
    /// A `sofie-notify` topic named `<module>.<name>`, in the module's `src/topics.rs`.
    Topic {
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

/// Inside a cargo workspace the app's crates join it; elsewhere the app is a workspace of its own.
pub fn new_app(parent: &Path, name: &str) -> Result<Vec<GenOutcome>, GenError> {
    let name = checked_app_name(name)?;
    let root = parent.join(name);
    refuse_existing(&root)?;
    let mut journal = Journal::default();
    let written = match workspace::owning_workspace(parent)? {
        Some(workspace) => write_member_app(&mut journal, parent, name, &workspace),
        None => write_standalone_app(&mut journal, &root, name),
    }
    .and_then(|()| gen_app(&root, GenMode::Write));
    journal.finish(written)
}

fn write_standalone_app(journal: &mut Journal, root: &Path, name: &str) -> Result<(), GenError> {
    let workspace = workspace_manifest();
    let files = [
        ("Cargo.toml", workspace.clone()),
        (".gitignore", "/target\n/.env\n".to_owned()),
    ];
    write_all(journal, root, &files)?;
    write_all(journal, root, &app_files(name, &workspace))
}

fn write_member_app(
    journal: &mut Journal,
    parent: &Path,
    name: &str,
    workspace: &Path,
) -> Result<(), GenError> {
    let prefix = workspace::member_prefix(workspace, parent)? + name;
    let text = read(workspace)?;
    let members = [format!("{prefix}/app"), format!("{prefix}/modules/*")];
    let joined = workspace::joined(workspace, &text, &members)?;
    journal.write(workspace, &joined)?;
    write_all(journal, &parent.join(name), &app_files(name, &joined))
}

fn app_files(name: &str, workspace: &str) -> Vec<(String, String)> {
    let [build, library, main] = app_sources(&crate_name(name));
    vec![
        ("app/Cargo.toml".to_owned(), app_manifest(name, workspace)),
        (build.0.to_owned(), build.1),
        (library.0.to_owned(), library.1),
        (main.0.to_owned(), main.1),
        (format!("modules/{KEEP}"), String::new()),
    ]
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
    let workspace = workspace::owning_workspace(root)?;
    let workspace_text = match &workspace {
        Some(manifest) => read(manifest)?,
        None => workspace_manifest(),
    };
    let files = [
        ("Cargo.toml", module_manifest(name, &workspace_text)),
        (
            "contract/Cargo.toml",
            contract_manifest(name, &workspace_text),
        ),
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
        .and_then(|()| include_contracts(&mut journal, workspace.as_deref(), root))
        .and_then(|()| {
            let manifest = root.join("app").join("Cargo.toml");
            let dependency = path_dependency(name, &format!("../modules/{name}"));
            journal.write(
                &manifest,
                with_dependency_line(&read(&manifest)?, &dependency),
            )
        })
        .and_then(|()| regenerated(root, GenMode::Write));
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
            member,
        } => area::new_area(edits, app, module, name, *access, member.as_deref()),
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
        NewCode::Topic { module, name } => {
            topic::new_topic(edits, app, find_module(app, module)?, checked_name(name)?)
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
fn include_contracts(
    journal: &mut Journal,
    workspace: Option<&Path>,
    root: &Path,
) -> Result<(), GenError> {
    let Some(workspace) = workspace else {
        return Ok(());
    };
    let prefix = workspace::member_prefix(workspace, root)?;
    let text = read(workspace)?;
    if !text.contains(&format!("\"{prefix}modules/*\"")) {
        return Ok(());
    }
    let contracts = format!("{prefix}modules/*/contract");
    match workspace::with_members(&text, &[contracts]) {
        Some(edited) if edited != text => journal.write(workspace, edited),
        _ => Ok(()),
    }
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

/// An app is a package only, never a Rust path, so it may take `-` between words as crates
/// commonly do; its crate name, with `_`, is what has to be free.
fn checked_app_name(name: &str) -> Result<&str, GenError> {
    let plain = name.starts_with(|first: char| first.is_ascii_lowercase())
        && name.split('-').all(|word| {
            !word.is_empty()
                && word.chars().all(|character| {
                    character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
                })
        });
    if !plain {
        return Err(GenError::Usage(format!(
            "`{name}` is not a name `sofie new app` takes: start with a lowercase letter and use \
             only lowercase letters, digits, `_` and single `-` between words"
        )));
    }
    checked_package_name(&crate_name(name))?;
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
    files: &[(impl AsRef<Path>, String)],
) -> Result<(), GenError> {
    for (relative, text) in files {
        journal.write(&directory.join(relative), text)?;
    }
    Ok(())
}

/// Where this binary was built from, which decides where a new app finds the framework.
enum Origin {
    Registry,
    Commit(&'static str),
    Checkout,
}

impl Origin {
    fn of_this_binary() -> Self {
        if option_env!("SOFIE_CLI_REGISTRY").is_some() {
            return Self::Registry;
        }
        option_env!("SOFIE_CLI_COMMIT").map_or(Self::Checkout, Self::Commit)
    }
}

/// The version a CLI from crates.io was published as, the commit `cargo install --git` built
/// it from, or this checkout by path, so the app and the CLI always match.
pub(super) fn framework_source(package: &str) -> String {
    source_of(package, &Origin::of_this_binary())
}

fn source_of(package: &str, origin: &Origin) -> String {
    match origin {
        Origin::Registry => format!("version = \"{}\"", env!("CARGO_PKG_VERSION")),
        Origin::Commit(commit) => format!(
            "git = \"{}\", rev = \"{commit}\"",
            env!("CARGO_PKG_REPOSITORY")
        ),
        Origin::Checkout => checkout_path(package),
    }
}

/// As `framework_source`, but a checkout named by a path relative to `workspace`.
fn framework_source_in(package: &str, workspace: &Path) -> String {
    let origin = Origin::of_this_binary();
    let relative = matches!(origin, Origin::Checkout)
        .then(|| relative_path(workspace, &checkout_directory(package)))
        .flatten();
    relative.map_or_else(
        || source_of(package, &origin),
        |path| format!("path = \"{}\"", toml_escaped(&path)),
    )
}

fn checkout_path(package: &str) -> String {
    let path = checkout_directory(package).display().to_string();
    format!("path = \"{}\"", toml_escaped(&path))
}

fn checkout_directory(package: &str) -> PathBuf {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map_or_default(Path::to_path_buf);
    let directory = if package == "sofie-framework" {
        "sofie"
    } else {
        package
    };
    crates.join(directory)
}

/// The framework is `sofie` to every app, whatever its package is called.
fn framework_declaration(package: &str, source: &str) -> String {
    if package == "sofie-framework" {
        return format!("sofie = {{ package = \"sofie-framework\", {source} }}");
    }
    format!("{package} = {{ {source} }}")
}

fn workspace_manifest() -> String {
    let package: Vec<String> = workspace::PACKAGE_DEFAULTS
        .iter()
        .map(|(key, value)| format!("{key} = {value}\n"))
        .collect();
    let package = package.concat();
    format!(
        "[workspace]\nresolver = \"3\"\nmembers = [\"app\", \"modules/*\"]\n\n\
         [workspace.package]\n{package}\n\
         [workspace.dependencies]\n{}\n{}\n\n\
         [workspace.lints.rust]\nunsafe_code = \"forbid\"\n\n\
         [workspace.lints.clippy]\nall = {{ level = \"warn\", priority = -1 }}\n\
         pedantic = {{ level = \"warn\", priority = -1 }}\nmissing_errors_doc = \"allow\"\n\
         missing_panics_doc = \"allow\"\n# Handlers are `async fn` by contract, whether or not \
         their body awaits.\nunused_async = \"allow\"\n\n\
         # What `sofie build` builds with: about half the binary and less CPU per request.\n\
         [profile.dist]\ninherits = \"release\"\nlto = \"fat\"\ncodegen-units = 1\nstrip = \"symbols\"\n",
        framework_declaration("sofie-framework", &framework_source("sofie-framework")),
        framework_declaration("sofie-cli", &framework_source("sofie-cli")),
    )
}

fn app_manifest(name: &str, workspace: &str) -> String {
    format!(
        "{}\n[dependencies]\nsofie.workspace = true\n\n\
         [build-dependencies]\nsofie-cli.workspace = true\n{}",
        workspace::package_section(name, workspace),
        workspace::lints_section(workspace)
    )
}

fn module_manifest(name: &str, workspace: &str) -> String {
    format!(
        "{}\n[dependencies]\nsofie = {{ workspace = true, features = [\"config\"] }}\n{}",
        workspace::package_section(name, workspace),
        workspace::lints_section(workspace)
    )
}

fn contract_manifest(name: &str, workspace: &str) -> String {
    format!(
        "{}{}",
        workspace::package_section(&contract_package(name), workspace),
        workspace::lints_section(workspace)
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
    use super::{
        Origin, PRELUDE_TYPES, checked_app_name, checked_name, checked_package_name,
        checked_type_name, source_of,
    };

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
    fn an_app_name_takes_single_hyphens_between_words() {
        assert_eq!(
            checked_app_name("restarts-pilot").unwrap(),
            "restarts-pilot"
        );
        assert_eq!(checked_app_name("shop_2").unwrap(), "shop_2");
        for name in ["-shop", "shop-", "re--start", "Shop", "2shop", "shop.app"] {
            let refused = checked_app_name(name).unwrap_err().to_string();
            assert!(refused.contains("single `-`"), "{name}: {refused}");
        }
        assert!(checked_app_name("proc-macro").is_err());
    }

    #[test]
    fn the_framework_is_named_by_version_commit_or_path_after_where_the_cli_came_from() {
        assert_eq!(
            source_of("sofie-framework", &Origin::Registry),
            format!("version = \"{}\"", env!("CARGO_PKG_VERSION"))
        );
        let commit = "318950386cebddd99c40181813ae4f4e8da5c6a7";
        assert_eq!(
            source_of("sofie-framework", &Origin::Commit(commit)),
            format!("git = \"https://github.com/sofie-rs/sofie\", rev = \"{commit}\"")
        );
        let local = source_of("sofie-framework", &Origin::Checkout);
        assert!(local.starts_with("path = \""), "{local}");
        assert!(local.ends_with("crates/sofie\""), "{local}");
        assert!(source_of("sofie-cli", &Origin::Checkout).ends_with("crates/sofie-cli\""));
    }
}
