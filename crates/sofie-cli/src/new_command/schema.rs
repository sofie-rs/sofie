use sofie_model::naming::{DEFAULT_BEHAVIORS, behavior_columns, file_stem, snake_case};
use sofie_model::{Module, ModuleDeclaration, module_declaration};

use super::Edits;
use crate::gen_command::GenError;
use crate::source_edits::{add_import, add_module_line};

pub(super) const SQLITE_NEEDS_NO_STEP: &str = "-- SQLite needs no such step.\n";

pub(super) fn next_migration_number(module: &Module) -> u32 {
    module
        .migrations
        .iter()
        .filter_map(|migration| file_stem(migration).split('_').next()?.parse::<u32>().ok())
        .max()
        .map_or(1, |last| last + 1)
}

pub(super) fn new_migration(
    edits: &mut Edits,
    module: &Module,
    name: &str,
    postgres: &str,
    sqlite: impl FnOnce(&str) -> String,
) -> Result<(), GenError> {
    let number = next_migration_number(module);
    create_migration(
        edits,
        module,
        &format!("{number:04}_{name}.sql"),
        postgres,
        sqlite,
    )
}

/// A module with `migrations/sqlite/` has SQLite run its twin instead of the Postgres file,
/// whose SQL SQLite may not read; `sqlite` writes the twin from the module's table prefix.
pub(super) fn create_migration(
    edits: &mut Edits,
    module: &Module,
    file_name: &str,
    postgres: &str,
    sqlite: impl FnOnce(&str) -> String,
) -> Result<(), GenError> {
    let folder = module.krate.path.join("migrations");
    edits.create(&folder.join(file_name), postgres)?;
    if module.sqlite_migrations.is_empty() {
        return Ok(());
    }
    let twin = sqlite(&module.krate.crate_name);
    edits.create(&folder.join("sqlite").join(file_name), &twin)
}

/// No `factory.rs`: one nothing calls yet is dead code, and with `sofie/factories` the derive writes the default one.
pub(super) fn new_aggregate(
    edits: &mut Edits,
    module: &Module,
    name: &str,
) -> Result<(), GenError> {
    let folder = snake_case(name);
    let library = module.krate.path.join("src").join("lib.rs");
    let declared = module_declaration(&library, &[&folder]).map_err(GenError::Model)?;
    if module
        .code
        .aggregates
        .iter()
        .any(|aggregate| aggregate.name == name)
        || declared == ModuleDeclaration::Declared
    {
        return Err(GenError::Usage(format!(
            "module `{}` has `{name}` or a module `{folder}` already; choose another name",
            module.krate.crate_name
        )));
    }
    edits.create(
        &module.krate.path.join("src").join(&folder).join("mod.rs"),
        &format!(
            "use sofie::prelude::*;\n\n#[derive(Aggregate)]\npub struct {name} {{\n    meta: Meta,\n}}\n"
        ),
    )?;
    edits.edit_rust(&library, |text| {
        let declared = add_module_line(text, &format!("mod {folder};"));
        Ok(add_import(&declared, &format!("pub use {folder}"), name))
    })?;
    new_migration(
        edits,
        module,
        &format!("create_{folder}"),
        &create_table(&folder, "id uuid PRIMARY KEY"),
        |prefix| {
            create_table(
                &format!("{prefix}_{folder}"),
                "id uuid PRIMARY KEY NOT NULL",
            )
        },
    )
}

/// Quoted, since a table named after a type is often a reserved word (`order`, `user`).
fn create_table(table: &str, id: &str) -> String {
    let mut columns = vec![id.to_owned()];
    columns.extend(
        DEFAULT_BEHAVIORS
            .iter()
            .flat_map(|behavior| behavior_columns(behavior))
            .map(|column| format!("{} {} NOT NULL", column.name, column.sql_type)),
    );
    format!(
        "CREATE TABLE \"{table}\" (\n    {}\n);\n",
        columns.join(",\n    ")
    )
}
