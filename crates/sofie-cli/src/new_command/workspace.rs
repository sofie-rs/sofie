//! The cargo workspace an app's crates belong to: the one `sofie new app` writes for it, or
//! one enclosing the directory the app is made in.

use std::path::{Path, PathBuf};

use sofie_model::naming::relative_path;

use super::edits::{dependency_line_start, with_workspace_dependency, workspace_manifest};
use super::{framework_declaration, framework_source_in};
use crate::gen_command::{GenError, io_error};
use crate::source_edits::{toml_escaped, toml_table};

/// What a crate states itself when its workspace does not declare it for inheriting.
pub(super) const PACKAGE_DEFAULTS: [(&str, &str); 3] = [
    ("edition", "\"2024\""),
    ("version", "\"0.1.0\""),
    ("publish", "false"),
];

/// The framework crates every app's manifests inherit from the workspace.
const FRAMEWORK: [&str; 2] = ["sofie-framework", "sofie-cli"];

/// Canonical first: a relative directory such as `.` has no ancestors to search.
pub(super) fn owning_workspace(directory: &Path) -> Result<Option<PathBuf>, GenError> {
    let canonical = directory
        .canonicalize()
        .map_err(|source| io_error(directory, source))?;
    Ok(workspace_manifest(&canonical))
}

/// `directory` as the workspace's members name it, ending in `/`, or empty at the workspace root.
pub(super) fn member_prefix(workspace: &Path, directory: &Path) -> Result<String, GenError> {
    let canonical = directory
        .canonicalize()
        .map_err(|source| io_error(directory, source))?;
    let root = workspace.parent().unwrap_or(workspace);
    let relative = relative_path(root, &canonical).ok_or_else(|| {
        GenError::Usage(format!(
            "{} cannot name {} by a relative path",
            workspace.display(),
            canonical.display()
        ))
    })?;
    if relative.is_empty() {
        return Ok(relative);
    }
    Ok(relative + "/")
}

pub(super) fn package_section(name: &str, workspace: &str) -> String {
    let inherited = toml_table(workspace, "[workspace.package]")
        .map_or("", |(_, start, end)| &workspace[start..end]);
    let lines: Vec<String> = PACKAGE_DEFAULTS
        .iter()
        .map(|(key, value)| {
            if dependency_line_start(inherited, key).is_some() {
                format!("{key}.workspace = true\n")
            } else {
                format!("{key} = {value}\n")
            }
        })
        .collect();
    format!("[package]\nname = \"{name}\"\n{}", lines.concat())
}

pub(super) fn lints_section(workspace: &str) -> &'static str {
    let declares_lints = workspace
        .lines()
        .any(|line| line.trim_start().starts_with("[workspace.lints"));
    if declares_lints {
        "\n[lints]\nworkspace = true\n"
    } else {
        ""
    }
}

/// The enclosing workspace with the framework crates declared, a checkout's by a path relative
/// to it, and `members` listed after the ones it has.
pub(super) fn joined(workspace: &Path, text: &str, members: &[String]) -> Result<String, GenError> {
    let root = workspace.parent().unwrap_or(workspace);
    let declared = FRAMEWORK.iter().fold(text.to_owned(), |text, package| {
        let line = framework_declaration(package, &framework_source_in(package, root));
        let key = line.split(' ').next().unwrap_or_default();
        with_workspace_dependency(&text, key, &line).unwrap_or(text)
    });
    with_members(&declared, members).ok_or_else(|| {
        GenError::Usage(format!(
            "{} has no `members = [...]` list under `[workspace]` to add {} to",
            workspace.display(),
            members.join(", ")
        ))
    })
}

/// Adds each of `members` the list lacks, in its layout: one per line or all on one.
pub(super) fn with_members(manifest: &str, members: &[String]) -> Option<String> {
    let (_, body_start, body_end) = toml_table(manifest, "[workspace]")?;
    let body = &manifest[body_start..body_end];
    let Some(key) = dependency_line_start(body, "members") else {
        let listed: Vec<String> = members.iter().map(|member| quoted(member)).collect();
        let line = format!("members = [{}]\n", listed.join(", "));
        return Some(format!(
            "{}{line}{}",
            &manifest[..body_start],
            &manifest[body_start..]
        ));
    };
    let open = body_start + key + body[key..].find('[')?;
    let close = open + manifest[open..].find(']')?;
    let listed = &manifest[open + 1..close];
    let missing: Vec<String> = members
        .iter()
        .map(|member| quoted(member))
        .filter(|member| !listed.contains(member.as_str()))
        .collect();
    if missing.is_empty() {
        return Some(manifest.to_owned());
    }
    let entries = listed.trim_end();
    let separator = if entries.trim().is_empty() || entries.ends_with(',') {
        ""
    } else {
        ","
    };
    let inserted = if listed.contains('\n') {
        let indent = listed
            .lines()
            .find(|line| !line.trim().is_empty())
            .map_or("    ", |line| &line[..line.len() - line.trim_start().len()]);
        let lines: Vec<String> = missing
            .iter()
            .map(|member| format!("\n{indent}{member},"))
            .collect();
        format!("{entries}{separator}{}\n", lines.concat())
    } else {
        let space = if entries.trim().is_empty() { "" } else { " " };
        format!("{entries}{separator}{space}{}", missing.join(", "))
    };
    Some(format!(
        "{}{inserted}{}",
        &manifest[..=open],
        &manifest[close..]
    ))
}

fn quoted(member: &str) -> String {
    format!("\"{}\"", toml_escaped(member))
}

#[cfg(test)]
mod tests {
    use super::{package_section, with_members};

    fn members(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn members_join_a_list_in_its_own_layout() {
        let one_line = "[workspace]\nmembers = [\"app\", \"modules/*\"]\n";
        let per_line = "[workspace]\nresolver = \"3\"\nmembers = [\n    \"crates/*\",\n]\n\n[workspace.package]\n";

        assert_eq!(
            with_members(one_line, &members(&["modules/*/contract"])).unwrap(),
            "[workspace]\nmembers = [\"app\", \"modules/*\", \"modules/*/contract\"]\n"
        );
        assert_eq!(
            with_members(per_line, &members(&["demo/app", "crates/*"])).unwrap(),
            "[workspace]\nresolver = \"3\"\nmembers = [\n    \"crates/*\",\n    \"demo/app\",\n]\n\n[workspace.package]\n"
        );
        assert_eq!(
            with_members("[workspace]\n", &members(&["demo/app"])).unwrap(),
            "[workspace]\nmembers = [\"demo/app\"]\n"
        );
    }

    #[test]
    fn a_crate_inherits_only_what_its_workspace_declares() {
        let workspace = "[workspace]\n\n[workspace.package]\nedition = \"2024\"\n";

        assert_eq!(
            package_section("demo", workspace),
            "[package]\nname = \"demo\"\nedition.workspace = true\nversion = \"0.1.0\"\npublish = false\n"
        );
    }
}
