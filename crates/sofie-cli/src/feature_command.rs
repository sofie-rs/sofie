use std::fmt;
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde_json::Value;

use crate::cargo;
use crate::gen_command::{GenError, io_error};

const FACADE_PACKAGE: &str = "sofie-framework";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FeatureChange {
    Add,
    Remove,
}

pub struct FeatureEdit {
    pub manifest: PathBuf,
    pub feature: String,
    pub change: FeatureChange,
    /// False when the manifest already said so.
    pub changed: bool,
    pub features: Vec<String>,
}

impl fmt::Display for FeatureEdit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let verb = match (self.change, self.changed) {
            (FeatureChange::Add, true) => "added",
            (FeatureChange::Remove, true) => "removed",
            (FeatureChange::Add, false) => "already had",
            (FeatureChange::Remove, false) => "did not have",
        };
        write!(
            formatter,
            "{}: the `sofie` dependency {verb} the feature `{}`; its features are now [{}]",
            self.manifest.display(),
            self.feature,
            self.features.join(", ")
        )
    }
}

/// Edits only the features array of the `sofie` dependency in `app/Cargo.toml`, as text, so
/// the manifest keeps its comments and layout.
pub fn change_feature(
    root: &Path,
    feature: &str,
    change: FeatureChange,
) -> Result<FeatureEdit, GenError> {
    let manifest = cargo::app_manifest(root);
    let known = facade_features(&manifest)?;
    if !known.iter().any(|known| known == feature) {
        return Err(GenError::Usage(format!(
            "`sofie` has no feature `{feature}`; its features are {}",
            known.join(", ")
        )));
    }
    let text = fs::read_to_string(&manifest).map_err(|source| io_error(&manifest, source))?;
    let refused = |message: String| GenError::Usage(format!("{}: {message}", manifest.display()));
    let edited = edited_manifest(&text, feature, change).map_err(refused)?;
    if edited.changed {
        fs::write(&manifest, &edited.text).map_err(|source| io_error(&manifest, source))?;
    }
    Ok(FeatureEdit {
        manifest,
        feature: feature.to_owned(),
        change,
        changed: edited.changed,
        features: edited.features,
    })
}

/// Every feature the facade declares, sorted; `default` is no feature to add.
fn facade_features(manifest: &Path) -> Result<Vec<String>, GenError> {
    let output = cargo::command()
        .args(["metadata", "--format-version", "1", "--manifest-path"])
        .arg(manifest)
        .stdin(Stdio::null())
        .output()
        .map_err(|source| io_error(manifest, source))?;
    if !output.status.success() {
        return Err(GenError::App(format!(
            "`cargo metadata` for {} failed: {}",
            manifest.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let metadata: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| GenError::App(format!("`cargo metadata` answered no JSON: {error}")))?;
    let facade = metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|package| package["name"] == FACADE_PACKAGE)
        .ok_or_else(|| {
            GenError::Usage(format!(
                "{} does not depend on `{FACADE_PACKAGE}`, so it has no `sofie` features",
                manifest.display()
            ))
        })?;
    let mut features: Vec<String> = facade["features"]
        .as_object()
        .into_iter()
        .flat_map(|features| features.keys())
        .filter(|name| *name != "default")
        .cloned()
        .collect();
    features.sort();
    Ok(features)
}

#[derive(Debug, PartialEq, Eq)]
struct EditedManifest {
    text: String,
    changed: bool,
    features: Vec<String>,
}

/// Where the `sofie` dependency of `[dependencies]` lists its features, or where a list goes.
#[derive(Debug, PartialEq, Eq)]
enum FeatureList {
    /// From `[` through `]`.
    Array(Range<usize>),
    /// Into an inline table before its closing `}`.
    InlineEntry(usize),
    /// A line of its own after this offset, with this key.
    Line(usize, &'static str),
}

fn edited_manifest(
    text: &str,
    feature: &str,
    change: FeatureChange,
) -> Result<EditedManifest, String> {
    let list = feature_list(text)?;
    let listed = match &list {
        FeatureList::Array(range) => array_features(&text[range.clone()])?,
        _ => Vec::new(),
    };
    let present = listed.iter().any(|listed| listed == feature);
    let features: Vec<String> = match change {
        FeatureChange::Add if present => listed,
        FeatureChange::Add => listed.into_iter().chain([feature.to_owned()]).collect(),
        FeatureChange::Remove if !present => {
            return Err(format!(
                "the `sofie` dependency does not list the feature `{feature}`; one on by default \
                 goes with `default-features = false` and the defaults it keeps listed"
            ));
        }
        FeatureChange::Remove => listed.into_iter().filter(|name| name != feature).collect(),
    };
    let changed = present != (change == FeatureChange::Add);
    let mut edited = text.to_owned();
    match list {
        FeatureList::Array(range) => {
            let array = rendered_array(&text[range.clone()], &features);
            edited.replace_range(range, &array);
        }
        FeatureList::InlineEntry(at) => {
            let entry = format!(", features = {}", rendered_array("[]", &features));
            edited.insert_str(at, &entry);
        }
        FeatureList::Line(at, key) => {
            let line = format!("\n{key} = {}", rendered_array("[]", &features));
            edited.insert_str(at, &line);
        }
    }
    Ok(EditedManifest {
        text: edited,
        changed,
        features,
    })
}

fn feature_list(text: &str) -> Result<FeatureList, String> {
    let mut section = "";
    let mut dotted_end = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let content = without_comment(line).trim();
        if content.starts_with('[') {
            if section == "dependencies.sofie" {
                break;
            }
            section = content.trim_matches(['[', ']']).trim();
            if section == "dependencies.sofie" {
                dotted_end = Some((start + line.trim_end().len(), "features"));
            }
            continue;
        }
        if section == "dependencies.sofie" && key_of(content) == Some("features") {
            return array_from(text, start + line.find('[').unwrap_or(line.len()));
        }
        if section != "dependencies" {
            continue;
        }
        match key_of(content) {
            Some("sofie.features") => {
                return array_from(text, start + line.find('[').unwrap_or(line.len()));
            }
            Some(key) if key.starts_with("sofie.") => {
                dotted_end = Some((start + line.trim_end().len(), "sofie.features"));
            }
            Some("sofie") => return inline_list(line, start),
            _ => {}
        }
    }
    dotted_end
        .map(|(at, key)| FeatureList::Line(at, key))
        .ok_or_else(|| "its `[dependencies]` has no `sofie` dependency".to_owned())
}

fn key_of(content: &str) -> Option<&str> {
    let (key, _) = content.split_once('=')?;
    Some(key.trim())
}

/// Feature names and paths hold no `#`, so the first one starts a comment.
fn without_comment(line: &str) -> &str {
    line.split_once('#').map_or(line, |(before, _)| before)
}

fn inline_list(line: &str, start: usize) -> Result<FeatureList, String> {
    let content = without_comment(line);
    let Some(close) = content.rfind('}') else {
        return Err(
            "the `sofie` dependency is a version string; write it as an inline table, \
             `sofie = { version = \"…\" }`, to give it features"
                .to_owned(),
        );
    };
    let Some(key) = features_key(content) else {
        return Ok(FeatureList::InlineEntry(
            start + content[..close].trim_end().len(),
        ));
    };
    let open = content[key..].find('[').map(|found| key + found);
    let close = open.and_then(|open| content[open..].find(']').map(|found| open + found));
    match (open, close) {
        (Some(open), Some(close)) => Ok(FeatureList::Array(start + open..start + close + 1)),
        _ => Err("the `sofie` dependency's `features` is no array on one line".to_owned()),
    }
}

/// Not the `features` of `default-features`.
fn features_key(content: &str) -> Option<usize> {
    content
        .match_indices("features")
        .map(|(at, _)| at)
        .find(|at| {
            let before = content[..*at].trim_end();
            let after = content[at + "features".len()..].trim_start();
            (before.ends_with('{') || before.ends_with(',')) && after.starts_with('=')
        })
}

fn array_from(text: &str, open: usize) -> Result<FeatureList, String> {
    let close = text
        .get(open..)
        .filter(|rest| rest.starts_with('['))
        .and_then(|rest| rest.find(']'))
        .ok_or("the `sofie` dependency's `features` is no array")?;
    Ok(FeatureList::Array(open..open + close + 1))
}

fn array_features(array: &str) -> Result<Vec<String>, String> {
    let inner = &array[1..array.len() - 1];
    if inner.contains('#') {
        return Err(
            "a comment sits inside the `sofie` dependency's features; edit that list by hand"
                .to_owned(),
        );
    }
    inner
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| {
            item.strip_prefix('"')
                .and_then(|item| item.strip_suffix('"'))
                .map(str::to_owned)
                .ok_or_else(|| format!("the feature `{item}` is no quoted string"))
        })
        .collect()
}

/// An array written over several lines stays so, one feature a line.
fn rendered_array(original: &str, features: &[String]) -> String {
    let quoted: Vec<String> = features
        .iter()
        .map(|feature| format!("\"{feature}\""))
        .collect();
    if quoted.is_empty() || !original.contains('\n') {
        return format!("[{}]", quoted.join(", "));
    }
    let inner = &original[1..original.len() - 1];
    let indent = inner
        .lines()
        .find(|line| !line.trim().is_empty())
        .map_or("    ", |line| &line[..line.len() - line.trim_start().len()]);
    let closing = inner.rsplit_once('\n').map_or("", |(_, last)| last);
    let separator = format!(",\n{indent}");
    format!("[\n{indent}{},\n{closing}]", quoted.join(&separator))
}

#[cfg(test)]
mod tests {
    use super::{FeatureChange, edited_manifest};

    fn edited(text: &str, feature: &str, change: FeatureChange) -> String {
        edited_manifest(text, feature, change).unwrap().text
    }

    #[test]
    fn an_inline_dependency_gains_and_loses_a_feature_and_keeps_its_comments() {
        let manifest = "[dependencies]\n# The framework.\nsofie = { workspace = true, features = \
                        [\"engine\", \"postgres\"] } # pinned\n\n[dev-dependencies]\nsofie = { \
                        workspace = true, features = [\"capture\"] }\n";

        let added = edited(manifest, "i18n", FeatureChange::Add);
        let removed = edited(&added, "engine", FeatureChange::Remove);

        assert_eq!(
            added,
            manifest.replace("\"postgres\"]", "\"postgres\", \"i18n\"]")
        );
        assert_eq!(
            removed,
            manifest.replace("[\"engine\", \"postgres\"]", "[\"postgres\", \"i18n\"]")
        );
        assert!(
            !edited_manifest(manifest, "engine", FeatureChange::Add)
                .unwrap()
                .changed
        );
    }

    #[test]
    fn a_dependency_without_features_gets_a_list_in_its_own_form() {
        let inline = "[dependencies]\nsofie = { workspace = true }\n";
        let dotted = "[dependencies]\nsofie.workspace = true\nserde = \"1\"\n";
        let table = "[dependencies.sofie]\nworkspace = true\n";

        assert_eq!(
            edited(inline, "i18n", FeatureChange::Add),
            "[dependencies]\nsofie = { workspace = true, features = [\"i18n\"] }\n"
        );
        assert_eq!(
            edited(dotted, "i18n", FeatureChange::Add),
            "[dependencies]\nsofie.workspace = true\nsofie.features = [\"i18n\"]\nserde = \"1\"\n"
        );
        assert_eq!(
            edited(table, "i18n", FeatureChange::Add),
            "[dependencies.sofie]\nfeatures = [\"i18n\"]\nworkspace = true\n"
        );
    }

    #[test]
    fn a_list_over_several_lines_stays_one_feature_a_line() {
        let manifest = "[dependencies.sofie]\nworkspace = true\nfeatures = [\n    \"engine\",\n    \
                        \"postgres\"\n]\n";

        assert_eq!(
            edited(manifest, "i18n", FeatureChange::Add),
            "[dependencies.sofie]\nworkspace = true\nfeatures = [\n    \"engine\",\n    \
             \"postgres\",\n    \"i18n\",\n]\n"
        );
    }

    #[test]
    fn what_cannot_be_edited_safely_is_refused() {
        let commented = "[dependencies]\nsofie.features = [\n  \"engine\", # the screens\n]\n";
        let missing = "[dependencies]\nserde = \"1\"\n";
        let version = "[dependencies]\nsofie = \"0.1\"\n";
        let listed =
            "[dependencies]\nsofie = { default-features = false, features = [\"engine\"] }\n";

        let refusal = |text, change| edited_manifest(text, "i18n", change).unwrap_err();

        assert!(refusal(commented, FeatureChange::Add).contains("by hand"));
        assert!(refusal(missing, FeatureChange::Add).contains("no `sofie` dependency"));
        assert!(refusal(version, FeatureChange::Add).contains("inline table"));
        assert!(refusal(listed, FeatureChange::Remove).contains("does not list"));
    }
}
