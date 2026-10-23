use sofie_db::worker::Worker;
use sofie_db::{Db, File, Migrations};

#[cfg(feature = "engine")]
use crate::screens::Form;
use crate::{App, RouteName};

pub type Mount = fn(App, &'static str) -> App;

/// A module crate's `MODULE`: its manifest (`name`, `tier`, `context`, `settings`)
/// spreading what `sofie gen` writes in its `src/wiring.rs`; a package writes its own.
#[derive(Clone, Copy)]
pub struct Module {
    /// The crate name; also its database schema and its configuration prefix.
    pub name: &'static str,
    pub tier: Tier,
    /// Metadata only, granting no access.
    pub context: Option<&'static str>,
    pub settings: &'static [Setting],
    pub migrations: Option<Migrations>,
    /// A handler's module path is not known at runtime, so the wiring names each route.
    pub route_names: &'static [RouteName],
    /// A key naming another module's screen, `<module>/<area>/<name>`, replaces that screen.
    pub screens: &'static [(&'static str, File)],
    #[cfg(feature = "engine")]
    pub forms: &'static [Form],
    pub areas: &'static [Mount],
    pub jobs: fn(Worker) -> Worker,
}

/// A module depends only on its own tier or lower ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Experience,
    Core,
    Platform,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Setting {
    pub name: &'static str,
    pub default: SettingValue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingValue {
    Switch(bool),
    Number(i64),
    Text(&'static str),
}

impl Module {
    /// A package's: the platform tier, no context, settings or registrations.
    #[must_use]
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            tier: Tier::Platform,
            context: None,
            settings: &[],
            migrations: None,
            route_names: &[],
            screens: &[],
            #[cfg(feature = "engine")]
            forms: &[],
            areas: &[],
            jobs: no_jobs,
        }
    }
}

fn no_jobs(worker: Worker) -> Worker {
    worker
}

#[must_use]
pub fn migrations(modules: &[Module]) -> Vec<Migrations> {
    modules
        .iter()
        .filter_map(|module| module.migrations)
        .collect()
}

#[must_use]
pub fn worker(db: Db, modules: &[Module]) -> Worker {
    modules
        .iter()
        .fold(Worker::new(db), |worker, module| (module.jobs)(worker))
}

impl App {
    #[must_use]
    pub fn modules(mut self, modules: &[Module]) -> Self {
        for module in modules {
            self.names.extend_from_slice(module.route_names);
        }
        modules.iter().fold(self, |app, module| {
            module
                .areas
                .iter()
                .fold(app, |app, mount| mount(app, module.name))
        })
    }
}
