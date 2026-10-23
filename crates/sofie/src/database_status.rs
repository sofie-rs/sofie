use std::fmt::Write;

use sofie_core::{Error, Result};
use sofie_db::{Db, MigrationState, Migrations, ModuleStatus};

pub(crate) async fn print(db: &Db, migrations: &[Migrations]) -> Result<()> {
    let modules = db.schema().migration_status(migrations).await?;
    print!("{}", report(&modules));
    let inconsistent: Vec<&str> = modules
        .iter()
        .filter(|module| !module.is_consistent())
        .map(|module| module.module.as_str())
        .collect();
    if inconsistent.is_empty() {
        return Ok(());
    }
    Err(Error::internal_message(format!(
        "migrations of {} were edited after being applied or are missing as files; \
         restore them, or rebuild a local database with `db fresh`",
        inconsistent.join(", ")
    )))
}

fn report(modules: &[ModuleStatus]) -> String {
    let mut text = String::new();
    for module in modules {
        let _ = writeln!(text, "{}", module.module);
        for migration in &module.migrations {
            let _ = writeln!(text, "  {:<8} {}", label(migration.state), migration.name);
        }
    }
    text
}

fn label(state: MigrationState) -> &'static str {
    match state {
        MigrationState::Applied => "applied",
        MigrationState::Pending => "pending",
        MigrationState::EditedAfterApplied => "edited",
        MigrationState::MissingFile => "missing",
    }
}

#[cfg(test)]
mod tests {
    use sofie_db::{MigrationState, MigrationStatus, ModuleStatus};

    use super::report;

    #[test]
    fn each_migration_shows_under_its_module_with_its_state() {
        let migration = |name: &str, state| MigrationStatus {
            name: name.to_owned(),
            state,
        };
        let modules = [ModuleStatus {
            module: "shop".to_owned(),
            migrations: vec![
                migration("0001_create.sql", MigrationState::Applied),
                migration("0002_edit.sql", MigrationState::EditedAfterApplied),
                migration("0003_gone.sql", MigrationState::MissingFile),
                migration("0004_next.sql", MigrationState::Pending),
            ],
        }];

        assert_eq!(
            report(&modules),
            "shop\n  applied  0001_create.sql\n  edited   0002_edit.sql\n  \
             missing  0003_gone.sql\n  pending  0004_next.sql\n"
        );
    }
}
