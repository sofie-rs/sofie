use std::path::Path;

use serde_json::{Map, Value};

use super::ToolOutput;
use crate::cargo::{self, Diagnostics};
use crate::child_process::{self, ProcessGroup};
use crate::dev_command::{DEVELOPMENT, app_command};
use crate::gen_command::{GenError, io_error};
use crate::local_app::refuse_remote_database;

pub(super) fn run(root: &Path, name: &str, arguments: Option<&str>) -> ToolOutput {
    let words = match arguments.map_or(Ok(Vec::new()), command_line) {
        Ok(words) => words,
        Err(message) => return ToolOutput::failed(message),
    };
    let mut command = vec!["task".to_owned(), name.to_owned()];
    command.extend(words);
    run_app(root, &command, &format!("<app> task {name}"))
}

pub(super) fn db_status(root: &Path) -> ToolOutput {
    run_app(
        root,
        &["db".to_owned(), "status".to_owned()],
        "<app> db status",
    )
}

/// The development binary once, with the app's `.env`, against its database.
fn run_app(root: &Path, command: &[String], shown: &str) -> ToolOutput {
    match run_binary(root, command) {
        Ok((text, succeeded)) => ToolOutput::judged(text, succeeded).with_rest(shown.to_owned()),
        Err(error) => ToolOutput::from_error(root, &error),
    }
}

fn run_binary(root: &Path, command: &[String]) -> Result<(String, bool), GenError> {
    refuse_remote_database(root, "mcp")?;
    let executable = cargo::build(root, DEVELOPMENT, Diagnostics::Captured)?;
    let output = child_process::output(
        ProcessGroup::Own,
        app_command(root, &executable)?.args(command),
    )
    .map_err(|source| io_error(&executable, source))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok((text, output.status.success()))
}

/// Passed as separate process arguments, never through a shell.
fn command_line(arguments: &str) -> Result<Vec<String>, String> {
    let refused = || {
        "`arguments` is a JSON object of argument names to strings, numbers or booleans, as \
         `{\"since\": \"2026-01-01\"}`"
            .to_owned()
    };
    let object: Map<String, Value> = serde_json::from_str(arguments).map_err(|_| refused())?;
    let mut words = Vec::new();
    for (name, value) in object {
        let value = match value {
            Value::String(text) => text,
            Value::Number(number) => number.to_string(),
            Value::Bool(flag) => flag.to_string(),
            _ => return Err(refused()),
        };
        words.extend([format!("--{name}"), value]);
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::command_line;

    #[test]
    fn arguments_become_named_pairs_and_anything_but_an_object_of_scalars_is_refused() {
        assert_eq!(
            command_line(r#"{"since": "2026-01-01", "limit": 5, "dry_run": true}"#).unwrap(),
            ["--dry_run", "true", "--limit", "5", "--since", "2026-01-01"]
        );
        for refused in [
            r#"["since"]"#,
            r#"{"since": null}"#,
            r#"{"since": {"day": 1}}"#,
            "since",
        ] {
            assert!(command_line(refused).is_err(), "{refused}");
        }
    }
}
