use std::path::Path;

use sofie_model::MacroCall;

use super::{Finding, Rule};

const ENVIRONMENT_FUNCTIONS: [&str; 4] = ["var", "vars", "var_os", "vars_os"];
const ENVIRONMENT_MACROS: [&str; 2] = ["env", "option_env"];
/// Cargo sets these at compile time; they are build facts, not configuration.
const COMPILE_TIME_PREFIX: &str = "CARGO_";

/// Only reading functions: `env::args`, `current_dir` and `consts::OS` are not configuration.
pub(super) fn path_reads_environment(segments: &[String]) -> bool {
    segments
        .windows(2)
        .any(|pair| pair[0] == "env" && ENVIRONMENT_FUNCTIONS.contains(&pair[1].as_str()))
}

pub(super) fn macro_reads_environment(call: &MacroCall) -> bool {
    let is_compile_time_constant = call
        .first_string
        .as_deref()
        .is_some_and(|key| key.starts_with(COMPILE_TIME_PREFIX));
    ENVIRONMENT_MACROS.contains(&call.name()) && !is_compile_time_constant
}

pub(super) fn finding(file: &Path, line: usize, written: &str) -> Finding {
    Finding {
        rule: Rule::Environment,
        file: file.to_path_buf(),
        line,
        message: format!(
            "`{written}` reads the environment directly, so the value bypasses the module's \
             config, its key prefix and its validation"
        ),
        fix: "declare the value in the module's config and read it from there".to_owned(),
    }
}
