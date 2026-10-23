use std::env;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Exactly as `cargo fmt` formats `file`, so a generated file survives `cargo fmt --all` and
/// `sofie gen --check` stays stable: rustfmt reads its config from the file's folder up,
/// never from the caller's.
pub fn format_rust(source: &str, file: &Path) -> Result<String, String> {
    let rustfmt = env::var_os("RUSTFMT").unwrap_or_else(|| "rustfmt".into());
    let folder = file
        .ancestors()
        .skip(1)
        .find(|folder| folder.is_dir())
        .unwrap_or(Path::new("."));
    let mut child = Command::new(&rustfmt)
        .args(["--edition", "2024", "--emit", "stdout"])
        .current_dir(folder)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            format!(
                "cannot run `{}`: {error}; install it with `rustup component add rustfmt` or set RUSTFMT",
                rustfmt.display()
            )
        })?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(source.as_bytes())
            .map_err(|error| format!("writing to rustfmt failed: {error}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("waiting for rustfmt failed: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "rustfmt rejected the generated code: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("rustfmt printed invalid UTF-8: {error}"))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use sofie_model::testing::TemporaryDirectory;

    use super::format_rust;

    #[test]
    fn formats_like_cargo_fmt() {
        let formatted =
            format_rust("fn  main( ) { let x=vec![1,\n2]; }\n", Path::new("main.rs")).unwrap();

        assert_eq!(formatted, "fn main() {\n    let x = vec![1, 2];\n}\n");
    }

    #[test]
    fn reads_the_config_above_the_file_not_above_the_caller() {
        let directory = TemporaryDirectory::new("rustfmt-config");
        directory.write("rustfmt.toml", "tab_spaces = 2\n");

        let formatted = format_rust(
            "fn main() { let x = 1; }\n",
            &directory.path("src/not_yet_written/wiring.rs"),
        )
        .unwrap();

        assert_eq!(formatted, "fn main() {\n  let x = 1;\n}\n");
    }

    #[test]
    fn refuses_invalid_rust() {
        let error = format_rust("fn main( {", Path::new("main.rs")).unwrap_err();

        assert!(
            error.starts_with("rustfmt rejected the generated code"),
            "{error}"
        );
    }
}
