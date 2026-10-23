mod actions;
mod allowances;
mod bindings;
mod clients;
mod clock;
mod constraints;
mod cross_module;
mod database;
mod dead_code;
mod entitlements;
mod environment;
mod exposed;
mod flags;
mod fragments;
mod islands;
mod locking;
mod mails;
mod manifest;
mod members;
mod messages;
mod migration;
mod module_manifest;
mod names;
mod outgoing;
mod own_contract;
mod raw_sql;
mod registrations;
mod remote;
mod routes;
mod safe_methods;
mod sources;
mod tasks;
mod tiers;
mod visibility;
mod wiring;

use std::fmt;
use std::path::{Path, PathBuf};

use sofie_model::App;

use crate::screens::Screens;

pub use allowances::{ALLOW_FILE, AllowList};
pub use clients::check_client_feature;
pub use dead_code::{MAX_UNUSED_PUBLIC_RATIO, check_dead_code};
pub use flags::check_flag_ages;
pub use migration::Schema as MigratedSchema;
pub use names::missing_gen_exports;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Rule {
    ActionInput,
    AppDependency,
    AreaDefinition,
    AreaPrefix,
    BehaviorColumn,
    ClientFeature,
    CommentDensity,
    Config,
    CrossModuleSql,
    DatabaseDriver,
    EmbeddedPath,
    Entitlement,
    Environment,
    EventInContract,
    ExposedField,
    FeatureFlag,
    FlagAge,
    Island,
    IslandProps,
    GodType,
    JobName,
    ListenerEvent,
    MailTemplate,
    MemberHome,
    MigrationLock,
    MirrorField,
    MissingScreen,
    ModuleDependency,
    ModuleManifest,
    Outgoing,
    OwnContract,
    PlatformListener,
    QueuedRoute,
    RawSql,
    RemoteCall,
    RemoteDependency,
    RouteConflict,
    RouteKind,
    RouteName,
    ScheduleEach,
    ScheduleInterval,
    ScheduleZone,
    SchemaConstraint,
    ScreenBinding,
    ScreenOverride,
    ScreenTemplate,
    StaleAllowance,
    StaleDocs,
    StaleWiring,
    SystemTime,
    TaskRunner,
    ThenScreen,
    ThenScreenFragment,
    TierDirection,
    UndeclaredGen,
    UndeclaredRoutes,
    UnknownArea,
    UnusedDependency,
    UnusedItem,
    UnusedRatio,
    Visibility,
    WritingGet,
}

impl Rule {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::ActionInput => "action-input",
            Self::AppDependency => "app-dependency",
            Self::AreaDefinition => "area-definition",
            Self::AreaPrefix => "area-prefix",
            Self::BehaviorColumn => "behavior-column",
            Self::ClientFeature => "client-feature",
            Self::CommentDensity => "comment-density",
            Self::Config => "config",
            Self::CrossModuleSql => "cross-module-sql",
            Self::DatabaseDriver => "database-driver",
            Self::EmbeddedPath => "embedded-path",
            Self::Entitlement => "entitlement",
            Self::Environment => "environment",
            Self::EventInContract => "event-in-contract",
            Self::ExposedField => "exposed-field",
            Self::FeatureFlag => "feature-flag",
            Self::FlagAge => "flag-age",
            Self::GodType => "god-type",
            Self::JobName => "job-name",
            Self::Island => "island",
            Self::IslandProps => "island-props",
            Self::ListenerEvent => "listener-event",
            Self::MailTemplate => "mail-template",
            Self::MemberHome => "member-home",
            Self::MigrationLock => "migration-lock",
            Self::MirrorField => "mirror-field",
            Self::MissingScreen => "missing-screen",
            Self::ModuleDependency => "module-dependency",
            Self::ModuleManifest => "module-manifest",
            Self::Outgoing => "outgoing",
            Self::OwnContract => "own-contract",
            Self::PlatformListener => "platform-listener",
            Self::QueuedRoute => "queued-route",
            Self::RawSql => "raw-sql-in-queries-only",
            Self::RemoteCall => "remote-call",
            Self::RemoteDependency => "remote-dependency",
            Self::RouteConflict => "route-conflict",
            Self::RouteKind => "route-kind",
            Self::RouteName => "route-name",
            Self::ScreenBinding => "screen-binding",
            Self::ScheduleEach => "schedule-each",
            Self::ScheduleInterval => "schedule-interval",
            Self::ScheduleZone => "schedule-zone",
            Self::SchemaConstraint => "schema-constraint",
            Self::ScreenOverride => "screen-override",
            Self::ScreenTemplate => "screen-template",
            Self::StaleAllowance => "stale-allowance",
            Self::StaleDocs => "stale-docs",
            Self::StaleWiring => "stale-wiring",
            Self::SystemTime => "system-time",
            Self::TaskRunner => "task-runner",
            Self::ThenScreen => "then-screen",
            Self::ThenScreenFragment => "then-screen-fragment",
            Self::TierDirection => "tier-direction",
            Self::UndeclaredGen => "undeclared-gen",
            Self::UndeclaredRoutes => "undeclared-routes",
            Self::UnknownArea => "unknown-area",
            Self::UnusedDependency => "unused-dependency",
            Self::UnusedItem => "unused-item",
            Self::UnusedRatio => "unused-ratio",
            Self::Visibility => "visibility",
            Self::WritingGet => "writing-get",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub rule: Rule,
    pub file: PathBuf,
    pub line: usize,
    pub message: String,
    pub fix: String,
}

impl Finding {
    fn render(&self, root: &Path) -> String {
        let file = self.file.strip_prefix(root).unwrap_or(&self.file);
        format!(
            "error[{}]: {}\n  --> {}:{}\n   = help: {}\n",
            self.rule.name(),
            self.message,
            file.display(),
            self.line,
            self.fix
        )
    }
}

impl fmt::Display for Finding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.render(Path::new("")))
    }
}

#[must_use]
pub fn check(app: &App, screens: &Screens) -> Vec<Finding> {
    let migrations = migration::Migrations::read(&app.modules);
    let mut findings = manifest::check_manifests(app);
    findings.extend(module_manifest::check_module_manifests(&app.modules));
    findings.extend(sources::check_sources(app));
    findings.extend(outgoing::check_gateways(&app.modules));
    findings.extend(routes::check_routes(app));
    findings.extend(safe_methods::check_safe_methods(&app.modules));
    findings.extend(own_contract::check_own_contract(&app.modules));
    findings.extend(fragments::check_fragments(&app.modules, screens));
    findings.extend(mails::check_mails(&app.modules));
    findings.extend(bindings::check_bindings(&app.modules, screens));
    findings.extend(actions::check_actions(app, screens));
    findings.extend(islands::check_islands(&app.modules, screens));
    findings.extend(exposed::check_exposed(&app.modules));
    findings.extend(exposed::check_mirrors(&app.modules));
    findings.extend(names::check_names(app));
    findings.extend(registrations::check_registrations(app));
    findings.extend(raw_sql::check_raw_sql(&app.modules));
    findings.extend(constraints::check_constraints(&app.modules, &migrations));
    findings.extend(cross_module::check_cross_module(&app.modules, &migrations));
    findings.extend(locking::check_locking(&app.modules, &migrations));
    findings.extend(messages::check_messages(app));
    findings.extend(messages::check_queued_routes(app));
    findings.extend(members::check_member_areas(app));
    findings.extend(visibility::check_visibility(&app.modules));
    findings.extend(wiring::check_wiring(app));
    findings.extend(flags::check_flags(app));
    findings.extend(entitlements::check_entitlements(app, screens));
    findings.extend(remote::check_remote_modules(app));
    findings.extend(tasks::check_task_runner(app));
    sort_findings(&mut findings);
    // A handler routed twice has its rendered screens checked twice.
    findings.dedup();
    findings
}

pub fn sort_findings(findings: &mut [Finding]) {
    findings.sort_by(|left, right| {
        (&left.file, left.line, left.rule.name()).cmp(&(&right.file, right.line, right.rule.name()))
    });
}

#[must_use]
pub fn report(root: &Path, findings: &[Finding]) -> String {
    if findings.is_empty() {
        return "ok\n".to_owned();
    }
    let mut blocks: Vec<String> = findings
        .iter()
        .map(|finding| finding.render(root) + "\n")
        .collect();
    let noun = if findings.len() == 1 {
        "problem"
    } else {
        "problems"
    };
    blocks.push(format!("{} {noun}\n", findings.len()));
    blocks.concat()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Finding, Rule, report};

    fn finding(rule: Rule, file: &str, line: usize) -> Finding {
        Finding {
            rule,
            file: PathBuf::from(file),
            line,
            message: format!("{} is broken", rule.name()),
            fix: format!("fix {}", rule.name()),
        }
    }

    #[test]
    fn reports_findings_relative_to_the_root_with_a_count() {
        let findings = [
            finding(
                Rule::RouteKind,
                "shop/modules/order/src/areas/web/routes.rs",
                5,
            ),
            finding(Rule::StaleWiring, "shop/app/src/wiring.rs", 1),
        ];

        let text = report(Path::new("shop"), &findings);

        assert_eq!(
            text,
            "error[route-kind]: route-kind is broken\n  --> modules/order/src/areas/web/routes.rs:5\n   = help: fix route-kind\n\n\
             error[stale-wiring]: stale-wiring is broken\n  --> app/src/wiring.rs:1\n   = help: fix stale-wiring\n\n\
             2 problems\n"
        );
    }

    #[test]
    fn reports_ok_when_clean() {
        assert_eq!(report(Path::new("."), &[]), "ok\n");
    }
}
