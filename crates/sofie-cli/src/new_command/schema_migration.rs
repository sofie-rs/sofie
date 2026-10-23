//! The module's current schema is read from its migration files, so no database is needed.

use std::fs;

use sofie_check::MigratedSchema;
use sofie_core::sql_lexer::lex;
use sofie_model::Module;

use super::Edits;
use super::declared_tables::{DeclaredColumn, DeclaredIndex, DeclaredTable, declared_tables};
use super::schema::{SQLITE_NEEDS_NO_STEP, create_migration, next_migration_number};
use crate::gen_command::GenError;

pub(super) fn new_schema_migration(
    edits: &mut Edits,
    module: &Module,
    name: &str,
) -> Result<(), GenError> {
    let schema = migrated_schema(module)?;
    let tables = declared_tables(module);
    let plan = MigrationPlan::of(&tables, &schema);
    if plan.is_empty() {
        return Err(GenError::Usage(nothing_to_do(module, &plan.notes)));
    }
    let first = next_migration_number(module);
    for (number, migration) in (first..).zip(plan.migrations(name)) {
        let notes = if number == first {
            plan.comments()
        } else {
            String::new()
        };
        create_migration(
            edits,
            module,
            &format!("{number:04}_{}.sql", migration.name),
            &(notes.clone() + &migration.render(Dialect::Postgres)),
            |prefix| notes + &migration.render(Dialect::Sqlite { prefix }),
        )?;
    }
    Ok(())
}

fn migrated_schema(module: &Module) -> Result<MigratedSchema, GenError> {
    let mut schema = MigratedSchema::default();
    for file in &module.migrations {
        let sql = fs::read_to_string(file).map_err(|source| GenError::Io {
            path: file.clone(),
            source,
        })?;
        schema.read(&lex(&sql));
    }
    Ok(schema)
}

fn nothing_to_do(module: &Module, notes: &[String]) -> String {
    let mut message = format!(
        "module `{}`'s migrations already create every table, column, CHECK and index its \
         aggregates declare; nothing to write",
        module.krate.crate_name
    );
    for note in notes {
        message.push_str("\n  ");
        message.push_str(note);
    }
    message
}

#[derive(Clone, Copy)]
enum Statement<'a> {
    CreateTable(&'a DeclaredTable),
    CreateIndex {
        table: &'a str,
        index: &'a DeclaredIndex,
        concurrently: bool,
    },
    AddColumn {
        table: &'a str,
        column: &'a DeclaredColumn,
    },
    AddCheck {
        table: &'a str,
        column: &'a DeclaredColumn,
        added: bool,
    },
    Validate {
        table: &'a str,
        column: &'a DeclaredColumn,
    },
}

/// Postgres validates a CHECK added `NOT VALID` and builds an index concurrently
/// only in a later migration, so the plan spans several files.
#[derive(Default)]
struct MigrationPlan<'a> {
    /// What the generator leaves to the developer.
    notes: Vec<String>,
    /// Contract steps, written commented out: never destructive by default.
    contracts: Vec<String>,
    expand: Vec<Statement<'a>>,
    validations: Vec<Statement<'a>>,
    concurrent_indexes: Vec<(&'a str, DeclaredIndex)>,
}

struct Migration<'a> {
    name: String,
    statements: Vec<Statement<'a>>,
}

impl<'a> MigrationPlan<'a> {
    fn of(tables: &'a [DeclaredTable], schema: &MigratedSchema) -> Self {
        let mut plan = Self::default();
        for table in tables {
            if schema.knows_table(&table.name) {
                plan.alter(table, schema);
            } else {
                plan.create(table);
            }
        }
        plan
    }

    fn is_empty(&self) -> bool {
        self.contracts.is_empty()
            && self.expand.is_empty()
            && self.validations.is_empty()
            && self.concurrent_indexes.is_empty()
    }

    fn create(&mut self, table: &'a DeclaredTable) {
        self.expand.push(Statement::CreateTable(table));
        self.expand
            .extend(table.indexes.iter().map(|index| Statement::CreateIndex {
                table: &table.name,
                index,
                concurrently: false,
            }));
        for column in &table.columns {
            self.note_bounds(&table.name, column);
        }
        for field in &table.unknown {
            self.note_unknown(&table.name, &field.name, &field.type_name);
        }
        self.notes
            .extend(table.unshaped.iter().map(|index| index.reason.clone()));
    }

    fn alter(&mut self, table: &'a DeclaredTable, schema: &MigratedSchema) {
        for column in &table.columns {
            if schema.has_column(&table.name, &column.name) {
                self.check_existing(&table.name, column, schema);
            } else {
                self.add_column(&table.name, column);
            }
        }
        for index in &table.indexes {
            if !is_present(&table.name, index, schema) {
                self.add_index(&table.name, index, schema);
            }
        }
        for index in &table.unshaped {
            if !schema.has_index(&format!("{}_{}", table.name, index.suffix)) {
                self.notes.push(index.reason.clone());
            }
        }
        let existing = schema.columns(&table.name);
        for field in &table.unknown {
            if !existing.iter().any(|column| field.may_store(column)) {
                self.note_unknown(&table.name, &field.name, &field.type_name);
            }
        }
        for column in existing {
            if column != "id" && !table.may_store(column) {
                self.contracts.push(format!(
                    "Contract step, once no running version reads `{}.{column}`:\n\
                     ALTER TABLE {} DROP COLUMN {};",
                    table.name,
                    quoted(&table.name),
                    quoted(column)
                ));
            }
        }
    }

    /// A unique index takes the `_key` name Postgres gives an inline `UNIQUE` when a plain
    /// index holds its own: `CREATE INDEX IF NOT EXISTS` would skip it silently.
    fn add_index(&mut self, table: &'a str, index: &DeclaredIndex, schema: &MigratedSchema) {
        let name = format!("{table}_{}", index.suffix);
        let mut index = index.clone();
        if index.unique && schema.has_index(&name) {
            index.suffix.push_str("_key");
            self.contracts.push(format!(
                "Contract step: the unique index `{name}_key` serves every lookup the index \
                 `{name}` does, so once it is built:\n\
                 DROP INDEX CONCURRENTLY IF EXISTS {};",
                quoted(&name)
            ));
        }
        self.concurrent_indexes.push((table, index));
    }

    fn add_column(&mut self, table: &'a str, column: &'a DeclaredColumn) {
        self.expand.push(Statement::AddColumn { table, column });
        if !column.allowed.is_empty() {
            self.expand.push(Statement::AddCheck {
                table,
                column,
                added: true,
            });
            self.validations.push(Statement::Validate { table, column });
        }
        if !column.nullable {
            self.contracts.push(match column.backfill {
                Some(_) => drop_default_contract(table, &column.name),
                None => not_null_contract(
                    table,
                    &column.name,
                    "added nullable, as existing rows have no value",
                ),
            });
        }
        self.note_bounds(table, column);
    }

    fn check_existing(
        &mut self,
        table: &'a str,
        column: &'a DeclaredColumn,
        schema: &MigratedSchema,
    ) {
        self.compare_column(table, column, schema);
        if column.allowed.is_empty() {
            return;
        }
        let Some(checks) = schema.check_literals(table, &column.name) else {
            self.expand.push(Statement::AddCheck {
                table,
                column,
                added: false,
            });
            self.validations.push(Statement::Validate { table, column });
            return;
        };
        let lists_all = checks.iter().any(|listed| {
            column
                .allowed
                .iter()
                .all(|name| listed.contains(&name.as_str()))
        });
        if !lists_all {
            self.notes.push(format!(
                "The CHECK on `{table}.{}` does not list every one of {}: replace it with one that \
                 does, by hand.",
                column.name,
                literals(&column.allowed)
            ));
        }
    }

    fn compare_column(&mut self, table: &str, column: &DeclaredColumn, schema: &MigratedSchema) {
        let name = &column.name;
        if schema.has_type(table, name, column.sql_type) == Some(false) {
            self.notes.push(format!(
                "`{table}.{name}` is declared as `{}`, yet migrated as another type: change it by \
                 hand, in steps the running version can read through.",
                column.sql_type
            ));
        }
        match schema.is_nullable(table, name) {
            Some(true) if !column.nullable => {
                self.notes
                    .push(not_null_contract(table, name, "migrated nullable"));
            }
            Some(false) if column.nullable => self.notes.push(format!(
                "`{table}.{name}` is optional, yet migrated NOT NULL, so a record without one \
                 fails to save. Before a version saving one runs:\n\
                 ALTER TABLE {} ALTER COLUMN {} DROP NOT NULL;",
                quoted(table),
                quoted(name)
            )),
            _ => {}
        }
    }

    fn note_bounds(&mut self, table: &str, column: &DeclaredColumn) {
        if column.bounds.is_empty() {
            return;
        }
        let bounds: Vec<String> = column
            .bounds
            .iter()
            .map(|bound| format!("`{bound}`"))
            .collect();
        self.notes.push(format!(
            "`{table}.{}` declares {}: add the CHECK it needs by hand.",
            column.name,
            bounds.join(", ")
        ));
    }

    fn note_unknown(&mut self, table: &str, field: &str, type_name: &str) {
        self.notes.push(format!(
            "`{table}.{field}` is a `{type_name}`, whose columns `sofie gen migration` cannot \
             tell: add them by hand."
        ));
    }

    fn comments(&self) -> String {
        self.notes
            .iter()
            .chain(&self.contracts)
            .map(|paragraph| commented(paragraph) + "\n")
            .collect()
    }

    fn migrations(&self, name: &str) -> Vec<Migration<'_>> {
        let mut migrations = Vec::new();
        if !self.expand.is_empty() || !self.contracts.is_empty() {
            migrations.push(Migration {
                name: name.to_owned(),
                statements: self.expand.clone(),
            });
        }
        if !self.validations.is_empty() {
            migrations.push(Migration {
                name: format!("{name}_checks"),
                statements: self.validations.clone(),
            });
        }
        migrations.extend(
            self.concurrent_indexes
                .iter()
                .map(|(table, index)| Migration {
                    name: format!("index_{table}_{}", index.suffix),
                    statements: vec![Statement::CreateIndex {
                        table,
                        index,
                        concurrently: true,
                    }],
                }),
        );
        migrations
    }
}

fn is_present(table: &str, index: &DeclaredIndex, schema: &MigratedSchema) -> bool {
    let columns: Vec<&str> = index.columns.iter().map(String::as_str).collect();
    match (index.unique, columns.as_slice()) {
        (true, _) => schema.is_unique_over(table, &columns),
        (false, [column]) => schema.is_indexed(table, column),
        (false, _) => schema.has_index(&format!("{table}_{}", index.suffix)),
    }
}

/// The version still serving inserts rows without the column, so its default outlives the expand step.
fn drop_default_contract(table: &str, column: &str) -> String {
    format!(
        "Contract step, once no running version inserts without `{table}.{column}`:\n\
         ALTER TABLE {} ALTER COLUMN {} DROP DEFAULT;",
        quoted(table),
        quoted(column)
    )
}

fn not_null_contract(table: &str, column: &str, why: &str) -> String {
    let (table_name, column_name) = (quoted(table), quoted(column));
    let constraint = quoted(&format!("{table}_{column}_not_null"));
    format!(
        "Contract step: `{table}.{column}` is required, yet {why}. Once every row has one:\n\
         ALTER TABLE {table_name} ADD CONSTRAINT {constraint} CHECK ({column_name} IS NOT NULL) NOT VALID;\n\
         then in a later migration:\n\
         ALTER TABLE {table_name} VALIDATE CONSTRAINT {constraint};\n\
         ALTER TABLE {table_name} ALTER COLUMN {column_name} SET NOT NULL;"
    )
}

#[derive(Clone, Copy)]
enum Dialect<'m> {
    Postgres,
    /// SQLite has no schemas, so a module's tables are named `<module>_<table>`.
    Sqlite {
        prefix: &'m str,
    },
}

impl Dialect<'_> {
    fn table(self, table: &str) -> String {
        match self {
            Self::Postgres => table.to_owned(),
            Self::Sqlite { prefix } => format!("{prefix}_{table}"),
        }
    }
}

impl Migration<'_> {
    fn render(&self, dialect: Dialect) -> String {
        let statements: Vec<String> = self
            .statements
            .iter()
            .map(|statement| statement.render(dialect))
            .filter(|text| !text.is_empty())
            .collect();
        if statements.is_empty() && matches!(dialect, Dialect::Sqlite { .. }) {
            return SQLITE_NEEDS_NO_STEP.to_owned();
        }
        statements.concat()
    }
}

impl Statement<'_> {
    fn render(&self, dialect: Dialect) -> String {
        match *self {
            Self::CreateTable(table) => create_table(table, dialect),
            Self::CreateIndex {
                table,
                index,
                concurrently,
            } => create_index(table, index, concurrently, dialect),
            Self::AddColumn { table, column } => add_column(table, column, dialect),
            Self::AddCheck {
                table,
                column,
                added,
            } => add_check(table, column, added, dialect),
            Self::Validate { table, column } => match dialect {
                Dialect::Postgres => format!(
                    "ALTER TABLE {} VALIDATE CONSTRAINT {};\n",
                    quoted(table),
                    quoted(&check_name(table, &column.name))
                ),
                Dialect::Sqlite { .. } => String::new(),
            },
        }
    }
}

fn create_table(table: &DeclaredTable, dialect: Dialect) -> String {
    let id = match dialect {
        Dialect::Postgres => "\"id\" uuid PRIMARY KEY",
        Dialect::Sqlite { .. } => "\"id\" uuid PRIMARY KEY NOT NULL",
    };
    let columns: Vec<String> = std::iter::once(id.to_owned())
        .chain(table.columns.iter().map(|column| {
            let required = if column.nullable { "" } else { " NOT NULL" };
            format!(
                "{} {}{required}{}",
                quoted(&column.name),
                column.sql_type,
                inline_check(column)
            )
        }))
        .collect();
    format!(
        "CREATE TABLE {} (\n    {}\n);\n",
        quoted(&dialect.table(&table.name)),
        columns.join(",\n    ")
    )
}

fn create_index(
    table: &str,
    index: &DeclaredIndex,
    concurrently: bool,
    dialect: Dialect,
) -> String {
    let table = dialect.table(table);
    let unique = if index.unique { "UNIQUE " } else { "" };
    let concurrently = match dialect {
        Dialect::Postgres if concurrently => "CONCURRENTLY IF NOT EXISTS ",
        _ => "",
    };
    let columns: Vec<String> = index.columns.iter().map(|column| quoted(column)).collect();
    let only_where = index
        .only_where
        .map(|column| format!(" WHERE {}", quoted(column)))
        .unwrap_or_default();
    format!(
        "CREATE {unique}INDEX {concurrently}{} ON {} ({}){only_where};\n",
        quoted(&format!("{table}_{}", index.suffix)),
        quoted(&table),
        columns.join(", ")
    )
}

fn add_column(table: &str, column: &DeclaredColumn, dialect: Dialect) -> String {
    let table = quoted(&dialect.table(table));
    let name = quoted(&column.name);
    let added = format!("ALTER TABLE {table} ADD COLUMN {name} {}", column.sql_type);
    match (dialect, column.backfill) {
        (Dialect::Postgres, Some(backfill)) if !column.nullable => {
            format!("{added} NOT NULL DEFAULT {backfill};\n")
        }
        (Dialect::Postgres, _) => format!("{added};\n"),
        (Dialect::Sqlite { .. }, Some(NOW)) if !column.nullable => format!(
            "-- SQLite adds no NOT NULL column without a constant default, so it stays nullable.\n\
             {added};\nUPDATE {table} SET {name} = {SQLITE_NOW};\n"
        ),
        (Dialect::Sqlite { .. }, Some(backfill)) if !column.nullable => format!(
            "{added} NOT NULL DEFAULT {backfill}{};\n",
            inline_check(column)
        ),
        (Dialect::Sqlite { .. }, _) => {
            let nullable = if column.nullable {
                ""
            } else {
                "-- SQLite adds no NOT NULL column without a constant default, so it stays nullable.\n"
            };
            format!("{nullable}{added}{};\n", inline_check(column))
        }
    }
}

const NOW: &str = "now()";

/// As sofie-sqlite stores an instant: microseconds since the epoch.
const SQLITE_NOW: &str = "CAST(unixepoch('subsec') * 1000000 AS INTEGER)";

fn add_check(table: &str, column: &DeclaredColumn, added: bool, dialect: Dialect) -> String {
    match dialect {
        Dialect::Postgres => format!(
            "ALTER TABLE {} ADD CONSTRAINT {} CHECK ({}) NOT VALID;\n",
            quoted(table),
            quoted(&check_name(table, &column.name)),
            allowed_values(column)
        ),
        Dialect::Sqlite { .. } if added => String::new(),
        Dialect::Sqlite { .. } => format!(
            "-- SQLite adds no CHECK to an existing column: copy `{}` into a table whose `{}` \
             has CHECK ({}).\n",
            dialect.table(table),
            column.name,
            allowed_values(column)
        ),
    }
}

/// Postgres's own name for a CHECK on one column, so a later migration names it either way.
fn check_name(table: &str, column: &str) -> String {
    format!("{table}_{column}_check")
}

fn inline_check(column: &DeclaredColumn) -> String {
    if column.allowed.is_empty() {
        return String::new();
    }
    format!(" CHECK ({})", allowed_values(column))
}

fn allowed_values(column: &DeclaredColumn) -> String {
    format!(
        "{} IN ({})",
        quoted(&column.name),
        literals(&column.allowed)
    )
}

fn literals(names: &[String]) -> String {
    let quoted: Vec<String> = names.iter().map(|name| format!("'{name}'")).collect();
    quoted.join(", ")
}

/// Every name quoted: a table named after a type is often a reserved word (`order`, `user`).
fn quoted(name: &str) -> String {
    format!("\"{name}\"")
}

fn commented(paragraph: &str) -> String {
    paragraph
        .lines()
        .map(|line| format!("-- {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}
