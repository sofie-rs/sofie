use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::cargo_rustflags::config_rustflags;
use crate::child_process::{ChildProcess, ProcessGroup};
use crate::gen_command::{GenError, io_error};

pub(crate) fn command() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

pub(crate) fn app_manifest(root: &Path) -> PathBuf {
    root.join("app").join("Cargo.toml")
}

#[derive(Clone, Copy)]
pub(crate) enum Diagnostics {
    Terminal,
    /// For `sofie mcp`, whose stdout is the protocol: a failed build answers the
    /// compiler's errors, bounded, instead of printing them.
    #[cfg(feature = "mcp")]
    Captured,
}

impl Diagnostics {
    /// A child in the terminal gets its Ctrl-C there; `sofie mcp` installs no terminal and
    /// stops a child with whatever it started.
    pub(crate) fn process_group(self) -> ProcessGroup {
        match self {
            Self::Terminal => ProcessGroup::Inherited,
            #[cfg(feature = "mcp")]
            Self::Captured => ProcessGroup::Own,
        }
    }
}

pub(crate) fn build(
    root: &Path,
    arguments: &[&str],
    diagnostics: Diagnostics,
) -> Result<PathBuf, GenError> {
    build_with(root, arguments, diagnostics, command())
}

/// Builds for `host` with `flags` after those cargo would read (the environment's,
/// else its config's, which flags in the environment would replace), and `c_flags`
/// after any for the C and C++ that crates' build scripts compile.
pub(crate) fn build_with_rustflags(
    root: &Path,
    arguments: &[&str],
    host: &str,
    flags: &[String],
    c_flags: &str,
) -> Result<PathBuf, GenError> {
    let given = match std::env::var("CARGO_ENCODED_RUSTFLAGS") {
        Ok(encoded) if !encoded.is_empty() => encoded.split('\x1f').map(str::to_owned).collect(),
        _ => std::env::var("RUSTFLAGS")
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>(),
    };
    let mut all = if given.is_empty() {
        let directory = std::env::current_dir().map_err(|source| io_error(root, source))?;
        config_rustflags(&directory, host)?
    } else {
        given
    };
    all.extend_from_slice(flags);
    let mut cargo = command();
    cargo
        .env_remove("RUSTFLAGS")
        .env("CARGO_ENCODED_RUSTFLAGS", all.join("\x1f"));
    for variable in ["CFLAGS", "CXXFLAGS"] {
        let given = std::env::var(variable).unwrap_or_default();
        cargo.env(variable, format!("{given} {c_flags}").trim());
    }
    build_with(root, arguments, Diagnostics::Terminal, cargo)
}

fn build_with(
    root: &Path,
    arguments: &[&str],
    diagnostics: Diagnostics,
    mut cargo: Command,
) -> Result<PathBuf, GenError> {
    let manifest = app_manifest(root);
    let stderr = match diagnostics {
        Diagnostics::Terminal => Stdio::inherit(),
        #[cfg(feature = "mcp")]
        Diagnostics::Captured => Stdio::piped(),
    };
    let mut stdout = None;
    let mut stderr_pipe = None;
    let process = ChildProcess::spawn(diagnostics.process_group(), &mut cargo, |cargo| {
        let mut child = cargo
            .arg("build")
            .arg("--manifest-path")
            .arg(&manifest)
            .args(arguments)
            .arg("--message-format=json-render-diagnostics")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .spawn()?;
        stdout = child.stdout.take();
        stderr_pipe = child.stderr.take();
        Ok(child)
    })
    .map_err(|source| io_error(&manifest, source))?;
    // Read beside stdout, so neither pipe fills while the other is read.
    let rendered = stderr_pipe.map(|mut stderr| {
        std::thread::spawn(move || {
            let mut text = String::new();
            stderr.read_to_string(&mut text).map(|_| text)
        })
    });
    let mut executable = None;
    if let Some(stdout) = stdout {
        for line in BufReader::new(stdout).lines() {
            let line = line.map_err(|source| io_error(&manifest, source))?;
            executable = executable_of(&line).or(executable);
        }
    }
    let status = process
        .wait()
        .map_err(|source| io_error(&manifest, source))?;
    let rendered = match rendered.map(std::thread::JoinHandle::join) {
        None => String::new(),
        Some(Ok(read)) => read.map_err(|source| io_error(&manifest, source))?,
        Some(Err(_)) => return Err(GenError::App("reading cargo's errors panicked".to_owned())),
    };
    if !status.success() {
        let failure = format!("`cargo build` failed with {status}\n{rendered}");
        return Err(GenError::Build(match diagnostics {
            Diagnostics::Terminal => failure.trim_end().to_owned(),
            #[cfg(feature = "mcp")]
            Diagnostics::Captured => crate::mcp::build_failure(&failure),
        }));
    }
    executable.ok_or_else(|| GenError::App("`cargo build` built no executable".to_owned()))
}

fn executable_of(message: &str) -> Option<PathBuf> {
    let message: serde_json::Value = serde_json::from_str(message).ok()?;
    message["executable"].as_str().map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::executable_of;

    #[test]
    fn reads_the_executable_cargo_built() {
        let message = r#"{"reason":"compiler-artifact","executable":"/t/debug/my \"app\""}"#;

        assert_eq!(
            executable_of(message),
            Some(PathBuf::from("/t/debug/my \"app\""))
        );
        assert_eq!(executable_of(r#"{"executable":null}"#), None);
    }
}
