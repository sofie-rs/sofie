use sofie_model::naming::snake_case;
use sofie_model::{Aggregate, Module};

use super::Edits;
use super::schema::{SQLITE_NEEDS_NO_STEP, create_migration, next_migration_number};
use crate::gen_command::GenError;

const BEHAVIOR: &str = "Searched";

/// The behavior on the aggregate, its column, and the index built without
/// locking the table, which a migration of its own runs outside a transaction.
pub(super) fn new_search(
    edits: &mut Edits,
    module: &Module,
    aggregate: &str,
) -> Result<(), GenError> {
    let found = module
        .code
        .aggregates
        .iter()
        .find(|declared| declared.name == aggregate)
        .ok_or_else(|| {
            GenError::Usage(format!(
                "module `{}` declares no aggregate `{aggregate}`",
                module.krate.crate_name
            ))
        })?;
    if found.behaviors.iter().any(|behavior| behavior == BEHAVIOR) {
        return Err(GenError::Usage(format!(
            "`{aggregate}` is searched already"
        )));
    }
    add_behavior(edits, found)?;
    let table = snake_case(aggregate);
    let number = next_migration_number(module);
    let column = |table: &str| {
        format!(
            "-- What `Searched` keeps of {aggregate}'s searched fields; a record written\n\
             -- before is found once it is written again.\n\
             ALTER TABLE \"{table}\" ADD COLUMN search_text text NOT NULL DEFAULT '';\n"
        )
    };
    create_migration(
        edits,
        module,
        &format!("{number:04}_{table}_search_text.sql"),
        &column(&table),
        |prefix| column(&format!("{prefix}_{table}")),
    )?;
    create_migration(
        edits,
        module,
        &format!("{:04}_{table}_search_index.sql", number + 1),
        &format!(
            "CREATE INDEX CONCURRENTLY IF NOT EXISTS {table}_search ON \"{table}\" \
             USING gin (to_tsvector('simple', search_text));\n"
        ),
        |_| SQLITE_NEEDS_NO_STEP.to_owned(),
    )?;
    edits.enable_framework_feature(&module.krate, "search")
}

/// Into its `#[aggregate(behaviors(…))]`, or a new one above the struct.
fn add_behavior(edits: &mut Edits, aggregate: &Aggregate) -> Result<(), GenError> {
    let file = &aggregate.file;
    let declaration = format!("pub struct {} ", aggregate.name);
    edits.edit_rust(file, |text| {
        let struct_at = text.find(&declaration).ok_or_else(|| {
            GenError::App(format!(
                "{} has no `{}` to add `{BEHAVIOR}` to",
                file.display(),
                declaration.trim_end()
            ))
        })?;
        let attributes_from = text[..struct_at].rfind("#[derive(").unwrap_or(struct_at);
        Ok(match text[attributes_from..struct_at].find("behaviors(") {
            Some(list) => {
                let at = attributes_from + list + "behaviors(".len();
                format!("{}{BEHAVIOR}, {}", &text[..at], &text[at..])
            }
            None => format!(
                "{}#[aggregate(behaviors({BEHAVIOR}))]\n{}",
                &text[..struct_at],
                &text[struct_at..]
            ),
        })
    })
}
