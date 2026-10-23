use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use sofie_check::{Finding, Rule};
use sofie_model::{App, ConfigKey};

use crate::dev_command::env_entries;
use crate::env_example::{framework_keys, module_keys, remote_keys};
use crate::gen_command::{GenError, io_error};

/// Values are never repeated, as they may be secrets.
pub fn check_config(root: &Path, environment: &str) -> Result<Vec<Finding>, GenError> {
    let app = sofie_model::load(root).map_err(GenError::Model)?;
    let file = root.join(format!(".env.{environment}"));
    let text = fs::read_to_string(&file).map_err(|source| io_error(&file, source))?;
    let values = EnvironmentFile::parse(&file, &text);
    Ok(app_keys(&app)
        .iter()
        .filter_map(|key| values.problem(key))
        .collect())
}

fn app_keys(app: &App) -> Vec<ConfigKey> {
    let modules = app.modules.iter().flat_map(module_keys);
    let remote = app
        .split
        .remote
        .iter()
        .flat_map(|remote| remote_keys(&remote.name));
    framework_keys(app)
        .into_iter()
        .chain(modules)
        .chain(remote)
        .collect()
}

struct EnvironmentFile {
    file: PathBuf,
    values: BTreeMap<String, (String, usize)>,
}

impl EnvironmentFile {
    fn parse(file: &Path, text: &str) -> Self {
        let values = env_entries(text)
            .map(|(line, key, value)| (key.to_owned(), (value.to_owned(), line)))
            .collect();
        Self {
            file: file.to_path_buf(),
            values,
        }
    }

    fn problem(&self, key: &ConfigKey) -> Option<Finding> {
        let Some((value, line)) = self.values.get(&key.name) else {
            return self.missing(key);
        };
        if key.kind.accepts(value) {
            return None;
        }
        Some(self.finding(
            *line,
            format!("{} is not {}", key.name, key.kind.description()),
            format!("set {} to {}", key.name, key.kind.description()),
        ))
    }

    fn missing(&self, key: &ConfigKey) -> Option<Finding> {
        let inherited = key
            .fallback
            .as_ref()
            .is_some_and(|fallback| self.values.contains_key(fallback));
        if !key.required || inherited {
            return None;
        }
        let documented = key
            .doc
            .first()
            .map_or(String::new(), |doc| format!(": {doc}"));
        Some(self.finding(
            1,
            format!(
                "{} is not set, and the app cannot start without it",
                key.name
            ),
            format!("add `{}=…`{documented}", key.name),
        ))
    }

    fn finding(&self, line: usize, message: String, fix: String) -> Finding {
        Finding {
            rule: Rule::Config,
            file: self.file.clone(),
            line,
            message,
            fix,
        }
    }
}
