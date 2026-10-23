use sofie_core::Result;

use super::{Applied, Migrations, Schema, applied_migrations, checksum, name_order};
use crate::{Engine, Step};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MigrationState {
    Applied,
    Pending,
    EditedAfterApplied,
    MissingFile,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MigrationStatus {
    pub name: String,
    pub state: MigrationState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleStatus {
    pub module: String,
    pub migrations: Vec<MigrationStatus>,
}

impl ModuleStatus {
    #[must_use]
    pub fn is_consistent(&self) -> bool {
        self.migrations.iter().all(|migration| {
            matches!(
                migration.state,
                MigrationState::Applied | MigrationState::Pending
            )
        })
    }
}

impl Schema<'_> {
    /// A module whose files are all gone still shows through its records.
    pub async fn migration_status(self, modules: &[Migrations]) -> Result<Vec<ModuleStatus>> {
        let dialect = self.db.driver().dialect();
        self.db
            .transaction(vec![Step::Script(dialect.migrations_setup().to_owned())])
            .await?;
        let mut applied = applied_migrations(self.db).await?;
        let framework = dialect.framework_migrations();
        let mut statuses = std::iter::once(&framework)
            .chain(modules)
            .map(|migrations| module_status(migrations, dialect.engine(), &mut applied))
            .collect::<Result<Vec<ModuleStatus>>>()?;
        statuses.extend(unknown_modules(applied));
        Ok(statuses)
    }
}

fn module_status(
    migrations: &Migrations,
    engine: Engine,
    applied: &mut Applied,
) -> Result<ModuleStatus> {
    let module = migrations.module.to_owned();
    let mut statuses: Vec<MigrationStatus> = migrations
        .in_name_order(engine)?
        .into_iter()
        .map(|(name, sql)| {
            let recorded = applied.remove(&(module.clone(), name.to_owned()));
            let state = match recorded {
                None => MigrationState::Pending,
                Some(sum) if sum == checksum(&sql) => MigrationState::Applied,
                Some(_) => MigrationState::EditedAfterApplied,
            };
            MigrationStatus {
                name: name.to_owned(),
                state,
            }
        })
        .collect();
    let missing = applied.extract_if(|(recorded_module, _), _| *recorded_module == module);
    statuses.extend(missing.map(|((_, name), _)| MigrationStatus {
        name,
        state: MigrationState::MissingFile,
    }));
    statuses.sort_by(|first, second| name_order(&first.name, &second.name));
    Ok(ModuleStatus {
        module,
        migrations: statuses,
    })
}

fn unknown_modules(applied: Applied) -> Vec<ModuleStatus> {
    let mut records: Vec<(String, String)> = applied.into_keys().collect();
    records.sort();
    let mut statuses: Vec<ModuleStatus> = Vec::new();
    for (module, name) in records {
        let missing = MigrationStatus {
            name,
            state: MigrationState::MissingFile,
        };
        match statuses.last_mut() {
            Some(last) if last.module == module => last.migrations.push(missing),
            _ => statuses.push(ModuleStatus {
                module,
                migrations: vec![missing],
            }),
        }
    }
    statuses
}
