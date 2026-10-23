use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use syn::ext::IdentExt;

use crate::config::read_config;
use crate::gateways::gateways;
use crate::identity::IDENTITY_CRATE;
use crate::manifest::read_manifest;
use crate::menu::read_menu;
use crate::messages::Found;
use crate::metadata::{Package, Packages};
use crate::naming::{
    BILLING_PACKAGE, CONTENT_PACKAGE, GEN_MODULE, IDENTITY_AREA, IDENTITY_PACKAGE, TENANCY_PACKAGE,
    contract_package, crate_name, file_stem, screen_key, snake_case,
};
use crate::package_routes::{read_module_mounts, read_mounts, read_route_tables};
use crate::provisions::{RootExports, provisions};
use crate::routes::read_routes;
use crate::sources::{ConditionalWiring, module_tree};
use crate::syntax::{parsed_or_read, public_uses};
use crate::targets::target_sources;
use crate::{
    App, AreaContribution, AreaDefinition, ContractTrait, Crate, EventDeclaration, Identity,
    MenuFile, ModelError, Module, RemoteModule, RoutedPackage, ScreenOverride, Source, Split,
};

pub fn load(root: &Path) -> Result<App, ModelError> {
    let app_directory = root.join("app");
    let app_manifest = manifest(&app_directory)?;
    let packages = Packages::read(&app_manifest)?;
    let app_package = packages.at(&app_manifest)?;
    let split = read_split(app_package, &packages, &app_manifest)?;
    let app = read_crate(app_package, &app_directory, ConditionalWiring::Refused)?;
    let modules = module_dirs(&root.join("modules"))?
        .iter()
        .map(|directory| read_module(directory, &packages))
        .collect::<Result<Vec<Module>, _>>()?;
    refuse_second_plans(&modules)?;
    let identity = read_identity(&app, &packages)?;
    let package_events = read_package_events(&app.krate, &modules, &packages)?;
    let routed = read_routed_packages(&app, &packages)?;
    let app = App {
        root: root.to_path_buf(),
        app: app.krate,
        modules,
        identity,
        package_events,
        packages: routed,
        split,
        structure_ceilings: app_package.structure_ceilings,
    };
    if app.has_permitted_areas() {
        app.modules
            .iter()
            .try_for_each(crate::permissions::refuse_colliding_constants)?;
    }
    Ok(app)
}

/// One catalog per app, so every owner's plan is looked up in the same place.
fn refuse_second_plans(modules: &[Module]) -> Result<(), ModelError> {
    let mut declaring = modules.iter().filter_map(|module| module.plans.as_ref());
    match (declaring.next(), declaring.next()) {
        (Some(first), Some(second)) => Err(ModelError::in_file(
            second,
            format!(
                "plans are declared once per app, and {} declares them already",
                first.display()
            ),
        )),
        _ => Ok(()),
    }
}

fn read_split(app: &Package, packages: &Packages, manifest: &Path) -> Result<Split, ModelError> {
    let remote = app
        .remote
        .iter()
        .map(|name| read_remote_module(name, packages, manifest))
        .collect::<Result<_, _>>()?;
    Ok(Split {
        serves: app.serves.clone(),
        remote,
    })
}

fn read_remote_module(
    name: &str,
    packages: &Packages,
    manifest: &Path,
) -> Result<RemoteModule, ModelError> {
    let package = contract_package(name);
    let found = packages.dependency(&package)?.ok_or_else(|| {
        ModelError::in_file(
            manifest,
            format!(
                "`[package.metadata.sofie] remote` names module `{name}`, but the workspace has no \
                 package `{package}`; add the split app to the workspace"
            ),
        )
    })?;
    let contract = read_contract(&found.directory, packages)?;
    let krate = contract.krate.ok_or_else(|| {
        ModelError::in_file(
            &found.directory,
            "the remote module's contract has no manifest",
        )
    })?;
    Ok(RemoteModule {
        name: crate_name(name),
        contract: krate,
        defined_areas: contract.areas,
        exports: contract.exports,
        events: contract
            .events
            .into_iter()
            .map(|event| event.name)
            .collect(),
        traits: contract.traits,
    })
}

pub fn workspace_crates(manifest: &Path) -> Result<Vec<Crate>, ModelError> {
    let packages = Packages::read(manifest)?;
    let mut crates = packages
        .members()
        .map(|package| {
            Ok(read_crate(package, &package.directory, ConditionalWiring::Allowed)?.krate)
        })
        .collect::<Result<Vec<_>, ModelError>>()?;
    crates.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(crates)
}

pub fn dependency_crate(root: &Path, package: &str) -> Result<Option<Crate>, ModelError> {
    let packages = Packages::read(&manifest(&root.join("app"))?)?;
    packages
        .dependency(package)?
        .map(|package| {
            Ok(read_crate(&package, &package.directory, ConditionalWiring::Allowed)?.krate)
        })
        .transpose()
}

struct CrateTree {
    krate: Crate,
    found: Found,
    syntax: BTreeMap<PathBuf, syn::File>,
    module_paths: BTreeMap<PathBuf, Vec<String>>,
}

fn manifest(directory: &Path) -> Result<PathBuf, ModelError> {
    let manifest = directory.join("Cargo.toml");
    if manifest.is_file() {
        return Ok(manifest);
    }
    Err(ModelError::in_file(
        &manifest,
        "missing manifest; an app root holds app/Cargo.toml and modules/<name>/Cargo.toml",
    ))
}

fn read_member(directory: &Path, packages: &Packages) -> Result<CrateTree, ModelError> {
    read_crate(
        packages.at(&manifest(directory)?)?,
        directory,
        ConditionalWiring::Refused,
    )
}

/// `directory` keeps the caller's spelling, so model paths stay relative when the root was.
fn read_crate(
    package: &Package,
    directory: &Path,
    conditional_wiring: ConditionalWiring,
) -> Result<CrateTree, ModelError> {
    let sources = module_tree(directory, conditional_wiring)?;
    let mut tests_and_examples = Vec::new();
    let build_script = directory.join("build.rs");
    if build_script.is_file() {
        tests_and_examples.push(Source::read(&build_script)?);
    }
    tests_and_examples.extend(target_sources(directory)?);
    let krate = Crate {
        package: package.name.clone(),
        crate_name: package.crate_name.clone(),
        path: directory.to_path_buf(),
        dependencies: package.dependencies.clone(),
        sources: sources.files,
        tests_and_examples,
    };
    Ok(CrateTree {
        krate,
        found: sources.found,
        syntax: sources.syntax,
        module_paths: sources.module_paths,
    })
}

fn read_module(directory: &Path, packages: &Packages) -> Result<Module, ModelError> {
    let tree = read_member(directory, packages)?;
    let manifest = read_manifest(directory, &tree.syntax)?;
    let config = read_config(directory, &tree.krate.crate_name, &tree.syntax)?;
    let contract = read_contract(&directory.join("contract"), packages)?;
    let migrations = files_with_extension(&directory.join("migrations"), "sql")?;
    let mails = crate::mails::read_mails(directory, &tree.krate.crate_name)?;
    let deletable: Vec<String> = tree
        .found
        .code
        .aggregates
        .iter()
        .filter(|aggregate| aggregate.soft_deletes())
        .map(|aggregate| snake_case(&aggregate.name))
        .collect();
    let areas = area_contributions(&tree, &deletable)?;
    let contract_name = contract
        .krate
        .as_ref()
        .map(|krate| krate.crate_name.as_str());
    let exports = RootExports::of(tree.syntax.get(&directory.join("src").join("lib.rs")));
    let (implementations, extensions) = provisions(
        &tree.found.impls,
        &tree.found.units,
        &tree.syntax,
        &exports,
        &tree.krate,
        contract_name,
    );
    let mut code = tree.found.code;
    code.telemetry
        .sensitive_values
        .extend(contract.sensitive_values);
    let gateways = gateways(
        tree.found.gateways,
        &tree.found.impls,
        &tree.syntax,
        &exports,
        &tree.krate.crate_name,
    )?;
    for aggregate in &mut code.aggregates {
        aggregate.path = exports.public(&aggregate.path);
    }
    for enumeration in &mut code.enums {
        enumeration.path = exports.public(&enumeration.path);
    }
    let islands = crate::islands::read_islands(directory, &areas)?;
    Ok(Module {
        krate: tree.krate,
        manifest,
        config,
        contract: contract.krate,
        defined_areas: contract.areas,
        contract_declares_gen: contract.declares_gen,
        contract_exports: contract.exports,
        contract_traits: contract.traits,
        migrations,
        sqlite_migrations: files_with_extension(&directory.join("migrations/sqlite"), "sql")?,
        areas,
        listeners: tree.found.listeners,
        schedules: tree.found.schedules,
        events: contract
            .events
            .iter()
            .chain(&tree.found.events)
            .map(|event| event.name.clone())
            .collect(),
        module_events: tree.found.events,
        jobs: tree.found.jobs,
        tasks: tree.found.tasks,
        implementations,
        extensions,
        gateways,
        islands,
        components: files_with_extension(&directory.join("web"), "html")?,
        languages: files_with_extension(&directory.join("lang"), "toml")?,
        mails,
        plans: Some(directory.join("src").join("plans.rs")).filter(|file| file.is_file()),
        private_items: tree.found.private_items,
        code,
    })
}

#[derive(Default)]
struct Contract {
    krate: Option<Crate>,
    areas: Vec<AreaDefinition>,
    declares_gen: bool,
    exports: Vec<String>,
    events: Vec<EventDeclaration>,
    traits: Vec<ContractTrait>,
    sensitive_values: Vec<String>,
}

fn read_contract(directory: &Path, packages: &Packages) -> Result<Contract, ModelError> {
    if !directory.join("Cargo.toml").is_file() {
        return Ok(Contract::default());
    }
    let tree = read_member(directory, packages)?;
    let contract = tree.krate;
    let member_areas = tree.found.member_areas;
    let sign_up_areas = tree.found.sign_up_areas;
    let areas = tree
        .found
        .areas
        .into_iter()
        .map(|mut area| {
            let name = area.type_path.last();
            area.implements_member_area = name.is_some_and(|name| member_areas.contains(name));
            area.opens_sign_up = name.is_some_and(|name| sign_up_areas.contains(name));
            area.type_path.insert(0, contract.crate_name.clone());
            area
        })
        .collect();
    let root = tree.syntax.get(&directory.join("src").join("lib.rs"));
    Ok(Contract {
        krate: Some(contract),
        areas,
        declares_gen: root.is_some_and(declares_gen_module),
        exports: root.map(gen_exports).unwrap_or_default(),
        events: tree.found.events,
        traits: tree.found.contract_traits,
        sensitive_values: tree.found.code.telemetry.sensitive_values,
    })
}

fn declares_gen_module(root: &syn::File) -> bool {
    root.items
        .iter()
        .any(|item| matches!(item, syn::Item::Mod(module) if module.ident.unraw() == GEN_MODULE))
}

fn gen_exports(root: &syn::File) -> Vec<String> {
    let generated = format!("r#{GEN_MODULE}");
    public_uses(&root.items)
        .filter_map(|used| match used.segments.as_slice() {
            [module, name] if *module == generated => Some(name.clone()),
            _ => None,
        })
        .collect()
}

/// Read from the crate itself so no list of identity's screens is kept beside it.
fn read_identity(app: &CrateTree, packages: &Packages) -> Result<Option<Identity>, ModelError> {
    if !app.krate.depends_on(IDENTITY_PACKAGE) {
        return Ok(None);
    }
    let identity = listed_dependency(&app.krate, packages, IDENTITY_PACKAGE)?;
    let screens = files_with_extension(&identity.directory.join("src").join("screens"), "html")?
        .iter()
        .map(|file| {
            screen_key(
                &crate_name(IDENTITY_PACKAGE),
                IDENTITY_AREA,
                &file_stem(file),
            )
        })
        .collect();
    let features = packages.enabled_features(IDENTITY_PACKAGE)?;
    Ok(Some(Identity {
        directory: identity.directory.clone(),
        screens,
        routes: read_route_tables(&identity.directory, &features)?,
        mounts: read_mounts(&app.syntax, IDENTITY_CRATE),
        shipped: crate::shipped::read_shipped(&identity.directory)?,
    }))
}

/// Mounted by the app's code, or by the package's `Module` itself.
fn read_routed_packages(
    app: &CrateTree,
    packages: &Packages,
) -> Result<Vec<RoutedPackage>, ModelError> {
    let mut routed = Vec::new();
    for package in [TENANCY_PACKAGE, CONTENT_PACKAGE] {
        if !app.krate.depends_on(package) {
            continue;
        }
        let found = listed_dependency(&app.krate, packages, package)?;
        let mut mounts = read_mounts(&app.syntax, &found.crate_name);
        for mount in read_module_mounts(&found.directory)? {
            if !mounts.contains(&mount) {
                mounts.push(mount);
            }
        }
        routed.push(RoutedPackage {
            routes: read_route_tables(&found.directory, &packages.enabled_features(package)?)?,
            crate_name: found.crate_name,
            mounts,
        });
    }
    Ok(routed)
}

fn listed_dependency(
    app: &Crate,
    packages: &Packages,
    package: &str,
) -> Result<Package, ModelError> {
    packages.dependency(package)?.ok_or_else(|| {
        ModelError::in_file(
            &app.path.join("Cargo.toml"),
            format!("`cargo metadata` did not list `{package}` among the app's dependencies"),
        )
    })
}

/// A listener may hear what a framework package raises; billing is reached through the facade.
fn read_package_events(
    app: &Crate,
    modules: &[Module],
    packages: &Packages,
) -> Result<Vec<EventDeclaration>, ModelError> {
    let depended_on = |package: &str| {
        app.depends_on(package)
            || modules
                .iter()
                .any(|module| module.krate.depends_on(package))
    };
    let billed = modules.iter().any(|module| module.plans.is_some());
    let mut events = Vec::new();
    for package in [IDENTITY_PACKAGE, TENANCY_PACKAGE, BILLING_PACKAGE] {
        let used = depended_on(package) || (package == BILLING_PACKAGE && billed);
        if !used {
            continue;
        }
        let found = if depended_on(package) {
            listed_dependency(app, packages, package)?
        } else {
            // Reached through the facade's `billing` feature, which a stand-in facade may lack.
            let Some(found) = packages.dependency(package)? else {
                continue;
            };
            found
        };
        events.extend(crate::package_events::read_events(&found.directory)?);
    }
    Ok(events)
}

fn area_contributions(
    tree: &CrateTree,
    deletable: &[String],
) -> Result<Vec<AreaContribution>, ModelError> {
    let areas_dir = tree.krate.path.join("src").join("areas");
    let mut contributions = Vec::new();
    for area_dir in subdirectories(&areas_dir)? {
        let routes_file = area_dir.join("routes.rs");
        if !routes_file.is_file() {
            continue;
        }
        let (area, module_path) = area_module(tree, &routes_file, &area_dir)?;
        if syn::parse_str::<syn::Ident>(&area).is_err() {
            return Err(ModelError::in_file(
                &routes_file,
                format!(
                    "the area folder `{area}` is not a Rust identifier, yet the wiring names its \
                     module after it; rename the folder, such as to `{}`",
                    area.replace(|character: char| !character.is_alphanumeric(), "_")
                ),
            ));
        }
        let routes = read_routes(
            &routes_file,
            parsed_or_read(&tree.syntax, &routes_file)?.as_ref(),
            module_path,
            deletable,
        )?;
        contributions.push(AreaContribution {
            declared: tree.syntax.contains_key(&routes_file),
            routes,
            area,
            routes_file,
            screens: files_with_extension(&area_dir.join("screens"), "html")?,
            overrides: screen_overrides(&area_dir.join("screens"))?,
            menu: menu_file(tree, &area_dir.join("menu.rs"))?,
        });
    }
    Ok(contributions)
}

/// A `#[path]` may give the area's module another name than its folder; the wiring names the module.
fn area_module(
    tree: &CrateTree,
    routes_file: &Path,
    area_dir: &Path,
) -> Result<(String, Vec<String>), ModelError> {
    if let Some(module_path) = tree.module_paths.get(routes_file)
        && let [areas, area, routes] = module_path.as_slice()
        && areas == "areas"
        && routes == "routes"
    {
        return Ok((area.clone(), module_path.clone()));
    }
    let area = directory_name(area_dir)?;
    let module_path = ["areas", &area, "routes"].map(str::to_owned).to_vec();
    Ok((area, module_path))
}

fn menu_file(tree: &CrateTree, file: &Path) -> Result<Option<MenuFile>, ModelError> {
    if !file.is_file() {
        return Ok(None);
    }
    let entries = read_menu(file, parsed_or_read(&tree.syntax, file)?.as_ref())?;
    Ok(Some(MenuFile {
        file: file.to_path_buf(),
        declared: tree.syntax.contains_key(file),
        entries,
    }))
}

fn screen_overrides(screens_dir: &Path) -> Result<Vec<ScreenOverride>, ModelError> {
    let mut overrides = Vec::new();
    for module_dir in subdirectories(screens_dir)? {
        let module = directory_name(&module_dir)?;
        for file in files_with_extension(&module_dir, "html")? {
            overrides.push(ScreenOverride {
                module: module.clone(),
                file,
            });
        }
    }
    Ok(overrides)
}

fn module_dirs(modules_dir: &Path) -> Result<Vec<PathBuf>, ModelError> {
    Ok(subdirectories(modules_dir)?
        .into_iter()
        .filter(|directory| directory.join("Cargo.toml").is_file())
        .collect())
}

pub fn rust_files(directory: &Path) -> Result<Vec<PathBuf>, ModelError> {
    let mut files = Vec::new();
    collect_rust_files(directory, &mut BTreeSet::new(), &mut files)?;
    Ok(files)
}

/// `walked` holds canonical folders, so a symlink back to one is not walked again.
fn collect_rust_files(
    directory: &Path,
    walked: &mut BTreeSet<PathBuf>,
    files: &mut Vec<PathBuf>,
) -> Result<(), ModelError> {
    let canonical = fs::canonicalize(directory).unwrap_or_else(|_| directory.to_path_buf());
    if !walked.insert(canonical) {
        return Ok(());
    }
    for entry in entries(directory, |_| true)? {
        if entry.is_dir() {
            collect_rust_files(&entry, walked, files)?;
        } else if entry.extension().is_some_and(|extension| extension == "rs") {
            files.push(entry);
        }
    }
    Ok(())
}

pub fn subdirectories(directory: &Path) -> Result<Vec<PathBuf>, ModelError> {
    entries(directory, Path::is_dir)
}

pub fn files_with_extension(directory: &Path, extension: &str) -> Result<Vec<PathBuf>, ModelError> {
    entries(directory, |path| {
        path.is_file() && path.extension().is_some_and(|found| found == extension)
    })
}

/// Sorted, so the model does not depend on the order the OS lists files in.
fn entries(directory: &Path, keep: impl Fn(&Path) -> bool) -> Result<Vec<PathBuf>, ModelError> {
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    let unreadable = |error: std::io::Error| {
        ModelError::in_file(directory, format!("cannot list the directory: {error}"))
    };
    let mut kept = Vec::new();
    for entry in fs::read_dir(directory).map_err(unreadable)? {
        let path = entry.map_err(unreadable)?.path();
        if keep(&path) {
            kept.push(path);
        }
    }
    kept.sort();
    Ok(kept)
}

fn directory_name(directory: &Path) -> Result<String, ModelError> {
    directory
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .ok_or_else(|| {
            ModelError::in_file(
                directory,
                "the directory name is not valid UTF-8; rename it",
            )
        })
}
