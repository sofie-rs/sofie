mod cargo;
mod changes;
mod dry_run;
mod edit;
mod generate;
mod instructions;
mod protocol;
mod read;
mod request;
mod session;
mod task;
mod tools;
mod traffic;

use std::fmt::Write as _;
use std::io::{self, BufRead, Write};
use std::path::Path;

use crate::child_process::stop_on_interrupt;
use crate::gen_command::GenError;
pub(crate) use cargo::build_failure;

/// The most an answer holds, in bytes; a test run can print megabytes.
const MAX_ANSWER: usize = 16 * 1024;

/// Every tool reads the app afresh, so edits between calls show.
pub fn serve_mcp(root: &Path, input: impl BufRead, mut output: impl Write) -> io::Result<()> {
    let root = root.canonicalize().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("the app root {} cannot be read: {error}", root.display()),
        )
    })?;
    stop_on_interrupt()?;
    let mut session = session::DevSession::new(&root);
    for line in input.split(b'\n') {
        let line = line?;
        let response = match String::from_utf8(line) {
            Ok(line) if line.trim().is_empty() => continue,
            Ok(line) => protocol::answer(&root, &mut session, &line),
            Err(_) => Some(protocol::not_utf8()),
        };
        if let Some(response) = response {
            writeln!(output, "{response}")?;
            output.flush()?;
        }
    }
    Ok(())
}

pub(crate) struct ToolOutput {
    text: String,
    failed: bool,
    /// The command printing what a cut answer leaves out.
    rest: Option<String>,
}

impl ToolOutput {
    fn done(text: String) -> Self {
        Self::judged(text, true)
    }

    fn failed(text: String) -> Self {
        Self::judged(text, false)
    }

    fn judged(text: String, succeeded: bool) -> Self {
        Self {
            text,
            failed: !succeeded,
            rest: None,
        }
    }

    fn with_rest(mut self, rest: String) -> Self {
        self.rest = Some(rest);
        self
    }

    /// Paths are shown relative to the app root, and the answer is bounded only here.
    fn finished(mut self, root: &Path) -> Self {
        let shown = root.display().to_string();
        let relative = self
            .text
            .replace(&format!("{shown}/"), "")
            .replace(&shown, ".");
        self.text = bounded(&relative, self.rest.as_deref());
        self
    }

    fn from_error(root: &Path, error: &GenError) -> Self {
        match error {
            GenError::Check(findings) => Self::failed(sofie_check::report(root, findings)),
            GenError::Build(diagnostics) => {
                Self::failed(diagnostics.clone()).with_rest(cargo::build_rest())
            }
            other => Self::failed(other.to_string()),
        }
    }
}

/// `rest` is the command printing what was cut.
fn bounded(text: &str, rest: Option<&str>) -> String {
    if text.len() <= MAX_ANSWER {
        return text.to_owned();
    }
    let end = text.floor_char_boundary(MAX_ANSWER);
    let mut cut = format!(
        "{}\n… cut at {MAX_ANSWER} bytes of {}",
        &text[..end],
        text.len()
    );
    if let Some(rest) = rest {
        let _ = write!(cut, "; run `{rest}` for the rest");
    }
    cut
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{MAX_ANSWER, ToolOutput, bounded};

    #[test]
    fn a_long_answer_keeps_its_hint_for_the_rest() {
        let text = format!("{}\n", "/app/src/lib.rs: warning\n".repeat(MAX_ANSWER));

        let output = ToolOutput::judged(text, false)
            .with_rest("cargo test --offline".to_owned())
            .finished(Path::new("/app"));

        assert!(output.failed);
        assert!(
            output.text.starts_with("src/lib.rs: warning"),
            "{}",
            &output.text[..40]
        );
        assert!(
            output
                .text
                .ends_with("run `cargo test --offline` for the rest"),
            "{}",
            &output.text[output.text.len() - 120..]
        );
    }

    #[test]
    fn a_long_answer_is_cut_at_a_character_boundary_and_says_so() {
        let text = "é".repeat(MAX_ANSWER);

        let cut = bounded(&text, Some("cargo test --offline"));

        assert!(cut.len() < MAX_ANSWER + 200, "{}", cut.len());
        assert!(
            cut.ends_with("run `cargo test --offline` for the rest"),
            "{cut}"
        );
        assert_eq!(bounded("short", None), "short");
    }
}
