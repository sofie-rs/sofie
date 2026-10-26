//! One function per name, so the generator and the checks cannot disagree.

use std::path::{Component, Path, PathBuf};

pub use sofie_core::case::snake_case;

use crate::{
    Aggregate, App, AreaContribution, AreaDefinition, Crate, Method, Module, Route, TypeName,
};

pub const WEB_AREA: &str = "web";
const WEB_AREA_TYPE: &str = "sofie::http::Web";

pub const IDENTITY_PACKAGE: &str = "sofie-identity";
pub const TENANCY_PACKAGE: &str = "sofie-tenancy";
pub const CONTENT_PACKAGE: &str = "sofie-content";
pub const BILLING_PACKAGE: &str = "sofie-billing";
pub const IDENTITY_AREA: &str = WEB_AREA;

pub(crate) const GEN_MODULE: &str = "gen";

/// The language the app's names are written in: labels and enum variants are those
/// names humanized, so it needs no file.
pub const SOURCE_LANGUAGE: &str = "en";

pub const QUERY_MODULE: &str = "queries";
const ACTION_MODULE: &str = "actions";

const VISITOR: &str = "Visitor";

const MEMBER_PRINCIPAL: &str = "Account";

/// The `sofie` functions an app's `main` hands its tasks to.
pub const TASK_RUNNERS: [&str; 2] = ["run_with_tasks", "run_with_flags_and_tasks"];

pub const DEFAULT_BEHAVIORS: [&str; 2] = ["Versioned", "Timestamps"];

pub struct BehaviorColumn {
    pub name: &'static str,
    pub sql_type: &'static str,
    pub nullable: bool,
    /// What the behavior stores when a write leaves the column empty, so rows written before it can hold it too.
    pub backfill: Option<&'static str>,
}

const fn column(name: &'static str, sql_type: &'static str) -> BehaviorColumn {
    BehaviorColumn {
        name,
        sql_type,
        nullable: false,
        backfill: None,
    }
}

const fn optional(name: &'static str, sql_type: &'static str) -> BehaviorColumn {
    BehaviorColumn {
        nullable: true,
        ..column(name, sql_type)
    }
}

const fn filled(
    name: &'static str,
    sql_type: &'static str,
    backfill: &'static str,
) -> BehaviorColumn {
    BehaviorColumn {
        backfill: Some(backfill),
        ..column(name, sql_type)
    }
}

/// Must match sofie-db's `Behavior::columns`.
#[must_use]
pub fn behavior_columns(behavior: &str) -> &'static [BehaviorColumn] {
    match behavior {
        "Versioned" => const { &[column("version", "bigint")] },
        "Timestamps" => {
            const {
                &[
                    filled("created_at", "timestamptz", "now()"),
                    filled("updated_at", "timestamptz", "now()"),
                ]
            }
        }
        "SoftDelete" => const { &[optional("deleted_at", "timestamptz")] },
        "PublishLock" => const { &[optional("published_at", "timestamptz")] },
        "Audited" => {
            const {
                &[
                    optional("created_by", "uuid"),
                    optional("updated_by", "uuid"),
                ]
            }
        }
        "Active" => const { &[filled("is_active", "boolean", "true")] },
        "Default" => const { &[filled("is_default", "boolean", "false")] },
        "Slugged" => const { &[column("slug", "text")] },
        "UniqueCode" => const { &[column("code", "text")] },
        "Searched" => const { &[column("search_text", "text")] },
        _ => &[],
    }
}

pub struct StoredColumn {
    /// Appended to the field's name: `total` and `total_currency` for a `Money` field `total`.
    pub suffix: &'static str,
    pub sql_type: &'static str,
}

const fn stored(suffix: &'static str, sql_type: &'static str) -> StoredColumn {
    StoredColumn { suffix, sql_type }
}

/// Must match each value's `Cast::COLUMNS`. `None` for a type stored another way:
/// an `Option`, an enum, a value of the app's own.
#[must_use]
pub fn stored_columns(ty: &TypeName) -> Option<&'static [StoredColumn]> {
    if let ("Secret", [inner]) = (ty.name.as_str(), ty.arguments.as_slice()) {
        return stored_columns(inner);
    }
    if ty.record().is_some() || ty.name == "Uuid" {
        return Some(const { &[stored("", "uuid")] });
    }
    Some(match ty.name.as_str() {
        "bool" => const { &[stored("", "boolean")] },
        "i32" => const { &[stored("", "integer")] },
        "i64" | "Percentage" => const { &[stored("", "bigint")] },
        "f64" => const { &[stored("", "double precision")] },
        "String" | "Email" | "Url" | "Slug" | "Locale" | "Currency" | "TimeZone" | "TimeOfDay"
        | "Weekday" | "PersonName" | "Password" => const { &[stored("", "text")] },
        "Instant" | "Timestamp" => const { &[stored("", "timestamptz")] },
        "Date" => const { &[stored("", "date")] },
        "Money" => const { &[stored("", "bigint"), stored("_currency", "text")] },
        "Measurement" => const { &[stored("", "double precision"), stored("_unit", "text")] },
        "LocalDateTime" => const { &[stored("", "timestamp"), stored("_zone", "text")] },
        "Coordinate" => {
            const {
                &[
                    stored("_latitude", "double precision"),
                    stored("_longitude", "double precision"),
                ]
            }
        }
        "PhoneNumber" => const { &[stored("_code", "bigint"), stored("_number", "text")] },
        _ => return None,
    })
}

/// Must match how `#[derive(Enum)]` stores a variant.
#[must_use]
pub fn stored_name(variant: &str) -> String {
    snake_case(variant).to_uppercase()
}

/// `weekly_news` as `WeeklyNews`.
#[must_use]
pub fn pascal_case(snake: &str) -> String {
    snake
        .split('_')
        .map(|word| {
            let mut letters = word.chars();
            letters.next().map_or_else(String::new, |first| {
                first.to_ascii_uppercase().to_string() + letters.as_str()
            })
        })
        .collect()
}

pub(crate) fn route_name(module: &Module, area: &str, route: &Route) -> String {
    format!(
        "{}.{area}.{}",
        module.krate.crate_name,
        route.handler.last().map_or("", |handler| unraw(handler))
    )
}

/// A name is the identifier as read, so `r#match` is named `match`.
#[must_use]
pub fn unraw(segment: &str) -> &str {
    segment.strip_prefix("r#").unwrap_or(segment)
}

/// A GET never changes state and an action is never a GET, so each lives in its own module.
#[must_use]
pub fn handler_module(method: Method) -> &'static str {
    match method {
        Method::Get => QUERY_MODULE,
        Method::Post | Method::Put | Method::Patch | Method::Delete => ACTION_MODULE,
    }
}

#[must_use]
pub fn contract_package(module_package: &str) -> String {
    format!("{module_package}-contract")
}

#[must_use]
pub fn crate_name(package: &str) -> String {
    package.replace('-', "_")
}

#[must_use]
pub fn screen_key(module: &str, area: &str, name: &str) -> String {
    format!("{module}/{area}/{name}")
}

#[must_use]
pub fn job_name(module: &Module, function: &[String]) -> String {
    let function: Vec<&str> = function.iter().map(|segment| unraw(segment)).collect();
    format!("{}.{}", module.krate.crate_name, function.join("."))
}

#[must_use]
pub fn task_name(module: &Module, function: &[String]) -> String {
    let name = function.last().map_or("", |name| unraw(name));
    format!("{}.{name}", module.krate.crate_name)
}

/// A screen name becomes a file path and must not leave `screens/`; an htmx target names one element, never a selector.
pub(crate) fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
}

#[must_use]
pub fn file_stem(file: &Path) -> String {
    file.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

impl AreaContribution {
    #[must_use]
    pub fn screen_file(&self, name: &str) -> PathBuf {
        self.routes_file
            .with_file_name("screens")
            .join(format!("{name}.html"))
    }
}

impl Crate {
    #[must_use]
    pub fn depends_on(&self, package: &str) -> bool {
        self.dependencies
            .iter()
            .any(|dependency| dependency.package == package)
    }

    #[must_use]
    pub fn gen_file(&self) -> PathBuf {
        self.path.join("src").join(GEN_MODULE).join("mod.rs")
    }

    #[must_use]
    pub fn wiring_file(&self) -> PathBuf {
        self.path.join("src").join("wiring.rs")
    }
}

/// Which generated file names a module's wired item: its own `src/wiring.rs`, so the item
/// may stay private to the crate, or the app's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WiringSide {
    Module,
    App,
}

impl Module {
    /// A gated job runs with the app's started `Flags`, which only the app's wiring holds.
    #[must_use]
    pub fn job_side(&self, function: &[String]) -> WiringSide {
        match self.code.features.gate_on(function) {
            Some(_) => WiringSide::App,
            None => WiringSide::Module,
        }
    }
}

impl AreaDefinition {
    #[must_use]
    pub fn needs_generated_member_area(&self) -> bool {
        !self.implements_member_area
            && (self.is_permitted()
                || self
                    .principal
                    .last()
                    .is_some_and(|name| name == MEMBER_PRINCIPAL))
    }
}

impl KnownArea<'_> {
    #[must_use]
    pub fn is_public(&self) -> bool {
        self.definition.is_none_or(|definition| {
            definition
                .principal
                .last()
                .is_some_and(|name| name == VISITOR)
        })
    }

    #[must_use]
    pub fn prefix(&self) -> Option<&str> {
        match self.definition {
            None => Some(""),
            Some(definition) => definition.prefix.as_deref(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownArea<'app> {
    pub name: &'app str,
    pub type_path: String,
    pub contract: Option<&'app str>,
    pub definition: Option<&'app AreaDefinition>,
}

impl App {
    #[must_use]
    pub fn areas(&self) -> Vec<KnownArea<'_>> {
        let web = KnownArea {
            name: WEB_AREA,
            type_path: WEB_AREA_TYPE.to_owned(),
            contract: None,
            definition: None,
        };
        let local = self.modules.iter().map(|module| {
            let contract = module.contract.as_ref().map(|krate| krate.package.as_str());
            (contract, &module.defined_areas)
        });
        let remote = self.split.remote.iter().map(|module| {
            (
                Some(module.contract.package.as_str()),
                &module.defined_areas,
            )
        });
        let defined = local.chain(remote).flat_map(|(contract, definitions)| {
            definitions.iter().map(move |definition| KnownArea {
                name: &definition.name,
                type_path: definition.type_path.join("::"),
                contract,
                definition: Some(definition),
            })
        });
        std::iter::once(web).chain(defined).collect()
    }

    #[must_use]
    pub fn area(&self, name: &str) -> Option<KnownArea<'_>> {
        self.areas().into_iter().find(|area| area.name == name)
    }

    #[must_use]
    pub fn member_home(&self, area: &AreaDefinition) -> Option<String> {
        let prefix = area.prefix.as_deref()?;
        self.modules_by_name()
            .into_iter()
            .flat_map(|module| &module.areas)
            .filter(|contribution| contribution.area == area.name)
            .flat_map(|contribution| &contribution.routes)
            .find(|route| route.screen.is_some() && !route.path.contains('{'))
            .map(|route| format!("{prefix}{}", route.path))
    }

    #[must_use]
    pub fn depends_on(&self, package: &str) -> bool {
        self.app.depends_on(package)
    }

    #[must_use]
    pub fn wiring_file(&self) -> PathBuf {
        self.app.wiring_file()
    }

    /// From the app crate's root, where `embed!` starts.
    #[must_use]
    pub fn embedded_path(&self, file: &Path) -> Option<String> {
        relative_path(&self.app.path, file)
    }

    #[must_use]
    pub fn route_sites(&self) -> Vec<RouteSite<'_>> {
        let mut sites = Vec::new();
        for module in self.modules_by_name() {
            let mut areas: Vec<&AreaContribution> = module.areas.iter().collect();
            areas.sort_by(|left, right| left.area.cmp(&right.area));
            for area in areas {
                sites.extend(area.routes.iter().map(|route| RouteSite {
                    module,
                    area,
                    route,
                }));
            }
        }
        sites
    }

    /// Generated output must not depend on the order modules were found in.
    fn modules_by_name(&self) -> Vec<&Module> {
        let mut modules: Vec<&Module> = self.modules.iter().collect();
        modules.sort_by(|left, right| left.krate.crate_name.cmp(&right.krate.crate_name));
        modules
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RouteSite<'app> {
    pub module: &'app Module,
    pub area: &'app AreaContribution,
    pub route: &'app Route,
}

impl RouteSite<'_> {
    #[must_use]
    pub fn name(&self) -> String {
        route_name(self.module, &self.area.area, self.route)
    }

    #[must_use]
    pub fn full_path(&self, app: &App) -> Option<String> {
        let area = app.area(&self.area.area)?;
        Some(format!("{}{}", area.prefix()?, self.route.path))
    }
}

/// `orders.count` is `ORDERS_COUNT` in a contract's `keys`.
#[must_use]
pub fn key_constant(key: &str) -> String {
    key.chars()
        .map(|character| match character {
            '.' | '-' => '_',
            other => other.to_ascii_uppercase(),
        })
        .collect()
}

#[must_use]
pub fn path_parameters(path: &str) -> Vec<&str> {
    written_parameters(path)
        .map(|name| name.trim_start_matches('*'))
        .collect()
}

/// `{*rest}` keeps its `*`.
fn written_parameters(path: &str) -> impl Iterator<Item = &str> {
    path.split('/')
        .filter_map(|segment| segment.strip_prefix('{')?.strip_suffix('}'))
}

/// Must match sofie-db's `bind`, which loads the aggregate named like the segment.
#[must_use]
pub fn bound_aggregate<'module>(
    module: &'module Module,
    parameter: &str,
) -> Option<&'module Aggregate> {
    module
        .code
        .aggregates
        .iter()
        .find(|aggregate| snake_case(&aggregate.name) == parameter)
}

/// What a route builder takes for a path parameter: what a handler can read from it.
#[derive(Clone, Debug)]
pub enum PathParameter<'app> {
    /// Loaded by sofie-db's `bind`.
    Aggregate(&'app Aggregate),
    /// A parent's id held by a field, which children and listings are kept within.
    Reference {
        module: &'app Module,
        aggregate: &'app Aggregate,
    },
    /// Another field's value, which a listing filters by, typed as a contract names it.
    Value(String),
    /// `{*rest}`, the rest of the path with its slashes.
    Rest,
}

/// `None` for a parameter no handler can read.
#[must_use]
pub fn route_parameters<'app, 'path>(
    app: &'app App,
    module: &'app Module,
    path: &'path str,
) -> Vec<(&'path str, Option<PathParameter<'app>>)> {
    written_parameters(path)
        .map(|name| match name.strip_prefix('*') {
            Some(rest) => (rest, Some(PathParameter::Rest)),
            None => (name, path_parameter(app, module, name)),
        })
        .collect()
}

fn path_parameter<'app>(
    app: &'app App,
    module: &'app Module,
    parameter: &str,
) -> Option<PathParameter<'app>> {
    if let Some(aggregate) = bound_aggregate(module, parameter) {
        return Some(PathParameter::Aggregate(aggregate));
    }
    let field = module
        .code
        .aggregates
        .iter()
        .flat_map(|aggregate| &aggregate.fields)
        .find(|field| field.name == parameter)?;
    let Some(record) = field.ty.record() else {
        return module
            .code
            .contract_type(&field.ty)
            .map(PathParameter::Value);
    };
    let referenced = app.modules.iter().find_map(|module| {
        let aggregate = module
            .code
            .aggregates
            .iter()
            .find(|aggregate| aggregate.name == record)?;
        Some(PathParameter::Reference { module, aggregate })
    });
    Some(referenced.unwrap_or_else(|| PathParameter::Value("sofie::db::Uuid".to_owned())))
}

/// Joined with `/`, as Rust paths in `concat!` need on every platform.
#[must_use]
pub fn relative_path(from: &Path, to: &Path) -> Option<String> {
    let from: Vec<Component> = from.components().collect();
    let to: Vec<Component> = to.components().collect();
    let common = from
        .iter()
        .zip(&to)
        .take_while(|(left, right)| left == right)
        .count();
    let mut parts = Vec::new();
    for component in &from[common..] {
        if !matches!(component, Component::Normal(_)) {
            return None;
        }
        parts.push("..");
    }
    for component in &to[common..] {
        parts.push(component.as_os_str().to_str()?);
    }
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{is_plain_name, relative_path};

    #[test]
    fn relative_paths_climb_out_of_the_app_crate() {
        let relative = relative_path(
            Path::new("./app/src"),
            Path::new("./modules/order/migrations/0001_a.sql"),
        );

        assert_eq!(
            relative.as_deref(),
            Some("../../modules/order/migrations/0001_a.sql")
        );
    }

    #[test]
    fn a_plain_name_holds_letters_digits_underscores_and_dashes() {
        assert!(is_plain_name("orders_table-2"));
        for name in ["", "../x", "#orders", "a b", "a.b"] {
            assert!(!is_plain_name(name), "{name}");
        }
    }
}
