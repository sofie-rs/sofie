use sofie_model::naming::{behavior_columns, snake_case};
use sofie_model::{Aggregate, FieldDeclaration, InputDeclaration, Module, TypeName};

use super::migration::{Migrations, Schema};
use super::{Finding, Rule};

/// A write that skips the input (a job, a script, another service) must still meet the schema.
pub(super) fn check_constraints(modules: &[Module], migrations: &Migrations) -> Vec<Finding> {
    modules
        .iter()
        .flat_map(|module| check_module(module, migrations))
        .collect()
}

fn check_module(module: &Module, migrations: &Migrations) -> Vec<Finding> {
    if module.code.aggregates.is_empty() {
        return Vec::new();
    }
    let mut findings: Vec<Finding> = module
        .code
        .inputs
        .iter()
        .flat_map(|input| dropped_constraints(input, module))
        .collect();
    let mut schema = Schema::default();
    // `migration-lock` reports a migration it cannot read.
    for (_, migration) in migrations.of(module) {
        if let Ok(migration) = migration {
            schema.read(migration);
        }
    }
    for aggregate in &module.code.aggregates {
        let table = snake_case(&aggregate.name);
        findings.extend(missing_behavior_columns(aggregate, &table, &schema));
        for field in &aggregate.fields {
            let site = Site {
                aggregate,
                field,
                table: &table,
            };
            findings.extend(site.check(&schema, module));
        }
    }
    findings
}

fn missing_behavior_columns(aggregate: &Aggregate, table: &str, schema: &Schema) -> Vec<Finding> {
    if !schema.knows_table(table) {
        return Vec::new();
    }
    aggregate
        .all_behaviors()
        .flat_map(|behavior| {
            behavior_columns(behavior)
                .iter()
                .map(move |column| (behavior, column))
        })
        .filter(|(_, column)| !schema.has_column(table, column.name))
        .map(|(behavior, column)| {
            let (column, ty) = (column.name, column.sql_type);
            Finding {
                rule: Rule::BehaviorColumn,
                file: aggregate.file.clone(),
                line: aggregate.line,
                message: format!(
                    "`{}` has the behavior `{behavior}`, but no migration gives `{table}` the \
                     column `{column}` it reads and writes",
                    aggregate.name
                ),
                fix: format!(
                    "add it in a new migration: `ALTER TABLE \"{table}\" ADD COLUMN {column} \
                     {ty}` with the default or backfill the existing rows need"
                ),
            }
        })
        .collect()
}

/// An input field declaring its own constraints replaces the schema's, yet the database still holds the dropped ones.
fn dropped_constraints(input: &InputDeclaration, module: &Module) -> Vec<Finding> {
    input
        .fields
        .iter()
        .filter(|field| !field.constraints.is_empty())
        .filter_map(|field| {
            let inherited = input.inherited(&field.name, &module.code)?;
            let dropped: Vec<&str> = inherited
                .constraints
                .iter()
                .map(String::as_str)
                .filter(|name| {
                    *name != "unique" && !field.constraints.iter().any(|kept| kept == name)
                })
                .collect();
            let names = dropped
                .iter()
                .map(|name| format!("`#[{name}]`"))
                .collect::<Vec<_>>();
            (!dropped.is_empty()).then(|| Finding {
                rule: Rule::SchemaConstraint,
                file: input.file.clone(),
                line: field.line,
                message: format!(
                    "`{}.{}` replaces the constraints it inherits from `{}` without {}, which the \
                     database still holds, so the input accepts what the commit refuses",
                    input.name,
                    field.name,
                    input.from.as_deref().unwrap_or_default(),
                    names.join(", ")
                ),
                fix: "declare those constraints on the input's field too, or mark it \
                      `#[unconstrained]` when the action keeps the value within them"
                    .to_owned(),
            })
        })
        .collect()
}

struct Site<'a> {
    aggregate: &'a Aggregate,
    field: &'a FieldDeclaration,
    table: &'a str,
}

impl Site<'_> {
    fn check(&self, schema: &Schema, module: &Module) -> Vec<Finding> {
        let column = self.field.name.as_str();
        let mut findings = Vec::new();
        let checks = schema.check_literals(self.table, column);
        if let Some(declared) = self.field.constraints.iter().find(|name| *name != "unique")
            && checks.is_none()
        {
            findings.push(self.missing_check(&format!("`#[{declared}]`")));
        }
        if self.field.constraints.iter().any(|name| name == "unique")
            && !schema.is_unique(self.table, column)
        {
            findings.push(self.finding(
                "`#[unique]`, but no migration makes the column unique",
                format!(
                    "add `CREATE UNIQUE INDEX {table}_{column} ON {table} ({column});` in a new \
                     migration",
                    table = self.table
                ),
            ));
        }
        findings.extend(self.check_enum(checks.as_deref(), module));
        findings
    }

    fn check_enum(&self, checks: Option<&[Vec<&str>]>, module: &Module) -> Option<Finding> {
        let enumeration = module
            .code
            .enums
            .iter()
            .find(|enumeration| enumeration.name == stored_type(&self.field.ty).name)?;
        let Some(checks) = checks else {
            return Some(self.missing_check(&format!("enum `{}`", enumeration.name)));
        };
        let unlisted = |listed: &Vec<&str>| -> Vec<&str> {
            enumeration
                .stored_names
                .iter()
                .map(String::as_str)
                .filter(|name| !listed.contains(name))
                .collect()
        };
        let missing = checks
            .iter()
            .map(unlisted)
            .min_by_key(Vec::len)
            .unwrap_or_default();
        if missing.is_empty() {
            return None;
        }
        Some(self.finding(
            &format!(
                "enum `{}`, but its CHECK does not list {}",
                enumeration.name,
                quoted(&missing)
            ),
            format!(
                "replace the CHECK on `{}.{}` in a new migration with one listing every variant",
                self.table, self.field.name
            ),
        ))
    }

    fn missing_check(&self, declared: &str) -> Finding {
        self.finding(
            &format!("{declared}, but no migration adds a CHECK on its column"),
            format!(
                "add `ALTER TABLE {} ADD CHECK (…{}…);` in a new migration",
                self.table, self.field.name
            ),
        )
    }

    fn finding(&self, declared: &str, fix: String) -> Finding {
        Finding {
            rule: Rule::SchemaConstraint,
            file: self.aggregate.file.clone(),
            line: self.field.line,
            message: format!(
                "`{}.{}` declares {declared}, so a write skipping the input could store what the \
                 schema refuses",
                self.aggregate.name, self.field.name
            ),
            fix,
        }
    }
}

fn stored_type(ty: &TypeName) -> &TypeName {
    match (ty.name.as_str(), ty.arguments.first()) {
        ("Option", Some(inner)) => inner,
        _ => ty,
    }
}

fn quoted(names: &[&str]) -> String {
    names
        .iter()
        .map(|name| format!("'{name}'"))
        .collect::<Vec<_>>()
        .join(", ")
}
