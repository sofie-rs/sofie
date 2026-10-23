//! Applied like any migration, a role migration is never rewritten, so a later template leaves it as it is.

use std::path::PathBuf;

use sofie_model::App;

const TEMPLATE: &str = include_str!("../module_role.sql");

const FILE_NAME: &str = "0000_module_role.sql";

pub(crate) fn missing_role_migrations(app: &App) -> Vec<(PathBuf, String)> {
    app.modules
        .iter()
        .filter(|module| !module.migrations.is_empty())
        .map(|module| (module.krate.path.join("migrations").join(FILE_NAME), module))
        .filter(|(path, _)| !path.exists())
        .map(|(path, module)| (path, TEMPLATE.replace("{module}", &module.krate.crate_name)))
        .collect()
}

pub(crate) fn with_role_migrations(app: &App, missing: &[(PathBuf, String)]) -> App {
    let mut app = app.clone();
    for module in &mut app.modules {
        let folder = module.krate.path.join("migrations");
        module.migrations.extend(
            missing
                .iter()
                .map(|(path, _)| path)
                .filter(|path| path.parent() == Some(folder.as_path()))
                .cloned(),
        );
        module.migrations.sort();
    }
    app
}

#[cfg(test)]
mod tests {
    use sofie_model::testing::{TemporaryDirectory, app, module};

    use super::*;
    use crate::gen_command::{GenMode, GenOutcome, sync_file};

    #[test]
    fn a_module_with_migrations_gets_its_role_migration_first_and_once() {
        let directory = TemporaryDirectory::new("sofie-module-roles");
        let mut catalog = module("catalog", directory.path("modules/catalog"));
        catalog.migrations = vec![directory.write(
            "modules/catalog/migrations/0001_products.sql",
            "CREATE TABLE product (id uuid PRIMARY KEY);",
        )];
        let empty = module("search", directory.path("modules/search"));
        let model = app(directory.root(), vec![catalog, empty]);

        let missing = missing_role_migrations(&model);
        let with_roles = with_role_migrations(&model, &missing);

        let role_file = directory.path("modules/catalog/migrations/0000_module_role.sql");
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].0, role_file);
        assert!(
            missing[0]
                .1
                .contains("SELECT sofie.module_role('catalog');")
        );
        assert!(!missing[0].1.contains("{module}"));
        assert_eq!(with_roles.modules[0].migrations[0], role_file);
        let text = Some(missing[0].1.clone());
        assert_eq!(
            sync_file(&role_file, text.clone(), GenMode::Check).unwrap(),
            Some(GenOutcome::Stale(role_file.clone()))
        );
        sync_file(&role_file, text, GenMode::Write).unwrap();
        assert_eq!(missing_role_migrations(&model), []);
    }
}
