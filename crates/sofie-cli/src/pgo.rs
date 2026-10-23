//! Profile-guided optimization: an instrumented build runs under `sofie bench`'s
//! load, then the binary is rebuilt with the recorded profile.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sofie_model::App;

use crate::bench::{Scripts, serve};
use crate::cargo;
use crate::gen_command::{GenError, io_error};
use crate::local_app::{private_directory, refuse_remote_database};
use crate::source_edits::files_with_extension;

/// The `cc` crate hands the profile flags to the C compiler too, whose profile
/// format need not be rustc's: mixed in one binary, they corrupt the profile
/// (seen with Apple clang), so C and C++ stay unprofiled.
const NO_C_PROFILE: &str = "-fno-profile-generate -fno-profile-use";

/// With an explicit target, the flags reach only the app's code, not build
/// scripts and macros, and the build lands in `target/<host>/dist`, so the
/// ordinary build is not invalidated.
pub(crate) fn build(root: &Path, app: &App, arguments: &[&str]) -> Result<PathBuf, GenError> {
    refuse_remote_database(root, "build --pgo")?;
    let rustc = Rustc::find()?;
    let profiles = Profiles::create()?;
    let mut arguments = arguments.to_vec();
    arguments.extend(["--target", &rustc.host]);
    let instrumented = cargo::build_with_rustflags(
        root,
        &arguments,
        &rustc.host,
        &[format!(
            "-Cprofile-generate={}",
            profiles.directory.display()
        )],
        NO_C_PROFILE,
    )?;
    record(root, app, &instrumented, &profiles.directory)?;
    let merged = profiles.merge(&rustc.profdata)?;
    cargo::build_with_rustflags(
        root,
        &arguments,
        &rustc.host,
        &[format!("-Cprofile-use={}", merged.display())],
        NO_C_PROFILE,
    )
}

fn record(root: &Path, app: &App, binary: &Path, profiles: &Path) -> Result<(), GenError> {
    let scripts = Scripts::write(root, app)?;
    let pattern = profiles.join("%m_%p.profraw");
    let running = serve(
        root,
        binary,
        "local",
        &[("LLVM_PROFILE_FILE", pattern.as_os_str())],
    )?;
    scripts.run(&running.target)?;
    running.stop()
}

struct Rustc {
    host: String,
    profdata: PathBuf,
}

impl Rustc {
    fn find() -> Result<Self, GenError> {
        let version = rustc(&["-vV"])?;
        let host = version
            .lines()
            .find_map(|line| line.strip_prefix("host: "))
            .ok_or_else(|| GenError::App(format!("`rustc -vV` names no host:\n{version}")))?
            .to_owned();
        let sysroot = rustc(&["--print", "sysroot"])?;
        let profdata = Path::new(sysroot.trim())
            .join("lib/rustlib")
            .join(&host)
            .join("bin/llvm-profdata");
        if !profdata.is_file() {
            return Err(GenError::Usage(format!(
                "{} is missing; install it with `rustup component add llvm-tools`",
                profdata.display()
            )));
        }
        Ok(Self { host, profdata })
    }
}

fn rustc(arguments: &[&str]) -> Result<String, GenError> {
    let program = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = Command::new(&program)
        .args(arguments)
        .output()
        .map_err(|source| io_error(Path::new(&program), source))?;
    if !output.status.success() {
        return Err(GenError::App(format!(
            "`rustc {}` failed with {}",
            arguments.join(" "),
            output.status
        )));
    }
    String::from_utf8(output.stdout)
        .map_err(|_| GenError::App(format!("`rustc {}` printed non-UTF-8", arguments.join(" "))))
}

/// Removed when dropped: a profile is only valid for the build it was recorded on.
struct Profiles {
    directory: PathBuf,
}

impl Profiles {
    fn create() -> Result<Self, GenError> {
        Ok(Self {
            directory: private_directory("sofie-pgo")?,
        })
    }

    fn merge(&self, profdata: &Path) -> Result<PathBuf, GenError> {
        let raw = files_with_extension(&self.directory, "profraw")?;
        if raw.is_empty() {
            return Err(GenError::App(format!(
                "the instrumented app wrote no profile into {}",
                self.directory.display()
            )));
        }
        let merged = self.directory.join("merged.profdata");
        let status = Command::new(profdata)
            .arg("merge")
            .arg("-o")
            .arg(&merged)
            .args(&raw)
            .status()
            .map_err(|source| io_error(profdata, source))?;
        if !status.success() {
            return Err(GenError::App(format!(
                "`llvm-profdata merge` failed with {status}"
            )));
        }
        Ok(merged)
    }
}

impl Drop for Profiles {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.directory) {
            eprintln!(
                "sofie build --pgo: removing {}: {error}",
                self.directory.display()
            );
        }
    }
}
