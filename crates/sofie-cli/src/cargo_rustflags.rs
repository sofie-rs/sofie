//! The rustflags cargo would read from its config files, which flags given
//! through the environment replace rather than join.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::gen_command::{GenError, io_error};

/// `build.rustflags`, or `target.<host>.rustflags` when set, which cargo then
/// reads instead; with an explicit `--target` both reach only the app's code.
pub(crate) fn config_rustflags(start: &Path, host: &str) -> Result<Vec<String>, GenError> {
    read_rustflags(&config_files(start), host)
}

fn read_rustflags(files: &[PathBuf], host: &str) -> Result<Vec<String>, GenError> {
    let mut build = Vec::new();
    let mut target = Vec::new();
    // Ancestors first, so a deeper directory's flags come later, as cargo joins them.
    for file in files.iter().rev() {
        let text = match fs::read_to_string(file) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => return Err(io_error(file, error)),
        };
        let found = Found::read(&text, host)
            .map_err(|message| GenError::Usage(format!("{}: {message}", file.display())))?;
        build.extend(found.build);
        target.extend(found.target);
    }
    let host_variable = format!(
        "CARGO_TARGET_{}_RUSTFLAGS",
        host.to_uppercase().replace(['-', '.'], "_")
    );
    build.extend(environment_flags("CARGO_BUILD_RUSTFLAGS"));
    target.extend(environment_flags(&host_variable));
    Ok(if target.is_empty() { build } else { target })
}

fn environment_flags(variable: &str) -> Vec<String> {
    std::env::var(variable)
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

/// Deepest first, then cargo's home.
fn config_files(start: &Path) -> Vec<PathBuf> {
    let mut directories: Vec<PathBuf> = start
        .ancestors()
        .map(|directory| directory.join(".cargo"))
        .collect();
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".cargo")));
    if let Some(home) = home.filter(|home| !directories.contains(home)) {
        directories.push(home);
    }
    directories
        .into_iter()
        .flat_map(|directory| [directory.join("config.toml"), directory.join("config")])
        .collect()
}

#[derive(Default)]
struct Found {
    build: Vec<String>,
    target: Vec<String>,
}

impl Found {
    /// Reads the `rustflags` keys of `[build]` and `[target.<triple>]` tables
    /// and their dotted forms; a `cfg(…)` target's flags cannot be matched
    /// without evaluating it, so they are refused rather than dropped.
    fn read(text: &str, host: &str) -> Result<Self, String> {
        let mut found = Self::default();
        let mut table = String::new();
        let mut lines = text.lines();
        while let Some(line) = lines.next() {
            let line = line.trim();
            if let Some(header) = line
                .strip_prefix('[')
                .and_then(|rest| rest.strip_suffix(']'))
            {
                table = unquoted(header.trim());
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = unquoted(key.trim());
            let full = if table.is_empty() {
                key
            } else {
                format!("{table}.{key}")
            };
            let Some(owner) = full.strip_suffix(".rustflags") else {
                continue;
            };
            let mut value = value.trim().to_owned();
            while value.starts_with('[') && !value.contains(']') {
                let Some(next) = lines.next() else { break };
                value.push_str(next.trim());
            }
            let flags = flags(&value);
            match owner.strip_prefix("target.") {
                None if owner == "build" => found.build.extend(flags),
                Some(triple) if triple == host => found.target.extend(flags),
                Some(cfg) if cfg.starts_with("cfg(") => {
                    return Err(format!(
                        "`sofie build --pgo` cannot tell whether `target.'{cfg}'.rustflags` \
                         applies; move those flags to `target.{host}.rustflags` or RUSTFLAGS"
                    ));
                }
                _ => {}
            }
        }
        Ok(found)
    }
}

fn unquoted(key: &str) -> String {
    key.chars()
        .filter(|character| !matches!(character, '"' | '\''))
        .collect()
}

/// A string splits at whitespace, as cargo splits one; an array holds one flag per string.
fn flags(value: &str) -> Vec<String> {
    let value = value.split(" #").next().unwrap_or_default().trim();
    let strings = quoted_strings(value);
    if value.starts_with('[') {
        return strings;
    }
    strings
        .iter()
        .flat_map(|text| text.split_whitespace())
        .map(str::to_owned)
        .collect()
}

fn quoted_strings(value: &str) -> Vec<String> {
    let mut strings = Vec::new();
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if matches!(character, '"' | '\'') {
            strings.push(
                characters
                    .by_ref()
                    .take_while(|next| *next != character)
                    .collect(),
            );
        }
    }
    strings
}

#[cfg(test)]
mod tests {
    use sofie_model::testing::TemporaryDirectory;

    use super::*;

    const HOST: &str = "aarch64-apple-darwin";

    #[test]
    fn joins_the_flags_of_each_config_file_deeper_ones_last() {
        let directory = TemporaryDirectory::new("sofie-rustflags");
        directory.write(
            ".cargo/config.toml",
            "[build]\nrustflags = [\"-C\", \"target-cpu=native\"]\n",
        );
        directory.write(
            "app/.cargo/config.toml",
            "build.rustflags = \"--cfg tokio_unstable\" # tracing\n",
        );

        let files = [
            directory.path("app/.cargo/config.toml"),
            directory.path(".cargo/config.toml"),
            directory.path("missing/.cargo/config.toml"),
        ];

        let flags = read_rustflags(&files, HOST).unwrap();

        assert_eq!(
            flags,
            ["-C", "target-cpu=native", "--cfg", "tokio_unstable"]
        );
    }

    #[test]
    fn the_host_s_target_flags_replace_the_build_flags() {
        let found = Found::read(
            "[build]\nrustflags = [\"-Dwarnings\"]\n\n[target.aarch64-apple-darwin]\nrustflags = [\n  \"-C\",\n  \"link-arg=-fuse-ld=lld\",\n]\n\n[target.x86_64-unknown-linux-gnu]\nrustflags = [\"-Cother\"]\n",
            HOST,
        )
        .unwrap();

        assert_eq!(found.build, ["-Dwarnings"]);
        assert_eq!(found.target, ["-C", "link-arg=-fuse-ld=lld"]);
    }

    #[test]
    fn a_cfg_target_s_flags_are_refused() {
        let error = Found::read(
            "[target.'cfg(target_os = \"macos\")']\nrustflags = [\"-Cx\"]\n",
            HOST,
        )
        .err()
        .unwrap_or_default();

        assert!(error.contains("cannot tell whether"), "{error}");
    }
}
