mod agents_md;
mod api;
mod bench;
mod build_command;
mod build_script;
mod cargo;
mod cargo_rustflags;
mod check_command;
mod child_process;
mod clients;
mod components;
mod config_command;
mod contract_gen;
mod db_command;
mod dev_command;
mod dockerfile;
#[cfg(feature = "docs")]
mod docs;
mod env_example;
mod explain;
mod feature_command;
mod fix_command;
mod flows;
mod forms;
mod fuzz;
mod gen_command;
mod http_client;
mod inputs;
mod islands;
mod local_app;
mod log_buffer;
#[cfg(feature = "mcp")]
mod mcp;
mod module_roles;
mod new_command;
mod pgo;
mod routes_command;
mod rustfmt;
mod scaffold_command;
mod scaffold_tests;
mod schedule_command;
mod schema_command;
mod seed_command;
mod sorted;
mod source_edits;
mod split_command;
mod targets;
mod tasks_command;
mod translations;
mod upgrade;
mod wiring;

#[cfg(feature = "openapi")]
pub use api::openapi::openapi;
#[cfg(feature = "typescript")]
pub use api::typescript::typescript;
pub use bench::{BenchReport, Measurement, RouteResult, Verdict, bench};
pub use build_command::{Binary, BuildOptions, Built, Image, build};
pub use build_script::check_in_build_script;
pub use check_command::check_app;
pub use components::{component, component_list};
pub use config_command::check_config;
pub use db_command::fresh_database;
pub use dev_command::dev;
#[cfg(feature = "docs")]
pub use docs::write_docs;
pub use explain::explain;
pub use feature_command::{FeatureChange, FeatureEdit, change_feature};
pub use fix_command::{FixReport, fix_app};
pub use fuzz::{Finding, FuzzOptions, FuzzReport, Outcome, fuzz};
pub use gen_command::{GenError, GenMode, GenOutcome, gen_app, regenerate, regenerated};
pub use islands::{IslandMode, has_islands};
#[cfg(feature = "mcp")]
pub use mcp::serve_mcp;
pub use new_command::{
    AccountScreen, AreaAccess, FileChange, NEW_COMMANDS, NewCode, NewCommand, NewMenuEntry,
    NewRoute, new_app, new_code, new_module,
};
pub use routes_command::route_table;
pub use scaffold_command::scaffold_screen;
pub use scaffold_tests::{ScaffoldedTests, scaffold_tests};
pub use schedule_command::run_schedule;
pub use schema_command::check_schema;
pub use seed_command::{SeedReport, SeededRoute, seed};
pub use split_command::{Split, SplitPlan, split_module, split_plan};
pub use tasks_command::task_table;
pub use upgrade::{Change, UpgradeReport, upgrade};
