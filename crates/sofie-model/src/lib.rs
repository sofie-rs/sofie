//! The app model (principle 8): read once from source and shared by every generator and check so they cannot disagree.

mod areas;
mod code;
mod config;
mod constraints;
mod contract_traits;
mod cross_owner;
mod declarations;
mod effects;
mod entitlements;
mod error;
mod flags;
mod gateways;
mod gating;
mod handlers;
mod identity;
mod islands;
mod load;
mod mails;
mod manifest;
mod menu;
mod messages;
mod metadata;
pub mod naming;
mod package_events;
mod package_routes;
pub mod permissions;
mod posts;
mod provisions;
mod references;
mod routes;
mod shipped;
mod sign_in;
mod sources;
pub mod syntax;
mod targets;
mod telemetry;
#[cfg(feature = "testing")]
pub mod testing;
pub mod usage;

use std::path::PathBuf;

use usage::{Mentions, PublicItem};

pub use code::{
    Aggregate, Code, EnumDeclaration, EnumVariant, FieldDeclaration, Function, InputDeclaration,
    KeyLiteral, RawSql, ReadModel, ScreenRender, TypeMethod, TypeName,
};
pub use config::{ConfigKey, DATABASE_URL, KeyKind, ModuleConfig, config_type_name};
pub use contract_traits::{ContractMethod, ContractTrait};
pub use cross_owner::CrossOwnerCheck;
pub use declarations::{ModuleDeclaration, module_declaration};
pub use effects::{Call, Effects, MethodCall, Queued, Raised};
pub use entitlements::{EntitlementDeclaration, Entitlements};
pub use error::ModelError;
pub use flags::{FeatureCode, FeatureDeclaration, FeatureGate};
pub use gateways::{Gateway, GatewayAdapter};
pub use islands::{Island, IslandBundle, PropsDeclaration, is_island_name, island_bundles};
pub use load::{
    dependency_crate, files_with_extension, load, rust_files, subdirectories, workspace_crates,
};
pub use manifest::{MANIFEST_CONSTANT, Manifest, Tier, manifest_file};
pub use messages::interval_seconds;
pub use metadata::{framework_features, workspace_root};
pub use naming::{KnownArea, WiringSide};
pub use package_routes::{ListedRoute, PackageRoutes, RouteMount, RoutedPackage, ServedRoute};
pub use routes::check_route_path;
pub use shipped::{MountedScreen, ScreenRoute, ShippedScreen};
pub use sign_in::SignInPaths;
pub use telemetry::{ProductEvent, TelemetryCode};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct App {
    pub root: PathBuf,
    pub app: Crate,
    pub modules: Vec<Module>,
    pub identity: Option<Identity>,
    /// Raised by the framework packages the app uses, which its listeners may hear.
    pub package_events: Vec<EventDeclaration>,
    /// The framework packages other than sofie-identity whose routes the app serves.
    pub packages: Vec<RoutedPackage>,
    pub split: Split,
    /// Set by `[package.metadata.sofie] structure-ceilings = true`.
    pub structure_ceilings: bool,
}

/// What `sofie split` wrote into the app's `[package.metadata.sofie]`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Split {
    /// This app's modules whose contracts, lookups and events it serves to other apps.
    pub serves: Vec<String>,
    pub remote: Vec<RemoteModule>,
}

/// A module split into another app, known here only through its contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteModule {
    pub name: String,
    pub contract: Crate,
    pub defined_areas: Vec<AreaDefinition>,
    pub exports: Vec<String>,
    pub events: Vec<String>,
    pub traits: Vec<ContractTrait>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    pub directory: PathBuf,
    pub screens: Vec<String>,
    pub routes: Vec<ListedRoute>,
    pub mounts: Vec<RouteMount>,
    pub shipped: Vec<ShippedScreen>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Crate {
    pub package: String,
    pub crate_name: String,
    pub path: PathBuf,
    pub dependencies: Vec<Dependency>,
    pub sources: Vec<Source>,
    pub tests_and_examples: Vec<Source>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dependency {
    pub package: String,
    pub crate_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub file: PathBuf,
    pub paths: Vec<PathReference>,
    pub macros: Vec<MacroCall>,
    pub mentions: Mentions,
    pub public_items: Vec<PublicItem>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathReference {
    pub segments: Vec<String>,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MacroCall {
    pub path: Vec<String>,
    pub first_string: Option<String>,
    pub line: usize,
}

impl MacroCall {
    #[must_use]
    pub fn name(&self) -> &str {
        self.path.last().map_or("", String::as_str)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Module {
    pub krate: Crate,
    pub manifest: Option<Manifest>,
    pub config: Option<ModuleConfig>,
    pub contract: Option<Crate>,
    pub defined_areas: Vec<AreaDefinition>,
    pub contract_declares_gen: bool,
    pub contract_exports: Vec<String>,
    pub contract_traits: Vec<ContractTrait>,
    pub migrations: Vec<PathBuf>,
    /// `migrations/sqlite/*.sql`, run on SQLite instead of the files of the same name.
    pub sqlite_migrations: Vec<PathBuf>,
    pub areas: Vec<AreaContribution>,
    pub listeners: Vec<Listener>,
    pub schedules: Vec<Schedule>,
    pub events: Vec<String>,
    pub module_events: Vec<EventDeclaration>,
    pub jobs: Vec<Job>,
    pub tasks: Vec<Task>,
    pub implementations: Vec<Provision>,
    pub extensions: Vec<Provision>,
    pub gateways: Vec<Gateway>,
    pub islands: Vec<Island>,
    /// `web/<Name>.html`, written in any screen as `<Name … />`.
    pub components: Vec<PathBuf>,
    /// `lang/<language>.toml`.
    pub languages: Vec<PathBuf>,
    pub mails: Vec<MailTemplate>,
    /// `src/plans.rs`, whose `PLANS` sofie-billing is installed with.
    pub plans: Option<PathBuf>,
    pub private_items: Vec<PrivateItem>,
    pub code: Code,
}

/// A `mod` or a wired function that is not `pub`: the wiring cannot name it or what is below it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrivateItem {
    pub path: Vec<String>,
    pub file: PathBuf,
    pub line: usize,
    /// `pub(crate)`: the module's own wiring reaches it, the app's does not.
    pub crate_visible: bool,
}

/// `src/mails/<name>.html`, keyed `<module>/<name>`; one under
/// `src/mails/<crate>/` is keyed `<crate>/<name>`, replacing that crate's own mail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MailTemplate {
    pub key: String,
    pub file: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AreaDefinition {
    pub name: String,
    pub type_path: Vec<String>,
    pub prefix: Option<String>,
    pub client: AreaClient,
    pub principal: Vec<String>,
    pub implements_member_area: bool,
    /// It implements sofie-identity's `SignUp`, so a generated `MemberArea` opens sign-up.
    pub opens_sign_up: bool,
    pub file: PathBuf,
    pub line: usize,
}

/// What an area's pages are: HTML over the wire, or a client framework's components compiled
/// from the same templates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AreaClient {
    #[default]
    Html,
    Svelte,
    React,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub file: PathBuf,
    pub line: usize,
    pub function: Vec<String>,
}

/// Named `<module>.<function>` wherever under `tasks` it lives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    pub file: PathBuf,
    pub line: usize,
    pub function: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Provision {
    pub contract: Vec<String>,
    pub ty: Vec<String>,
    /// A unit struct is built as written; any other type from the database through its `From<Db>`.
    pub unit: bool,
    pub file: PathBuf,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventDeclaration {
    pub name: String,
    pub file: PathBuf,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listener {
    pub file: PathBuf,
    pub line: usize,
    pub function: Vec<String>,
    pub event: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Schedule {
    pub file: PathBuf,
    pub line: usize,
    pub function: Vec<String>,
    pub cadence: Cadence,
    pub each: Option<Vec<String>>,
}

/// Written as `every 1h`, `daily at 03:00 Europe/Istanbul` or
/// `weekly on monday at 03:00 Europe/Istanbul`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cadence {
    /// As written, which the schedule-interval rule reads.
    Interval(String),
    WallClock(WallClock),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WallClock {
    pub daily_at: String,
    pub weekly_on: Option<String>,
    pub zone: String,
}

impl std::fmt::Display for Cadence {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Interval(interval) => write!(formatter, "every {interval}"),
            Self::WallClock(WallClock {
                daily_at,
                weekly_on: None,
                zone,
            }) => write!(formatter, "daily at {daily_at} {zone}"),
            Self::WallClock(WallClock {
                daily_at,
                weekly_on: Some(weekday),
                zone,
            }) => write!(formatter, "weekly on {weekday} at {daily_at} {zone}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AreaContribution {
    pub area: String,
    pub routes_file: PathBuf,
    /// An unreachable file would be silently left out of the build.
    pub declared: bool,
    pub routes: Vec<Route>,
    pub screens: Vec<PathBuf>,
    pub overrides: Vec<ScreenOverride>,
    pub menu: Option<MenuFile>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuFile {
    pub file: PathBuf,
    pub declared: bool,
    pub entries: Vec<MenuEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuEntry {
    pub label: String,
    pub feature: Option<String>,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenOverride {
    pub module: String,
    pub file: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl Method {
    const ALL: [Self; 5] = [Self::Get, Self::Post, Self::Put, Self::Patch, Self::Delete];

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }

    #[must_use]
    pub fn from_routes_method(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|method| method.name().to_ascii_lowercase() == name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    pub method: Method,
    pub path: String,
    pub handler: Vec<String>,
    pub screen: Option<String>,
    pub island: Option<String>,
    pub then: Option<Then>,
    pub modifiers: Vec<Modifier>,
    pub webhook: Option<String>,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Modifier {
    Throttle(String),
    Signed,
    Idempotent,
    BodyLimit(String),
    Feature(String),
    /// The entitlement a principal's plan must hold, as written.
    Entitled(String),
    /// A screen's fragment refetched when what its loader read changes.
    Live(String),
    /// How long a screen's loaded reply is kept, as written.
    Cached(String),
    /// Queues its `#[job]` action instead of running it.
    Queued,
    /// A page answering its loader's text as this media type, as written.
    Document(String),
    /// The largest file a form may upload, as written.
    FileLimit(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Then {
    Screen {
        screen: String,
        fragment: String,
    },
    Redirect(String),
    /// To the `https://` location the action replies.
    Follow,
    /// To the path, unless the action replies a location of this app.
    RedirectOrFollow(String),
}
