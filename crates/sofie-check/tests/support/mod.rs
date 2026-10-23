use std::path::{Path, PathBuf};

use sofie_model::testing::{TemporaryDirectory, aggregate, area, dependency, module};
use sofie_model::{App, Dependency, Module, Route, Source};

pub struct Fixture {
    directory: TemporaryDirectory,
}

impl Fixture {
    pub fn new() -> Self {
        Self {
            directory: TemporaryDirectory::new("sofie-check-rules"),
        }
    }

    pub fn root(&self) -> &Path {
        self.directory.root()
    }

    pub fn write(&self, relative: &str, text: &str) -> PathBuf {
        self.directory.write(relative, text)
    }

    pub fn module(&self, package: &str, manifest: &str, source: &str) -> Module {
        self.write(&format!("modules/{package}/Cargo.toml"), manifest);
        let source = self.write(&format!("modules/{package}/src/lib.rs"), source);
        let mut module = module(package, self.root().join("modules").join(package));
        module.krate.dependencies = manifest_dependencies(manifest);
        module.krate.sources = vec![Source::read(&source).expect("the fixture source parses")];
        module
    }

    pub fn module_with_screens(&self, routes: Vec<Route>, screens: &[(&str, &str)]) -> Module {
        let mut module = module("order", self.root().join("modules/order"));
        let mut web = area(&module, "web", routes);
        web.screens = screens
            .iter()
            .map(|(name, template)| {
                self.write(
                    &format!("modules/order/src/areas/web/screens/{name}.html"),
                    template,
                )
            })
            .collect();
        module.areas.push(web);
        module.code.aggregates.push(aggregate(
            "Order",
            Vec::new(),
            self.root().join("modules/order/src/order.rs"),
        ));
        module
    }

    pub fn app(&self, modules: Vec<Module>) -> App {
        sofie_model::testing::app(self.root(), modules)
    }
}

pub fn manifest_dependencies(manifest: &str) -> Vec<Dependency> {
    manifest
        .lines()
        .skip_while(|line| *line != "[dependencies]")
        .skip(1)
        .filter_map(|line| {
            line.split_once('=')
                .map(|(name, _)| dependency(name.trim()))
        })
        .collect()
}
