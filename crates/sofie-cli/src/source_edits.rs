//! Each edit changes only what is missing and leaves the rest of the file as written.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use sofie_model::{ModuleDeclaration, module_declaration};

use crate::gen_command::{GenError, io_error};
use crate::rustfmt::format_rust;

pub(crate) const DEPENDENCIES: &str = "[dependencies]";
pub(crate) const DEV_DEPENDENCIES: &str = "[dev-dependencies]";

pub(crate) fn declare_module(root: &Path, path: &[&str]) -> Result<Option<PathBuf>, GenError> {
    let Some((file, text)) = declared_module(root, path)? else {
        return Ok(None);
    };
    write(&file, text)?;
    Ok(Some(file))
}

/// The file declaring the module and its text with the declaration, unless it has it.
pub(crate) fn declared_module(
    root: &Path,
    path: &[&str],
) -> Result<Option<(PathBuf, String)>, GenError> {
    let ModuleDeclaration::Missing {
        file,
        before,
        missing,
    } = module_declaration(root, path).map_err(GenError::Model)?
    else {
        return Ok(None);
    };
    let levels: Vec<&str> = missing.iter().map(String::as_str).collect();
    let edited = insert_at(&read(&file)?, before, &declaration(&levels));
    let formatted = format_rust(&edited, &file).map_err(GenError::Rustfmt)?;
    Ok(Some((file, formatted)))
}

fn declaration(path: &[&str]) -> String {
    match path {
        [] => String::new(),
        [last] => format!("pub mod {last};\n"),
        [first, rest @ ..] => format!("pub mod {first} {{\n{}}}\n", declaration(rest)),
    }
}

pub(crate) fn insert_at(text: &str, before: Option<(usize, usize)>, insertion: &str) -> String {
    let Some((line, column)) = before else {
        let separator = if text.is_empty() || text.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        return format!("{text}{separator}{insertion}");
    };
    let line_start: usize = text
        .split_inclusive('\n')
        .take(line - 1)
        .map(str::len)
        .sum();
    let offset = line_start
        + text[line_start..]
            .chars()
            .take(column)
            .map(char::len_utf8)
            .sum::<usize>();
    format!("{}{insertion}{}", &text[..offset], &text[offset..])
}

pub(crate) fn add_dependency(manifest: &Path, package: &str, path: &str) -> Result<(), GenError> {
    let text = read(manifest)?;
    write(
        manifest,
        with_dependency_line(&text, &path_dependency(package, path)),
    )
}

pub(crate) fn path_dependency(package: &str, path: &str) -> String {
    format!("{package} = {{ path = \"{}\" }}\n", toml_escaped(path))
}

/// First under `[dependencies]`, which is added when the manifest has none.
pub(crate) fn with_dependency_line(manifest: &str, line: &str) -> String {
    if toml_section(manifest, DEPENDENCIES).is_some() {
        return with_section_line(manifest, DEPENDENCIES, line);
    }
    let Some((_, _, end)) =
        toml_table(manifest, "[package]").filter(|table| table.2 < manifest.len())
    else {
        return with_section_line(manifest, DEPENDENCIES, line);
    };
    let before = without_trailing_comments(&manifest[..end]);
    let end = before.len();
    let separator = if before.ends_with("\n\n") { "" } else { "\n" };
    format!(
        "{before}{separator}{DEPENDENCIES}\n{line}\n{}",
        &manifest[end..]
    )
}

/// The comment lines ending `text` note the table after them.
fn without_trailing_comments(text: &str) -> &str {
    let mut end = text.len();
    while let Some(line_start) = text[..end]
        .strip_suffix('\n')
        .and_then(|lines| lines.rfind('\n'))
        .map(|index| index + 1)
        && text[line_start..end].trim_start().starts_with('#')
    {
        end = line_start;
    }
    &text[..end]
}

/// First under `header`, which is added when the manifest has none.
pub(crate) fn with_section_line(manifest: &str, header: &str, line: &str) -> String {
    match toml_section(manifest, header) {
        Some((start, _)) => format!("{}{line}{}", &manifest[..start], &manifest[start..]),
        None => insert_at(manifest, None, &format!("\n{header}\n{line}")),
    }
}

/// The body of the table `header` names, from the line after its header to the
/// next table; a header on its own line only, so `# [dependencies]` is none.
pub(crate) fn toml_section(manifest: &str, header: &str) -> Option<(usize, usize)> {
    toml_table(manifest, header).map(|(_, start, end)| (start, end))
}

/// Where the header line starts, where the body starts and where it ends.
pub(crate) fn toml_table(manifest: &str, header: &str) -> Option<(usize, usize, usize)> {
    let mut offset = 0;
    let mut found = None;
    let wanted = table_header(header);
    for line in manifest.split_inclusive('\n') {
        let next = offset + line.len();
        let found_header = table_header(line);
        match found {
            None if found_header.is_some() && found_header == wanted => {
                found = Some((offset, next));
            }
            Some((header_start, start)) if found_header.is_some() => {
                return Some((header_start, start, offset));
            }
            _ => {}
        }
        offset = next;
    }
    found.map(|(header_start, start)| (header_start, start, manifest.len()))
}

/// A table header as TOML reads it: `[ dependencies ] # note` is `[dependencies]`.
fn table_header(line: &str) -> Option<String> {
    let line = line.trim();
    if !line.starts_with('[') {
        return None;
    }
    let written = line.split('#').next().unwrap_or_default();
    Some(
        written
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect(),
    )
}

/// Joins an existing `use` of the same path when there is one; rustfmt then sorts the names.
pub(crate) fn add_import(text: &str, path: &str, name: &str) -> String {
    let prefix = format!("{path}::");
    let lines: Vec<&str> = text.lines().collect();
    for (index, line) in lines.iter().enumerate() {
        let Some(rest) = line.strip_prefix(&prefix) else {
            continue;
        };
        if let Some(edited) = joined_import(&lines, index, &prefix, rest, name) {
            return edited;
        }
    }
    let anchor = last_statement_end(&lines, |line| {
        line.starts_with("use ") || line.starts_with("pub use ")
    })
    .or_else(|| last_statement_end(&lines, is_module_line));
    insert_line(&lines, anchor, &format!("{prefix}{name};"))
}

fn joined_import(
    lines: &[&str],
    index: usize,
    prefix: &str,
    rest: &str,
    name: &str,
) -> Option<String> {
    let names = |list: &str| -> bool { list.split(',').any(|listed| listed.trim() == name) };
    let mut edited: Vec<String> = lines.iter().map(|line| (*line).to_owned()).collect();
    if rest == "{" {
        let close = (index..lines.len()).find(|&line| lines[line].starts_with('}'))?;
        if names(&lines[index + 1..close].join(",")) {
            return Some(text_of(&edited));
        }
        edited.insert(index + 1, format!("    {name},"));
    } else if let Some(list) = rest
        .strip_prefix('{')
        .and_then(|rest| rest.strip_suffix("};"))
    {
        if names(list) {
            return Some(text_of(&edited));
        }
        edited[index] = format!("{prefix}{{{name}, {list}}};");
    } else {
        let single = rest.strip_suffix(';')?;
        if single.contains("::") {
            return None;
        }
        if single == name {
            return Some(text_of(&edited));
        }
        edited[index] = format!("{prefix}{{{name}, {single}}};");
    }
    Some(text_of(&edited))
}

/// After the last top-level `mod name;`, where rustfmt sorts the group.
pub(crate) fn add_module_line(text: &str, line: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let anchor = last_statement_end(&lines, is_module_line);
    insert_line(&lines, anchor, line)
}

fn is_module_line(line: &str) -> bool {
    (line.starts_with("mod ") || line.starts_with("pub mod ")) && line.ends_with(';')
}

fn last_statement_end(lines: &[&str], starts: impl Fn(&str) -> bool) -> Option<usize> {
    let mut end = None;
    let mut index = 0;
    while index < lines.len() {
        if starts(lines[index]) {
            if lines[index].ends_with('{') {
                index = (index..lines.len())
                    .find(|&line| lines[line].starts_with('}'))
                    .unwrap_or(index);
            }
            end = Some(index);
        }
        index += 1;
    }
    end
}

/// Without an anchor, the line goes after the file's inner doc comments and attributes, which must lead it.
fn insert_line(lines: &[&str], after: Option<usize>, line: &str) -> String {
    let mut edited: Vec<String> = lines.iter().map(|line| (*line).to_owned()).collect();
    if let Some(index) = after {
        edited.insert(index + 1, line.to_owned());
        return text_of(&edited);
    }
    let header = lines
        .iter()
        .take_while(|line| line.starts_with("//!") || line.starts_with("#!["))
        .count();
    edited.insert(header, line.to_owned());
    if edited.len() > header + 1 {
        edited.insert(header + 1, String::new());
    }
    text_of(&edited)
}

fn text_of(lines: &[String]) -> String {
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// Adds `<binding>.<call>;` as the last statement of `fn <function>`, naming its parameter when it was `_<binding>`.
pub(crate) fn add_call(text: &str, function: &str, call: &str) -> Option<String> {
    let opening = format!("fn {function}(");
    let start = text.find(&opening)? + opening.len();
    let colon = start + text[start..].find(':')?;
    let parameter = text[start..colon].trim();
    let (mutability, name) = parameter
        .strip_prefix("mut ")
        .map_or(("", parameter), |name| ("mut ", name.trim()));
    let binding = name.trim_start_matches('_');
    let open = colon + text[colon..].find('{')?;
    let close = closing_brace(text, open)?;
    Some(format!(
        "{}{mutability}{binding}{}{}\n    {binding}.{call};\n{}",
        &text[..start],
        &text[colon..=open],
        text[open + 1..close].trim_end(),
        &text[close..]
    ))
}

/// Skips literals and comments, as route paths hold braces.
fn closing_brace(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0_usize;
    let mut index = open;
    while index < bytes.len() {
        let rest = &bytes[index..];
        match rest {
            [b'"', ..] => index = string_end(bytes, index)?,
            [b'r', b'"' | b'#', ..] if is_raw_string(rest) => index += raw_string_length(rest)? - 1,
            [b'\'', b'\\', ..] => index += rest[2..].iter().position(|byte| *byte == b'\'')? + 2,
            [b'\'', _, b'\'', ..] => index += 2,
            [b'/', b'/', ..] => {
                index += text[index..].find('\n').unwrap_or(text.len() - index);
            }
            [b'/', b'*', ..] => index += block_comment_length(rest)? - 1,
            [b'{', ..] => depth += 1,
            [b'}', ..] => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

/// `r"…"` or `r#"…"#`, not an identifier ending in `r` followed by `#`.
fn is_raw_string(rest: &[u8]) -> bool {
    let hashes = rest[1..].iter().take_while(|byte| **byte == b'#').count();
    rest.get(1 + hashes) == Some(&b'"')
}

fn raw_string_length(rest: &[u8]) -> Option<usize> {
    let hashes = rest[1..].iter().take_while(|byte| **byte == b'#').count();
    let body = 2 + hashes;
    let mut closing = vec![b'"'];
    closing.extend(std::iter::repeat_n(b'#', hashes));
    let end = rest[body..]
        .windows(closing.len())
        .position(|window| window == closing.as_slice())?;
    Some(body + end + closing.len())
}

/// Rust's block comments nest.
fn block_comment_length(rest: &[u8]) -> Option<usize> {
    let mut depth = 0_usize;
    let mut index = 0;
    while index + 1 < rest.len() {
        match (rest[index], rest[index + 1]) {
            (b'/', b'*') => depth += 1,
            (b'*', b'/') => depth -= 1,
            _ => {
                index += 1;
                continue;
            }
        }
        index += 2;
        if depth == 0 {
            return Some(index);
        }
    }
    None
}

fn string_end(bytes: &[u8], open: usize) -> Option<usize> {
    let mut index = open + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 1,
            b'"' => return Some(index),
            _ => {}
        }
        index += 1;
    }
    None
}

pub(crate) fn append_lines(file: &Path, lines: &[String]) -> Result<(), GenError> {
    write(file, with_lines_appended(&read(file)?, lines))
}

pub(crate) fn with_lines_appended(text: &str, lines: &[String]) -> String {
    let mut appended = lines.join("\n");
    appended.push('\n');
    insert_at(text, None, &appended)
}

pub(crate) fn toml_escaped(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

pub(crate) fn read(file: &Path) -> Result<String, GenError> {
    fs::read_to_string(file).map_err(|source| io_error(file, source))
}

pub(crate) fn files_with_extension(
    directory: &Path,
    extension: &str,
) -> Result<Vec<PathBuf>, GenError> {
    sofie_model::files_with_extension(directory, extension).map_err(GenError::Model)
}

pub(crate) fn write(file: &Path, contents: impl AsRef<[u8]>) -> Result<(), GenError> {
    if let Some(directory) = file.parent() {
        fs::create_dir_all(directory).map_err(|source| io_error(directory, source))?;
    }
    fs::write(file, contents).map_err(|source| io_error(file, source))
}

/// A command writing several files records each step, so a failure part-way
/// leaves the app as it found it and a second run is not refused by what the
/// first left behind.
enum Step {
    Wrote {
        file: PathBuf,
        original: Option<Vec<u8>>,
    },
    CreatedDirectory(PathBuf),
    Renamed {
        from: PathBuf,
        to: PathBuf,
    },
}

#[derive(Default)]
pub(crate) struct Journal {
    steps: Vec<Step>,
}

impl Journal {
    pub(crate) fn write(
        &mut self,
        file: &Path,
        contents: impl AsRef<[u8]>,
    ) -> Result<(), GenError> {
        self.created_directories(file.parent());
        let original = match fs::read(file) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(source) => return Err(io_error(file, source)),
        };
        self.steps.push(Step::Wrote {
            file: file.to_path_buf(),
            original,
        });
        write(file, contents)
    }

    pub(crate) fn rename(&mut self, from: &Path, to: &Path) -> Result<(), GenError> {
        self.created_directories(to.parent());
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
        }
        fs::rename(from, to).map_err(|source| io_error(from, source))?;
        self.steps.push(Step::Renamed {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
        });
        Ok(())
    }

    /// Undoes every step, newest first, and returns `error`, naming what could
    /// not be restored, if anything.
    pub(crate) fn roll_back(self, error: GenError) -> GenError {
        let unrestored: Vec<String> = self
            .steps
            .into_iter()
            .rev()
            .filter_map(|step| undo(&step).err())
            .collect();
        if unrestored.is_empty() {
            return error;
        }
        GenError::App(format!(
            "{error}\nrestoring what was written before the failure failed too; fix by hand:\n  {}",
            unrestored.join("\n  ")
        ))
    }

    /// `Ok` as it is; on `Err`, everything rolled back first.
    pub(crate) fn finish<T>(self, result: Result<T, GenError>) -> Result<T, GenError> {
        result.map_err(|error| self.roll_back(error))
    }

    /// The outermost directory `path` creates, which goes as a whole on rollback.
    fn created_directories(&mut self, directory: Option<&Path>) {
        let mut outermost = None;
        let mut current = directory;
        while let Some(directory) = current.filter(|directory| !directory.as_os_str().is_empty()) {
            if directory.exists() {
                break;
            }
            outermost = Some(directory);
            current = directory.parent();
        }
        if let Some(directory) = outermost {
            self.steps
                .push(Step::CreatedDirectory(directory.to_path_buf()));
        }
    }
}

fn undo(step: &Step) -> Result<(), String> {
    let (path, undone) = match step {
        Step::Wrote {
            file,
            original: Some(original),
        } => (file, fs::write(file, original)),
        Step::Wrote {
            file,
            original: None,
        } => (file, remove(fs::remove_file(file))),
        Step::CreatedDirectory(directory) => (directory, remove(fs::remove_dir_all(directory))),
        Step::Renamed { from, to } => (to, fs::rename(to, from)),
    };
    undone.map_err(|error| format!("{}: {error}", path.display()))
}

/// Already gone is undone: a created directory takes the files written in it along.
fn remove(removal: std::io::Result<()>) -> std::io::Result<()> {
    match removal {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use sofie_model::testing::TemporaryDirectory;

    use super::{Journal, add_dependency, declare_module, with_dependency_line, with_section_line};
    use crate::gen_command::GenError;

    #[test]
    fn a_header_with_a_comment_or_inner_spaces_is_the_same_table() {
        let manifest = "[package]\nname = \"a\"\n\n[ dependencies ] # ours\nserde = \"1\"\n";

        let edited = with_section_line(manifest, "[dependencies]", "b = \"1\"\n");

        assert_eq!(
            edited,
            "[package]\nname = \"a\"\n\n[ dependencies ] # ours\nb = \"1\"\nserde = \"1\"\n"
        );
    }

    #[test]
    fn a_new_dependencies_table_follows_the_package_table() {
        let line = "sofie.workspace = true\n";
        let spaced = "[package]\nname = \"a\"\n\n[lints]\nworkspace = true\n";
        let tight = "[package]\nname = \"a\"\n[dev-dependencies]\nb = \"1\"\n";

        assert_eq!(
            with_dependency_line(spaced, line),
            "[package]\nname = \"a\"\n\n[dependencies]\nsofie.workspace = true\n\n\
             [lints]\nworkspace = true\n"
        );
        assert_eq!(
            with_dependency_line(tight, line),
            "[package]\nname = \"a\"\n\n[dependencies]\nsofie.workspace = true\n\n\
             [dev-dependencies]\nb = \"1\"\n"
        );
        assert_eq!(
            with_dependency_line("[package]\nname = \"a\"\n", line),
            "[package]\nname = \"a\"\n\n[dependencies]\nsofie.workspace = true\n"
        );
    }

    #[test]
    fn a_new_dependencies_table_leaves_the_next_table_s_comment_above_it() {
        let commented =
            "[package]\nname = \"a\"\n# Ours.\n\n# Shared by the workspace.\n# Keep.\n[lints]\n";

        assert_eq!(
            with_dependency_line(commented, "sofie.workspace = true\n"),
            "[package]\nname = \"a\"\n# Ours.\n\n[dependencies]\nsofie.workspace = true\n\n\
             # Shared by the workspace.\n# Keep.\n[lints]\n"
        );
    }

    #[test]
    fn declares_each_missing_level_inline_or_in_its_file() {
        let directory = TemporaryDirectory::new("sofie-cli-edits");
        let root = directory.write(
            "src/lib.rs",
            "pub mod module;\n\npub mod areas {\n    pub mod web {\n        pub mod routes;\n    }\n}\n",
        );

        declare_module(&root, &["areas", "console", "routes"]).unwrap();
        declare_module(&root, &["config"]).unwrap();
        let unchanged = declare_module(&root, &["areas", "web", "routes"]).unwrap();

        assert_eq!(
            fs::read_to_string(&root).unwrap(),
            "pub mod module;\n\npub mod areas {\n    pub mod web {\n        pub mod routes;\n    }\n    \
             pub mod console {\n        pub mod routes;\n    }\n}\npub mod config;\n"
        );
        assert_eq!(unchanged, None);
    }

    #[test]
    fn follows_a_module_into_its_own_file() {
        let directory = TemporaryDirectory::new("sofie-cli-edits");
        let root = directory.write("src/lib.rs", "pub mod areas;\n");
        let areas = directory.write("src/areas.rs", "pub mod web;\n");

        let edited = declare_module(&root, &["areas", "console", "menu"]).unwrap();

        assert_eq!(edited, Some(areas.clone()));
        assert_eq!(
            fs::read_to_string(&areas).unwrap(),
            "pub mod web;\npub mod console {\n    pub mod menu;\n}\n"
        );
    }

    #[test]
    fn adds_a_dependency_under_the_section() {
        let directory = TemporaryDirectory::new("sofie-cli-edits");
        let manifest = directory.write(
            "app/Cargo.toml",
            "[package]\nname = \"shop\"\n\n[dependencies]\nsofie.workspace = true\n",
        );

        add_dependency(&manifest, "billing", "../modules/billing").unwrap();

        assert_eq!(
            fs::read_to_string(&manifest).unwrap(),
            "[package]\nname = \"shop\"\n\n[dependencies]\nbilling = { path = \"../modules/billing\" }\n\
             sofie.workspace = true\n"
        );
    }

    #[test]
    fn adds_a_call_past_braces_in_literals_and_comments_and_keeps_mut() {
        let text = "fn routes(mut _routes: &mut Routes<Web>) {\n    \
                    let _ = ('{', '\\'', r#\"}\"#, \"}\");\n    /* } /* } */ } */\n}\n\nfn other() {}\n";

        let added = super::add_call(text, "routes", "get(\"/a\", list)").unwrap();

        assert_eq!(
            added,
            "fn routes(mut routes: &mut Routes<Web>) {\n    \
             let _ = ('{', '\\'', r#\"}\"#, \"}\");\n    /* } /* } */ } */\n    \
             routes.get(\"/a\", list);\n}\n\nfn other() {}\n"
        );
    }

    #[test]
    fn finds_a_crlf_section_and_skips_a_commented_header() {
        let manifest =
            "[package]\r\n# [dependencies]\r\n\r\n[dependencies]\r\nsofie.workspace = true\r\n";

        let edited = super::with_dependency_line(manifest, "billing = { path = \"b\" }\n");

        assert_eq!(
            edited,
            "[package]\r\n# [dependencies]\r\n\r\n[dependencies]\r\nbilling = { path = \"b\" }\n\
             sofie.workspace = true\r\n"
        );
    }

    #[test]
    fn a_failure_restores_changed_files_and_removes_created_ones() {
        let directory = TemporaryDirectory::new("sofie-journal");
        let kept = directory.write("app/Cargo.toml", "[package]\n");
        let moved = directory.write("modules/order/Cargo.toml", "order\n");
        let mut journal = Journal::default();

        journal.write(&kept, "[package]\nchanged\n").unwrap();
        journal
            .write(&directory.path("modules/new/src/lib.rs"), "")
            .unwrap();
        journal
            .rename(
                &directory.path("modules/order"),
                &directory.path("split/order/modules/order"),
            )
            .unwrap();
        let error = journal.roll_back(GenError::Usage("a later step failed".to_owned()));

        assert_eq!(error.to_string(), "a later step failed");
        assert_eq!(fs::read_to_string(&kept).unwrap(), "[package]\n");
        assert_eq!(fs::read_to_string(&moved).unwrap(), "order\n");
        assert!(!directory.path("modules/new").exists());
        assert!(!directory.path("split").exists());
    }
}
