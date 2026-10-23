use std::path::Path;

use crate::cargo::{self, Diagnostics};
use crate::child_process;
use crate::dev_command::{DEVELOPMENT, app_command};
use crate::gen_command::{GenError, io_error};
use crate::local_app::refuse_remote_database;

/// Queues the schedule's job through the app's development binary, the only one knowing the
/// worker's schedules; the worker `sofie dev` runs takes it next, whatever its interval.
pub fn run_schedule(root: &Path, name: &str) -> Result<String, GenError> {
    run_schedule_with(root, name, Diagnostics::Terminal)
}

pub(crate) fn run_schedule_with(
    root: &Path,
    name: &str,
    diagnostics: Diagnostics,
) -> Result<String, GenError> {
    refuse_remote_database(root, "schedule run")?;
    let executable = cargo::build(root, DEVELOPMENT, diagnostics)?;
    let output = child_process::output(
        diagnostics.process_group(),
        app_command(root, &executable)?.args(["schedule", "run", name]),
    )
    .map_err(|source| io_error(&executable, source))?;
    if !output.status.success() {
        return Err(GenError::App(format!(
            "`schedule run {name}` failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .to_owned())
}
