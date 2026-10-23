//! Read-only git commands only: `changes` reviews a working tree, it never alters one.

use std::fmt::Write as _;
use std::fs;
use std::path::{Component, Path};
use std::process::{Command, Stdio};

use super::ToolOutput;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ChangeKind {
    Added,
    Modified,
    Renamed,
    Deleted,
    Conflicted,
}

impl ChangeKind {
    fn name(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Renamed => "renamed",
            Self::Deleted => "deleted",
            Self::Conflicted => "conflicted",
        }
    }

    /// From the two status letters of `git status --porcelain`: index, then working tree.
    fn of(status: &str) -> Self {
        let has = |letter| status.contains(letter);
        if has('U') || status == "AA" || status == "DD" {
            return Self::Conflicted;
        }
        if has('D') {
            return Self::Deleted;
        }
        if has('R') {
            return Self::Renamed;
        }
        if status == "??" || has('A') || has('C') {
            return Self::Added;
        }
        Self::Modified
    }
}

struct FileChange {
    kind: ChangeKind,
    /// Relative to the app root.
    path: String,
    renamed_from: Option<String>,
}

pub(super) fn changes(root: &Path, path: Option<&str>) -> ToolOutput {
    match (working_tree(root), path) {
        (Err(error), _) => ToolOutput::failed(error),
        (Ok(changes), None) => ToolOutput::done(change_list(&changes)),
        (Ok(changes), Some(path)) => file_diff(root, &changes, path),
    }
}

/// Relative to the app root: the files added, modified or renamed since the last commit.
pub(super) fn changed_files(root: &Path) -> Result<Vec<String>, String> {
    Ok(working_tree(root)?
        .into_iter()
        .filter(|change| change.kind != ChangeKind::Deleted)
        .map(|change| change.path)
        .collect())
}

fn working_tree(root: &Path) -> Result<Vec<FileChange>, String> {
    let prefix = git(root, &["rev-parse", "--show-prefix"])?;
    let status = git(
        root,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--",
            ".",
        ],
    )?;
    Ok(file_changes(&status, prefix.trim_end()))
}

fn git(root: &Path, arguments: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| {
            format!("running `git` failed: {error}; `changes` reads git's working tree")
        })?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("not a git repository") {
        return Err(
            "the app is not in a git repository, so `changes` has nothing to compare with; \
             `git init` and a first commit start one"
                .to_owned(),
        );
    }
    Err(format!(
        "`git {}` failed: {}",
        arguments.join(" "),
        stderr.trim()
    ))
}

/// `status` is `-z` output, whose paths are relative to the repository root, so `prefix`
/// (the app root's path within it) is taken off. A rename's or copy's entry is followed by
/// its source path.
fn file_changes(status: &str, prefix: &str) -> Vec<FileChange> {
    let mut entries = status.split('\0').filter(|entry| !entry.is_empty());
    let mut changes = Vec::new();
    while let Some(entry) = entries.next() {
        let (Some(letters), Some(path)) = (entry.get(..2), entry.get(3..)) else {
            continue;
        };
        let relative = |path: &str| path.strip_prefix(prefix).unwrap_or(path).to_owned();
        let source = letters
            .contains(['R', 'C'])
            .then(|| entries.next().map(relative))
            .flatten();
        let kind = ChangeKind::of(letters);
        changes.push(FileChange {
            kind,
            path: relative(path),
            renamed_from: source.filter(|_| kind == ChangeKind::Renamed),
        });
    }
    changes.sort_by(|left, right| (left.kind, &left.path).cmp(&(right.kind, &right.path)));
    changes
}

fn change_list(changes: &[FileChange]) -> String {
    if changes.is_empty() {
        return "no changes under the app root since the last commit".to_owned();
    }
    let mut text = String::new();
    let mut kinds: Vec<ChangeKind> = changes.iter().map(|change| change.kind).collect();
    kinds.dedup();
    for kind in kinds {
        let files: Vec<&FileChange> = changes
            .iter()
            .filter(|change| change.kind == kind)
            .collect();
        let _ = writeln!(text, "{} ({}):", kind.name(), files.len());
        for file in files {
            match &file.renamed_from {
                Some(from) => {
                    let _ = writeln!(text, "  {from} -> {}", file.path);
                }
                None => {
                    let _ = writeln!(text, "  {}", file.path);
                }
            }
        }
    }
    text.push_str("`changes` with `path` shows one file's diff");
    text
}

fn file_diff(root: &Path, changes: &[FileChange], path: &str) -> ToolOutput {
    let inside = Path::new(path)
        .components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir));
    if !inside || path.is_empty() {
        return ToolOutput::failed(format!(
            "`path` is relative to the app root and stays inside it, as `app/src/lib.rs`, not \
             `{path}`"
        ));
    }
    let Some(change) = changes.iter().find(|change| change.path == path) else {
        return ToolOutput::failed(format!(
            "`{path}` has no changes; `changes` without `path` lists the changed files"
        ));
    };
    if change.kind == ChangeKind::Added {
        return new_file(root, path);
    }
    let mut arguments = vec![
        "diff",
        "--no-color",
        "--no-ext-diff",
        "-M",
        "HEAD",
        "--",
        path,
    ];
    arguments.extend(change.renamed_from.as_deref());
    let diffed = git(root, &arguments);
    match diffed {
        Ok(diff) if diff.is_empty() => {
            ToolOutput::done(format!("`{path}`: {}", change.kind.name()))
        }
        Ok(diff) => ToolOutput::done(diff),
        Err(error) => ToolOutput::failed(error),
    }
}

/// Git has no earlier version of an untracked file to diff against.
fn new_file(root: &Path, path: &str) -> ToolOutput {
    match fs::read_to_string(root.join(path)) {
        Ok(text) => ToolOutput::done(format!("new file {path}:\n{text}")),
        Err(error) => ToolOutput::failed(format!("the new file {path} cannot be read: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_source_path_and_a_copy_keep_the_entries_after_them_in_place() {
        let status = "R  app/lib.rs\0app/a\0C  app/copy.rs\0app/b.rs\0 M app/main.rs\0";

        let changes = file_changes(status, "app/");

        let listed: Vec<(&str, &str, Option<&str>)> = changes
            .iter()
            .map(|change| {
                (
                    change.kind.name(),
                    change.path.as_str(),
                    change.renamed_from.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            listed,
            [
                ("added", "copy.rs", None),
                ("modified", "main.rs", None),
                ("renamed", "lib.rs", Some("a")),
            ]
        );
    }
}
