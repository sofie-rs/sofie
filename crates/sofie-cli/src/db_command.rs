use std::path::Path;
use std::process::ExitStatus;

use crate::dev_command::run_development;
use crate::gen_command::GenError;

/// Run through the app's own binary, the only one embedding the migrations; production binaries leave the command out.
pub fn fresh_database(root: &Path) -> Result<ExitStatus, GenError> {
    run_development(root, &["db", "fresh"])
}
