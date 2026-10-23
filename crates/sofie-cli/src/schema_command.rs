use std::path::Path;
use std::process::ExitStatus;

use crate::dev_command::run_development;
use crate::gen_command::GenError;

/// Run through the app's own binary, the only one knowing its aggregates.
pub fn check_schema(root: &Path) -> Result<ExitStatus, GenError> {
    run_development(root, &["schema", "check"])
}
