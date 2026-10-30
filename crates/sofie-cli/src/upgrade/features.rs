use std::fs;
use std::path::{Path, PathBuf};

use super::{Change, Planned};
use crate::gen_command::{GenError, io_error};
use crate::source_edits::{InlineFeatures, inline_features, read};

const NAME: &str = "concurrent-indexes-dropped";
const CHANGE: &str =
    "11fce7b: `concurrent-indexes` is gone from `sofie`; concurrent index builds run without it";

const DROPPED: &str = "\"concurrent-indexes\"";

/// Found by walking the directory, as the model cannot load before they are upgraded.
pub(super) fn planned(root: &Path) -> Result<Planned, GenError> {
    let mut planned = Planned::default();
    for manifest in manifests(root)? {
        let Some((upgraded, changes)) = upgraded(&read(&manifest)?) else {
            continue;
        };
        planned
            .changes
            .extend(changes.into_iter().map(|change| Change {
                file: manifest.clone(),
                ..change
            }));
        planned.files.push((manifest, upgraded));
    }
    Ok(planned)
}

fn manifests(directory: &Path) -> Result<Vec<PathBuf>, GenError> {
    let mut entries: Vec<PathBuf> = fs::read_dir(directory)
        .and_then(|entries| {
            entries
                .map(|entry| entry.map(|entry| entry.path()))
                .collect()
        })
        .map_err(|source| io_error(directory, source))?;
    entries.sort();
    let mut found = Vec::new();
    for path in entries {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let is_directory = fs::symlink_metadata(&path)
            .map_err(|source| io_error(&path, source))?
            .is_dir();
        if is_directory && name != "target" && name != "node_modules" && !name.starts_with('.') {
            found.extend(manifests(&path)?);
        } else if name == "Cargo.toml" {
            found.push(path);
        }
    }
    Ok(found)
}

/// The manifest without the dropped feature, and a change per line it rewrote. A feature
/// list written over several lines is left to the build's own error.
fn upgraded(manifest: &str) -> Option<(String, Vec<Change>)> {
    let mut text = String::with_capacity(manifest.len());
    let mut changes = Vec::new();
    for (index, line) in manifest.split_inclusive('\n').enumerate() {
        let content = line.trim_end_matches(['\r', '\n']);
        let Some(after) = without_dropped(content) else {
            text.push_str(line);
            continue;
        };
        text.push_str(&after);
        text.push_str(&line[content.len()..]);
        changes.push(Change {
            file: PathBuf::new(),
            codemod: NAME,
            change: CHANGE,
            line: index + 1,
            before: content.trim().to_owned(),
            after: after.trim().to_owned(),
        });
    }
    (!changes.is_empty()).then_some((text, changes))
}

fn without_dropped(line: &str) -> Option<String> {
    let names_sofie = line
        .trim_start()
        .strip_prefix("sofie")
        .is_some_and(|rest| rest.starts_with([' ', '=', '.']));
    if !names_sofie || !line.contains(DROPPED) {
        return None;
    }
    let InlineFeatures::Array(array) = inline_features(line) else {
        return None;
    };
    let (open, close) = (array.start + 1, array.end - 1);
    let listed: Vec<&str> = line[open..close].split(',').map(str::trim).collect();
    if !listed.contains(&DROPPED) {
        return None;
    }
    let kept: Vec<&str> = listed
        .into_iter()
        .filter(|feature| !feature.is_empty() && *feature != DROPPED)
        .collect();
    Some(format!(
        "{}{}{}",
        &line[..open],
        kept.join(", "),
        &line[close..]
    ))
}

#[cfg(test)]
mod tests {
    use sofie_model::testing::TemporaryDirectory;

    use super::{manifests, upgraded};

    #[cfg(unix)]
    #[test]
    fn the_manifest_walk_does_not_follow_a_symlinked_directory() {
        let directory = TemporaryDirectory::new("sofie-upgrade-symlink");
        directory.write("app/Cargo.toml", "[package]\n");
        std::os::unix::fs::symlink(directory.root(), directory.path("app/loop")).unwrap();

        let found = manifests(directory.root()).unwrap();

        assert_eq!(found, [directory.path("app/Cargo.toml")]);
    }

    #[test]
    fn drops_the_feature_only_from_a_sofie_dependency() {
        let manifest = "[dependencies]\nsofie = { workspace = true, features = [\"concurrent-indexes\", \"search\"] }\n\
                        sofie-db = { workspace = true, features = [\"concurrent-indexes\"] }\n";

        let (text, changes) = upgraded(manifest).unwrap();

        assert_eq!(
            text,
            "[dependencies]\nsofie = { workspace = true, features = [\"search\"] }\n\
             sofie-db = { workspace = true, features = [\"concurrent-indexes\"] }\n"
        );
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].line, 2);
        let tight = "[dependencies]\nsofie = {workspace=true,features=[\"concurrent-indexes\"]}\n";
        assert_eq!(
            upgraded(tight).unwrap().0,
            "[dependencies]\nsofie = {workspace=true,features=[]}\n"
        );
        assert!(
            upgraded("[dependencies]\nsofie = { workspace = true } # \"concurrent-indexes\"\n")
                .is_none()
        );
    }
}
