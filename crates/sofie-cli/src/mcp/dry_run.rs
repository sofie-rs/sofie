//! The generators reread the model through `cargo metadata` after each step, so only a real workspace copy shows what a sequence does.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sofie_model::naming::relative_path;

use crate::cargo;
use crate::child_process::{self, ProcessGroup};
use crate::dev_command::NOT_SOURCES;
use crate::gen_command::{GenError, io_error};
use crate::local_app::private_directory;

const SHOWN_LINES_PER_FILE: usize = 80;
/// A created file is read whole, as a scaffolded test file; the bound only guards against huge ones.
const SHOWN_LINES_PER_CREATED_FILE: usize = 600;
const CONTEXT_LINES: usize = 3;
const MAX_DIFF_CELLS: usize = 4_000_000;

pub(super) struct Change {
    path: PathBuf,
    before: Option<Vec<u8>>,
    after: Option<Vec<u8>>,
}

pub(super) fn dry_run<T>(
    root: &Path,
    run: impl FnOnce(&Path) -> T,
) -> Result<(T, Vec<Change>), GenError> {
    let (workspace, packages) = workspace_of(root)?;
    let relative = root.strip_prefix(&workspace).map_err(|_| {
        io_error(
            root,
            io::Error::other(format!(
                "the app is outside its Cargo workspace at {}",
                workspace.display()
            )),
        )
    })?;
    let scratch = Scratch::new()?;
    let mut sources = vec![root.to_path_buf()];
    sources.extend(packages);
    sources.extend(
        ["Cargo.toml", "Cargo.lock", ".cargo"]
            .map(|name| workspace.join(name))
            .into_iter()
            .filter(|path| path.exists()),
    );
    for source in outermost(sources) {
        let relative_source = source.strip_prefix(&workspace).map_err(|_| {
            io_error(
                &source,
                io::Error::other(format!(
                    "a workspace member outside the workspace root {} cannot be copied for a dry \
                     run; run without `dry_run`",
                    workspace.display()
                )),
            )
        })?;
        copy_tree(&source, &scratch.path.join(relative_source))?;
    }
    let copy = if relative.as_os_str().is_empty() {
        scratch.path.clone()
    } else {
        scratch.path.join(relative)
    };
    // The copy before the run, not the app after it: the app can change meanwhile, as
    // when `sofie dev` regenerates it, and would hide what the run changed. The whole
    // copied workspace, as a generator may edit its manifest.
    let before = files(&scratch.path)?;
    let result = run(&copy);
    let mut changes = compare(&before, &files(&scratch.path)?);
    for change in &mut changes {
        change.path = from_app(relative, &change.path);
    }
    scratch.remove()?;
    Ok((result, changes))
}

/// A path in the workspace as the app's directory reaches it, `../Cargo.toml` for its manifest.
fn from_app(app: &Path, path: &Path) -> PathBuf {
    relative_path(app, path).map_or_else(|| path.to_path_buf(), PathBuf::from)
}

fn workspace_of(root: &Path) -> Result<(PathBuf, Vec<PathBuf>), GenError> {
    let manifest = cargo::app_manifest(root);
    let mut command = cargo::command();
    command
        .args([
            "metadata",
            "--no-deps",
            "--offline",
            "--format-version",
            "1",
        ])
        .arg("--manifest-path")
        .arg(&manifest);
    let output = child_process::output(ProcessGroup::Own, &mut command)
        .map_err(|source| io_error(&manifest, source))?;
    let failed = |message: String| io_error(&manifest, io::Error::other(message));
    if !output.status.success() {
        return Err(failed(format!(
            "`cargo metadata` rejected the workspace: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let metadata: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| failed(format!("`cargo metadata` printed invalid JSON: {error}")))?;
    let workspace = metadata["workspace_root"]
        .as_str()
        .ok_or_else(|| failed("`cargo metadata` named no workspace root".to_owned()))?;
    let packages = metadata["packages"]
        .as_array()
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter_map(|package| package["manifest_path"].as_str())
        .filter_map(|manifest| Path::new(manifest).parent().map(Path::to_path_buf))
        .collect();
    Ok((canonical(Path::new(workspace))?, packages))
}

fn outermost(mut paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths.sort();
    let mut kept: Vec<PathBuf> = Vec::new();
    for path in paths {
        if !kept.iter().any(|outer| path.starts_with(outer)) {
            kept.push(path);
        }
    }
    kept
}

/// A link to a file is copied as that file; a linked directory, which may loop or lead
/// anywhere, a dangling link and a special file such as a FIFO are left out.
fn copy_tree(from: &Path, to: &Path) -> Result<(), GenError> {
    let kind = fs::symlink_metadata(from).map_err(|source| io_error(from, source))?;
    let followed = if kind.file_type().is_symlink() {
        match fs::metadata(from) {
            Ok(target) if target.is_file() => target,
            _ => return Ok(()),
        }
    } else {
        kind
    };
    if followed.is_file() {
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
        }
        fs::copy(from, to).map_err(|source| io_error(from, source))?;
        return Ok(());
    }
    if !followed.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(to).map_err(|source| io_error(to, source))?;
    for entry in fs::read_dir(from).map_err(|source| io_error(from, source))? {
        let entry = entry.map_err(|source| io_error(from, source))?;
        if NOT_SOURCES
            .iter()
            .any(|skipped| entry.file_name() == *skipped)
        {
            continue;
        }
        copy_tree(&entry.path(), &to.join(entry.file_name()))?;
    }
    Ok(())
}

fn files(directory: &Path) -> Result<BTreeMap<PathBuf, Vec<u8>>, GenError> {
    let mut found = BTreeMap::new();
    collect(directory, directory, &mut found)?;
    Ok(found)
}

fn collect(
    base: &Path,
    directory: &Path,
    found: &mut BTreeMap<PathBuf, Vec<u8>>,
) -> Result<(), GenError> {
    for entry in fs::read_dir(directory).map_err(|source| io_error(directory, source))? {
        let path = entry.map_err(|source| io_error(directory, source))?.path();
        if NOT_SOURCES.iter().any(|skipped| path.ends_with(skipped)) {
            continue;
        }
        if path.is_dir() {
            collect(base, &path, found)?;
            continue;
        }
        let bytes = fs::read(&path).map_err(|source| io_error(&path, source))?;
        let relative = path.strip_prefix(base).unwrap_or(&path).to_path_buf();
        found.insert(relative, bytes);
    }
    Ok(())
}

fn compare(before: &BTreeMap<PathBuf, Vec<u8>>, after: &BTreeMap<PathBuf, Vec<u8>>) -> Vec<Change> {
    let paths: BTreeSet<&PathBuf> = before.keys().chain(after.keys()).collect();
    paths
        .into_iter()
        .filter(|path| before.get(*path) != after.get(*path))
        .map(|path| Change {
            path: path.clone(),
            before: before.get(path).cloned(),
            after: after.get(path).cloned(),
        })
        .collect()
}

impl fmt::Display for Change {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let verb = match (&self.before, &self.after) {
            (None, _) => "create",
            (Some(_), Some(_)) => "change",
            (Some(_), None) => "remove",
        };
        write!(formatter, "{verb} {}", self.path.display())
    }
}

impl Change {
    /// Unified hunks with a few lines of context, cut at `SHOWN_LINES_PER_FILE` or, for a file
    /// created, `SHOWN_LINES_PER_CREATED_FILE`.
    pub(super) fn diff(&self) -> String {
        let (Some(before), Some(after)) =
            (lines(self.before.as_deref()), lines(self.after.as_deref()))
        else {
            return format!("{self} (not text)");
        };
        let script = edit_script(&before, &after);
        let mut lines = vec![self.to_string()];
        for range in hunk_ranges(&script) {
            lines.extend(hunk(&script, range));
        }
        let shown = if self.before.is_none() {
            SHOWN_LINES_PER_CREATED_FILE
        } else {
            SHOWN_LINES_PER_FILE
        };
        if lines.len() > shown + 1 {
            let more = lines.len() - shown - 1;
            lines.truncate(shown + 1);
            lines.push(format!("… {more} more lines"));
        }
        lines.join("\n")
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Edit {
    Kept,
    Removed,
    Added,
}

impl Edit {
    fn marker(self) -> char {
        match self {
            Self::Kept => ' ',
            Self::Removed => '-',
            Self::Added => '+',
        }
    }
}

type Script<'text> = Vec<(Edit, &'text str)>;

fn edit_script<'text>(before: &[&'text str], after: &[&'text str]) -> Script<'text> {
    let common_start = before
        .iter()
        .zip(after)
        .take_while(|(old, new)| old == new)
        .count();
    let common_end = before[common_start..]
        .iter()
        .rev()
        .zip(after[common_start..].iter().rev())
        .take_while(|(old, new)| old == new)
        .count();
    let mut script: Script = marked(Edit::Kept, &before[..common_start]).collect();
    script.extend(changed_middle(
        &before[common_start..before.len() - common_end],
        &after[common_start..after.len() - common_end],
    ));
    script.extend(marked(Edit::Kept, &before[before.len() - common_end..]));
    script
}

/// A longest common subsequence keeps the lines both sides share; its table grows with the
/// product of the sides, so past `MAX_DIFF_CELLS` the middle is replaced whole.
fn changed_middle<'text>(before: &[&'text str], after: &[&'text str]) -> Script<'text> {
    let width = after.len() + 1;
    if (before.len() + 1).saturating_mul(width) > MAX_DIFF_CELLS {
        return marked(Edit::Removed, before)
            .chain(marked(Edit::Added, after))
            .collect();
    }
    let mut shared = vec![0_u32; (before.len() + 1) * width];
    for old in (0..before.len()).rev() {
        for new in (0..after.len()).rev() {
            shared[old * width + new] = if before[old] == after[new] {
                shared[(old + 1) * width + new + 1] + 1
            } else {
                shared[(old + 1) * width + new].max(shared[old * width + new + 1])
            };
        }
    }
    let (mut old, mut new) = (0, 0);
    let mut script = Vec::new();
    while old < before.len() && new < after.len() {
        if before[old] == after[new] {
            script.push((Edit::Kept, before[old]));
            (old, new) = (old + 1, new + 1);
        } else if shared[(old + 1) * width + new] >= shared[old * width + new + 1] {
            script.push((Edit::Removed, before[old]));
            old += 1;
        } else {
            script.push((Edit::Added, after[new]));
            new += 1;
        }
    }
    script.extend(marked(Edit::Removed, &before[old..]));
    script.extend(marked(Edit::Added, &after[new..]));
    script
}

fn marked<'text>(edit: Edit, lines: &[&'text str]) -> impl Iterator<Item = (Edit, &'text str)> {
    lines.iter().map(move |line| (edit, *line))
}

fn hunk_ranges(script: &[(Edit, &str)]) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for (index, (edit, _)) in script.iter().enumerate() {
        if *edit == Edit::Kept {
            continue;
        }
        let start = index.saturating_sub(CONTEXT_LINES);
        let end = (index + 1 + CONTEXT_LINES).min(script.len());
        match ranges.last_mut() {
            Some(last) if start <= last.end => last.end = end,
            _ => ranges.push(start..end),
        }
    }
    ranges
}

fn hunk(script: &[(Edit, &str)], range: Range<usize>) -> Vec<String> {
    let lines_of = |side_lacks: Edit, range: Range<usize>| {
        script[range]
            .iter()
            .filter(|(edit, _)| *edit != side_lacks)
            .count()
    };
    let side = |side_lacks: Edit| {
        let count = lines_of(side_lacks, range.clone());
        let preceding = lines_of(side_lacks, 0..range.start);
        let start = if count == 0 { preceding } else { preceding + 1 };
        format!("{start},{count}")
    };
    let mut lines = vec![format!(
        "@@ -{} +{} @@",
        side(Edit::Added),
        side(Edit::Removed)
    )];
    lines.extend(
        script[range]
            .iter()
            .map(|(edit, line)| format!("{}{line}", edit.marker())),
    );
    lines
}

/// `None` for what is not UTF-8.
fn lines(bytes: Option<&[u8]>) -> Option<Vec<&str>> {
    let Some(bytes) = bytes else {
        return Some(Vec::new());
    };
    Some(std::str::from_utf8(bytes).ok()?.lines().collect())
}

fn canonical(path: &Path) -> Result<PathBuf, GenError> {
    path.canonicalize().map_err(|source| io_error(path, source))
}

struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new() -> Result<Self, GenError> {
        let path = private_directory("sofie-mcp-dry-run")?;
        Ok(Self {
            path: canonical(&path)?,
        })
    }

    fn remove(self) -> Result<(), GenError> {
        fs::remove_dir_all(&self.path).map_err(|source| io_error(&self.path, source))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // After `remove` the directory is gone; on an early return it is not,
        // and a failure here cannot be returned beside the error that ended the run.
        if self.path.exists()
            && let Err(error) = fs::remove_dir_all(&self.path)
        {
            eprintln!(
                "sofie mcp: removing {} failed: {error}",
                self.path.display()
            );
        }
    }
}

/// What a dry run answers: the changes, the generator's own report, then each change's diff.
/// That report is in the past tense of the copy it ran on, so it is labelled as such.
pub(super) fn dry_report(text: &str, changes: &[Change]) -> String {
    let listed = if changes.is_empty() {
        "  nothing".to_owned()
    } else {
        changes
            .iter()
            .map(|change| format!("  {change}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let diffs: Vec<String> = changes.iter().map(Change::diff).collect();
    format!(
        "dry run, nothing was written; it would:\n{listed}\n\n\
         what the generators reported on a scratch copy, not this app:\n{text}\n\
         what it would write:\n{}",
        diffs.join("\n\n")
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use sofie_model::testing::{TemporaryDirectory, manifest};

    use std::path::PathBuf;

    use super::{Change, canonical, dry_run};

    #[test]
    fn a_file_the_run_changes_is_listed_even_when_the_app_gets_the_same_change_meanwhile() {
        let directory = TemporaryDirectory::new("sofie-dry-run");
        directory.write(
            "Cargo.toml",
            "[workspace]\nresolver = \"3\"\nmembers = [\"app\"]\n",
        );
        directory.write("app/Cargo.toml", &manifest("dry", ""));
        directory.write("app/src/lib.rs", "");
        let original = directory.write("app/src/wiring.rs", "old\n");
        let root = canonical(directory.root()).unwrap();

        let (copy, changes) = dry_run(&root, |copy| {
            fs::write(copy.join("app/src/wiring.rs"), "new\n").unwrap();
            fs::write(&original, "new\n").unwrap();
            copy.display().to_string()
        })
        .unwrap();

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].to_string(), "change app/src/wiring.rs");
        assert!(!copy.ends_with('/'), "{copy}");
    }

    #[test]
    fn a_change_to_the_workspace_manifest_above_the_app_is_listed_from_the_app() {
        let directory = TemporaryDirectory::new("sofie-dry-run-workspace");
        directory.write(
            "Cargo.toml",
            "[workspace]\nresolver = \"3\"\nmembers = [\"shop/app\"]\n",
        );
        directory.write("shop/app/Cargo.toml", &manifest("dry", ""));
        directory.write("shop/app/src/lib.rs", "");
        let root = canonical(&directory.path("shop")).unwrap();

        let ((), changes) = dry_run(&root, |copy| {
            fs::write(copy.join("../Cargo.toml"), "[workspace]\n").unwrap();
            fs::write(copy.join("app/src/lib.rs"), "pub fn new() {}\n").unwrap();
        })
        .unwrap();

        let listed: Vec<String> = changes.iter().map(ToString::to_string).collect();
        assert_eq!(listed, ["change ../Cargo.toml", "change app/src/lib.rs"]);
    }

    /// Copied by its absolute path, such a member was copied onto itself, which truncated its files.
    #[test]
    fn a_member_outside_the_workspace_is_refused_and_left_whole() {
        let directory = TemporaryDirectory::new("sofie-dry-run-outside");
        directory.write(
            "ws/Cargo.toml",
            "[workspace]\nresolver = \"3\"\nmembers = [\"app\", \"../extern\"]\n",
        );
        directory.write("ws/app/Cargo.toml", &manifest("dry", ""));
        directory.write("ws/app/src/lib.rs", "");
        directory.write(
            "extern/Cargo.toml",
            "[package]\nname = \"extern\"\nversion = \"0.1.0\"\nedition = \"2024\"\nworkspace = \"../ws\"\n",
        );
        let library = directory.write("extern/src/lib.rs", "pub fn kept() {}\n");
        let root = canonical(&directory.path("ws")).unwrap();

        let refused = dry_run(&root, |_| ());

        assert!(refused.is_err());
        assert_eq!(fs::read_to_string(library).unwrap(), "pub fn kept() {}\n");
    }

    /// A FIFO blocked the copy forever and a looping link copied the app into itself.
    #[test]
    fn special_files_and_linked_directories_are_left_out_of_the_copy() {
        let directory = TemporaryDirectory::new("sofie-dry-run-special");
        directory.write(
            "Cargo.toml",
            "[workspace]\nresolver = \"3\"\nmembers = [\"app\"]\n",
        );
        directory.write("app/Cargo.toml", &manifest("dry", ""));
        directory.write("app/src/lib.rs", "");
        let made = std::process::Command::new("mkfifo")
            .arg(directory.path("app/pipe"))
            .status()
            .unwrap();
        assert!(made.success());
        std::os::unix::fs::symlink("..", directory.path("app/src/loop")).unwrap();
        std::os::unix::fs::symlink("/nonexistent/path", directory.path("app/dangling")).unwrap();
        let root = canonical(directory.root()).unwrap();

        let (listed, changes) = dry_run(&root, |copy| {
            (
                copy.join("app/pipe").exists(),
                copy.join("app/src/loop").exists(),
            )
        })
        .unwrap();

        assert_eq!(listed, (false, false));
        assert!(changes.is_empty());
    }

    fn change(before: Option<&str>, after: Option<&str>) -> Change {
        Change {
            path: PathBuf::from("src/queries.rs"),
            before: before.map(|text| text.as_bytes().to_vec()),
            after: after.map(|text| text.as_bytes().to_vec()),
        }
    }

    #[test]
    fn a_change_shows_the_lines_it_removes_and_adds_and_a_new_file_all_of_its_lines() {
        let edited = change(
            Some("use a;\n\nfn one() {}\n"),
            Some("use a;\n\nfn two() {}\nfn three() {}\n"),
        );
        let created = change(None, Some("fn one() {}\n"));
        let binary = change(None, Some("\u{0}\u{ff}"));

        assert_eq!(
            edited.diff(),
            "change src/queries.rs\n@@ -1,3 +1,4 @@\n use a;\n \n-fn one() {}\n+fn two() {}\n\
             +fn three() {}"
        );
        assert_eq!(
            created.diff(),
            "create src/queries.rs\n@@ -0,0 +1,1 @@\n+fn one() {}"
        );
        assert!(binary.diff().starts_with("create src/queries.rs"));
    }

    #[test]
    fn changes_far_apart_in_a_long_file_are_two_small_hunks_and_the_lines_between_stay_out() {
        let before: Vec<String> = (1..=40).map(|number| format!("line {number}")).collect();
        let mut after = before.clone();
        after[4] = "line five".to_owned();
        after.insert(30, "inserted".to_owned());

        let diff = change(Some(&before.join("\n")), Some(&after.join("\n"))).diff();

        let expected = [
            "change src/queries.rs",
            "@@ -2,7 +2,7 @@",
            " line 2",
            " line 3",
            " line 4",
            "-line 5",
            "+line five",
            " line 6",
            " line 7",
            " line 8",
            "@@ -28,6 +28,7 @@",
            " line 28",
            " line 29",
            " line 30",
            "+inserted",
            " line 31",
            " line 32",
            " line 33",
        ];
        assert_eq!(diff, expected.join("\n"));
    }

    #[test]
    fn a_created_file_is_shown_whole_past_the_cut_of_a_changed_one_and_a_huge_one_is_cut() {
        let numbered = |count: usize| -> String {
            (1..=count)
                .map(|number| format!("line {number}"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let long = numbered(200);

        let created = change(None, Some(&long)).diff();
        let changed = change(Some(""), Some(&long)).diff();
        let huge = change(None, Some(&numbered(1000))).diff();

        assert!(created.ends_with("+line 200"), "{created}");
        assert!(changed.ends_with("… 121 more lines"), "{changed}");
        assert!(huge.ends_with("… 401 more lines"), "{huge}");
    }
}
