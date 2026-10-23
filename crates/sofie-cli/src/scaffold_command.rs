//! A resource's screens come from its description, which only the compiled app has, so the app's binary prints the source.

use std::path::{Path, PathBuf};
use std::process::Command;

use sofie_model::{App, AreaContribution, Module};

use crate::cargo::{self, Diagnostics};
use crate::child_process;
use crate::dev_command::{DEVELOPMENT, app_command};
use crate::gen_command::{GenError, io_error};
use crate::source_edits::write;

pub fn scaffold_screen(root: &Path, key: &str) -> Result<PathBuf, GenError> {
    scaffold_screen_built_by(root, root, key, Diagnostics::Terminal)
}

/// `built` prints the source, so a dry run's copy writes what the original app prints
/// and nothing is built outside the original's target directory.
pub(crate) fn scaffold_screen_built_by(
    root: &Path,
    built: &Path,
    key: &str,
    diagnostics: Diagnostics,
) -> Result<PathBuf, GenError> {
    let app = sofie_model::load(root).map_err(GenError::Model)?;
    let file = override_file(&app, key)?;
    let source = built_in_source(built, key, diagnostics)?;
    write(&file, &source)?;
    Ok(file)
}

fn override_file(app: &App, key: &str) -> Result<PathBuf, GenError> {
    let usage = || {
        let keys = built_in_keys(app);
        let known = if keys.is_empty() {
            "the app has none: a module's `routes.resource(…)` routes the built-in screens"
                .to_owned()
        } else {
            keys.join(", ")
        };
        GenError::Usage(format!(
            "`{key}` names no built-in screen of a module; name one as `<module>/<area>/<screen>`: \
             {known}"
        ))
    };
    let [module, area, name] = key.split('/').collect::<Vec<_>>()[..] else {
        return Err(usage());
    };
    let contribution = app
        .modules
        .iter()
        .filter(|candidate| candidate.krate.crate_name == module)
        .flat_map(|candidate| &candidate.areas)
        .find(|contribution| contribution.area == area && routes_built_in(contribution, name))
        .ok_or_else(usage)?;
    let file = contribution.screen_file(name);
    if file.exists() {
        return Err(GenError::Usage(format!(
            "{} overrides `{key}` already; edit it",
            file.display()
        )));
    }
    Ok(file)
}

fn routes_built_in(area: &AreaContribution, screen: &str) -> bool {
    area.routes
        .iter()
        .any(|route| route.is_resource() && route.screen.as_deref() == Some(screen))
}

fn built_in_keys(app: &App) -> Vec<String> {
    let mut keys: Vec<String> = app
        .modules
        .iter()
        .flat_map(|module: &Module| {
            module.areas.iter().flat_map(move |area| {
                area.routes
                    .iter()
                    .filter(|route| route.is_resource())
                    .filter_map(move |route| {
                        let screen = route.screen.as_deref()?;
                        Some(format!(
                            "{}/{}/{screen}",
                            module.krate.crate_name, area.area
                        ))
                    })
            })
        })
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

fn built_in_source(root: &Path, key: &str, diagnostics: Diagnostics) -> Result<String, GenError> {
    let executable = cargo::build(root, DEVELOPMENT, diagnostics)?;
    let output = child_process::output(
        diagnostics.process_group(),
        &mut source_command(root, &executable, key)?,
    )
    .map_err(|source| io_error(&executable, source))?;
    if !output.status.success() {
        return Err(GenError::App(format!(
            "`<app> screen source {key}` failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    String::from_utf8(output.stdout)
        .map_err(|_| GenError::App(format!("`<app> screen source {key}` printed no UTF-8")))
}

/// The app logs to stdout, where the source is read from.
fn source_command(root: &Path, executable: &Path, key: &str) -> Result<Command, GenError> {
    let mut command = app_command(root, executable)?;
    command
        .env("SOFIE_LOG", "off")
        .args(["screen", "source", key]);
    Ok(command)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::Path;

    use sofie_model::testing::{TemporaryDirectory, app, area, module, route};
    use sofie_model::{Method, Route};

    use super::{override_file, source_command};
    use crate::gen_command::GenError;

    fn resource_route(screen: &str) -> Route {
        let mut index = route(Method::Get, "/orders", "sofie::resource::index", 3);
        index.screen = Some(screen.to_owned());
        index
    }

    #[test]
    fn writes_over_a_built_in_screen_of_the_area_routing_it() {
        let mut order = module("order", "shop/modules/order");
        order
            .areas
            .push(area(&order, "console", vec![resource_route("order_index")]));
        let shop = app("shop", vec![order]);

        let file = override_file(&shop, "order/console/order_index").unwrap();
        let refused = override_file(&shop, "order/console/orders");

        assert_eq!(
            file,
            std::path::Path::new("shop/modules/order/src/areas/console/screens/order_index.html")
        );
        let Err(GenError::Usage(message)) = refused else {
            panic!("{refused:?}");
        };
        assert!(
            message.ends_with(": order/console/order_index"),
            "{message}"
        );
    }

    #[test]
    fn an_app_without_built_in_screens_is_told_where_they_come_from() {
        let shop = app("shop", vec![module("order", "shop/modules/order")]);

        let refused = override_file(&shop, "order/web/orders");

        let Err(GenError::Usage(message)) = refused else {
            panic!("{refused:?}");
        };
        assert!(
            message.ends_with(
                "the app has none: a module's `routes.resource(…)` routes the built-in screens"
            ),
            "{message}"
        );
    }

    #[test]
    fn runs_the_development_binary_in_the_app_root_with_its_env_file_and_no_logs() {
        let directory = TemporaryDirectory::new("sofie-scaffold-screen");
        directory.write(".env", "SOFIE_SCAFFOLD_TEST_KEY=local\n");

        let command = source_command(directory.root(), Path::new("app"), "a/b/c").unwrap();

        assert_eq!(command.get_current_dir(), Some(directory.root()));
        let variables: Vec<_> = command.get_envs().collect();
        assert!(variables.contains(&(
            OsStr::new("SOFIE_SCAFFOLD_TEST_KEY"),
            Some(OsStr::new("local"))
        )));
        assert!(variables.contains(&(OsStr::new("SOFIE_LOG"), Some(OsStr::new("off")))));
        let arguments: Vec<_> = command.get_args().collect();
        assert_eq!(arguments, ["screen", "source", "a/b/c"]);
    }
}
