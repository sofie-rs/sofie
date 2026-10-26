//! Test helpers panic: a broken fixture is a broken test.

use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::naming::{contract_package, crate_name, stored_name};
use crate::{
    Aggregate, App, AreaClient, AreaContribution, AreaDefinition, Crate, Dependency, Effects,
    EnumDeclaration, EnumVariant, FieldDeclaration, Function, Manifest, Method, Module, Route,
    Tier, TypeMethod, TypeName,
};

static NEXT_DIRECTORY: AtomicU32 = AtomicU32::new(0);

pub struct TemporaryDirectory {
    root: PathBuf,
}

impl TemporaryDirectory {
    /// Unique per process and call, so tests running in parallel never share one.
    #[must_use]
    pub fn new(prefix: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).expect("the temporary directory is creatable");
        Self { root }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    #[expect(
        clippy::must_use_candidate,
        reason = "most callers only want the file written"
    )]
    pub fn write(&self, relative: &str, text: &str) -> PathBuf {
        let path = self.path(relative);
        let directory = path.parent().expect("a written file lives in a directory");
        fs::create_dir_all(directory).expect("the fixture directory is creatable");
        fs::write(&path, text).expect("the fixture file is writable");
        path
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.root) {
            eprintln!("cannot remove {}: {error}", self.root.display());
        }
    }
}

#[must_use]
pub fn manifest(package: &str, dependencies: &str) -> String {
    format!(
        "[package]\nname = \"{package}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\n{dependencies}"
    )
}

pub fn krate(package: &str, path: impl Into<PathBuf>) -> Crate {
    Crate {
        package: package.to_owned(),
        crate_name: crate_name(package),
        path: path.into(),
        dependencies: Vec::new(),
        sources: Vec::new(),
        tests_and_examples: Vec::new(),
    }
}

#[must_use]
pub fn dependency(package: &str) -> Dependency {
    Dependency {
        package: package.to_owned(),
        crate_name: crate_name(package),
    }
}

pub fn module(package: &str, path: impl Into<PathBuf>) -> Module {
    let krate = krate(package, path);
    let manifest = Manifest {
        name: krate.crate_name.clone(),
        tier: Tier::Core,
        context: None,
        settings: Vec::new(),
        file: krate.path.join("src/module.rs"),
        line: 3,
        declared: true,
    };
    Module {
        krate,
        manifest: Some(manifest),
        config: None,
        contract: None,
        defined_areas: Vec::new(),
        contract_declares_gen: false,
        contract_exports: Vec::new(),
        contract_traits: Vec::new(),
        migrations: Vec::new(),
        sqlite_migrations: Vec::new(),
        areas: Vec::new(),
        listeners: Vec::new(),
        schedules: Vec::new(),
        events: Vec::new(),
        module_events: Vec::new(),
        jobs: Vec::new(),
        tasks: Vec::new(),
        implementations: Vec::new(),
        extensions: Vec::new(),
        gateways: Vec::new(),
        islands: Vec::new(),
        components: Vec::new(),
        languages: Vec::new(),
        mails: Vec::new(),
        plans: None,
        private_items: Vec::new(),
        code: crate::Code::default(),
    }
}

#[must_use]
pub fn area(module: &Module, name: &str, routes: Vec<Route>) -> AreaContribution {
    AreaContribution {
        area: name.to_owned(),
        routes_file: module
            .krate
            .path
            .join(format!("src/areas/{name}/routes.rs")),
        declared: true,
        routes,
        screens: Vec::new(),
        overrides: Vec::new(),
        menu: None,
    }
}

pub fn define_area(module: &mut Module, name: &str) {
    let path = module.krate.path.join("contract");
    let contract = krate(&contract_package(&module.krate.package), &path);
    module.defined_areas.push(AreaDefinition {
        name: name.to_owned(),
        type_path: vec![
            contract.crate_name.clone(),
            "area".to_owned(),
            "Member".to_owned(),
        ],
        prefix: Some("/account".to_owned()),
        client: AreaClient::Html,
        principal: vec!["Account".to_owned()],
        implements_member_area: true,
        opens_sign_up: false,
        file: path.join("src/area.rs"),
        line: 7,
    });
    module.contract = Some(contract);
}

#[must_use]
pub fn route(method: Method, path: &str, handler: &str, line: usize) -> Route {
    let handler = handler.split("::").map(str::to_owned).collect();
    crate::routes::new_route(method, path.to_owned(), handler, line)
}

pub fn app(root: impl Into<PathBuf>, modules: Vec<Module>) -> App {
    let root = root.into();
    let mut shop = krate("shop", root.join("app"));
    shop.dependencies = modules
        .iter()
        .flat_map(|module| std::iter::once(&module.krate).chain(&module.contract))
        .map(|krate| dependency(&krate.package))
        .collect();
    App {
        root,
        app: shop,
        modules,
        identity: None,
        package_events: Vec::new(),
        packages: Vec::new(),
        split: crate::Split::default(),
        structure_ceilings: false,
    }
}

#[must_use]
pub fn type_name(written: &str) -> TypeName {
    TypeName::of(&syn::parse_str(written).expect("a fixture type parses"))
}

#[must_use]
pub fn field(name: &str, ty: &str, line: usize) -> FieldDeclaration {
    FieldDeclaration {
        name: name.to_owned(),
        ty: type_name(ty),
        constraints: Vec::new(),
        bounds: Vec::new(),
        line,
    }
}

pub fn function(path: &str, file: impl Into<PathBuf>, line: usize) -> Function {
    Function {
        path: path.split("::").map(str::to_owned).collect(),
        arguments: Vec::new(),
        output: None,
        renders: Vec::new(),
        doc: None,
        effects: Effects::default(),
        file: file.into(),
        line,
    }
}

pub fn method(owner: &str, name: &str, file: impl Into<PathBuf>, line: usize) -> TypeMethod {
    TypeMethod {
        owner: owner.to_owned(),
        name: name.to_owned(),
        implements: None,
        doc: None,
        effects: Effects::default(),
        file: file.into(),
        line,
    }
}

pub fn aggregate(name: &str, fields: Vec<FieldDeclaration>, file: impl Into<PathBuf>) -> Aggregate {
    Aggregate {
        name: name.to_owned(),
        path: vec![name.to_owned()],
        mirror: None,
        fields,
        children: Vec::new(),
        behaviors: Vec::new(),
        exposed: Vec::new(),
        owner: None,
        owner_on_struct: false,
        doc: None,
        file: file.into(),
        line: 1,
    }
}

#[must_use]
pub fn enumeration(name: &str, variants: &[(&str, &[&str])]) -> EnumDeclaration {
    EnumDeclaration {
        name: name.to_owned(),
        path: vec![name.to_owned()],
        stored_names: variants
            .iter()
            .map(|(variant, _)| stored_name(variant))
            .collect(),
        variants: variants
            .iter()
            .map(|(variant, to)| EnumVariant {
                name: (*variant).to_owned(),
                to: to.iter().map(|target| (*target).to_owned()).collect(),
                tone: None,
            })
            .collect(),
    }
}
