use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sofie_model::ModelError;

pub const ALLOW_FILE: &str = "dead-code-allow.toml";

/// `dead-code-allow.toml`: `[[item]]` and `[[dependency]]` entries.
///
/// ```toml
/// [[item]]
/// package = "sofie-http"
/// name = "Screens"
/// reason = "Why it stays although nothing names it."
/// ```
pub struct AllowList {
    pub(super) file: PathBuf,
    pub(super) allowances: Vec<Allowance>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Allowed {
    Item,
    Dependency,
}

pub(super) struct Allowance {
    pub kind: Allowed,
    pub package: String,
    pub name: String,
    pub line: usize,
}

impl AllowList {
    pub fn read(root: &Path) -> Result<Self, ModelError> {
        let file = root.join(ALLOW_FILE);
        let text = read_optional(&file)?.unwrap_or_default();
        Self::parse(&file, &text)
    }

    fn parse(file: &Path, text: &str) -> Result<Self, ModelError> {
        let allowances = parse_entries(file, text, &["item", "dependency"], &["package", "name"])?
            .into_iter()
            .map(|entry| {
                Ok(Allowance {
                    kind: if entry.kind == "item" {
                        Allowed::Item
                    } else {
                        Allowed::Dependency
                    },
                    package: entry.required(file, "package")?,
                    name: entry.required(file, "name")?,
                    line: entry.line,
                })
            })
            .collect::<Result<_, ModelError>>()?;
        Ok(Self {
            file: file.to_path_buf(),
            allowances,
        })
    }

    /// An `[[item]]` entry to append to the file.
    #[must_use]
    pub fn item_entry(package: &str, name: &str, reason: &str) -> String {
        format!("\n[[item]]\npackage = \"{package}\"\nname = \"{name}\"\nreason = \"{reason}\"\n")
    }

    /// Where the `[[item]]` entry allowing `name` of `package` starts, if there is one.
    #[must_use]
    pub fn item_line(&self, package: &str, name: &str) -> Option<usize> {
        self.allowances
            .iter()
            .find(|allowance| {
                allowance.kind == Allowed::Item
                    && allowance.package == package
                    && allowance.name == name
            })
            .map(|allowance| allowance.line)
    }

    /// The file's text without the entries starting at `lines` (from 1), each
    /// up to the next entry, with the blank lines before it.
    #[must_use]
    pub fn without_entries(text: &str, lines: &[usize]) -> String {
        let all: Vec<&str> = text.split_inclusive('\n').collect();
        let mut kept: Vec<&str> = Vec::with_capacity(all.len());
        let mut dropping = false;
        for (index, line) in all.iter().enumerate() {
            if line.trim_start().starts_with("[[") {
                dropping = lines.contains(&(index + 1));
                if dropping {
                    while kept.last().is_some_and(|line| line.trim().is_empty()) {
                        kept.pop();
                    }
                }
            }
            if !dropping {
                kept.push(line);
            }
        }
        kept.concat()
    }

    pub(super) fn allows(&self, kind: Allowed, package: &str, name: &str) -> bool {
        self.allowances.iter().any(|allowance| {
            allowance.kind == kind && allowance.package == package && allowance.name == name
        })
    }
}

pub(super) fn read_optional(file: &Path) -> Result<Option<String>, ModelError> {
    match fs::read_to_string(file) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ModelError::in_file(
            file,
            format!("cannot read the file: {error}"),
        )),
    }
}

pub(super) struct Entry {
    pub kind: String,
    pub line: usize,
    values: Vec<(String, String)>,
}

impl Entry {
    pub(super) fn value(&self, key: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    pub(super) fn required(&self, file: &Path, key: &str) -> Result<String, ModelError> {
        self.value(key).map(str::to_owned).ok_or_else(|| {
            ModelError::new(file, self.line, format!("this entry needs `{key} = \"…\"`"))
        })
    }

    fn set(&mut self, line: &str, keys: &[&str]) -> Result<(), String> {
        let (key, value) = line.split_once('=').ok_or("write `key = \"value\"`")?;
        let key = key.trim();
        let value = plain_string(value.trim())
            .ok_or("write the value as a quoted string without quotes or backslashes inside")?;
        if key != "reason" && !keys.contains(&key) {
            let names: Vec<String> = keys.iter().map(|key| format!("`{key}`")).collect();
            return Err(format!("the keys are {} and `reason`", names.join(", ")));
        }
        if self.value(key).is_some() {
            return Err("this key is already set in this entry".to_owned());
        }
        self.values.push((key.to_owned(), value));
        Ok(())
    }
}

/// Read without a TOML library: the file holds only `[[kind]]` tables of quoted strings.
pub(super) fn parse_entries(
    file: &Path,
    text: &str,
    kinds: &[&str],
    keys: &[&str],
) -> Result<Vec<Entry>, ModelError> {
    let mut entries: Vec<Entry> = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let error = |message: String| ModelError::new(file, index + 1, message);
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(kind) = table_kind(line, kinds) {
            entries.push(Entry {
                kind: kind.to_owned(),
                line: index + 1,
                values: Vec::new(),
            });
            continue;
        }
        let entry = entries.last_mut().ok_or_else(|| {
            let tables: Vec<String> = kinds.iter().map(|kind| format!("`[[{kind}]]`")).collect();
            error(format!("start an entry with {}", tables.join(" or ")))
        })?;
        entry.set(line, keys).map_err(error)?;
    }
    for entry in &entries {
        let reason = entry.required(file, "reason")?;
        if reason.trim().is_empty() {
            return Err(ModelError::new(
                file,
                entry.line,
                "this entry needs `reason = \"…\"`",
            ));
        }
    }
    Ok(entries)
}

fn table_kind<'kind>(line: &str, kinds: &[&'kind str]) -> Option<&'kind str> {
    let name = line.strip_prefix("[[")?.strip_suffix("]]")?;
    kinds.iter().copied().find(|kind| *kind == name)
}

fn plain_string(value: &str) -> Option<String> {
    let inner = value.strip_prefix('"')?.strip_suffix('"')?;
    (!inner.contains(['"', '\\'])).then(|| inner.to_owned())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{AllowList, Allowed};

    fn parse(text: &str) -> Result<AllowList, String> {
        AllowList::parse(Path::new("dead-code-allow.toml"), text).map_err(|error| error.to_string())
    }

    #[test]
    fn reads_items_and_dependencies_with_their_reasons() {
        let list = parse(
            "# Exceptions.\n\n[[item]]\npackage = \"sofie-http\"\nname = \"Screens\"\nreason = \"x\"\n\n\
             [[dependency]]\npackage = \"sofie\"\nname = \"tokio\"\nreason = \"y\"\n",
        )
        .unwrap();

        assert!(list.allows(Allowed::Item, "sofie-http", "Screens"));
        assert!(list.allows(Allowed::Dependency, "sofie", "tokio"));
        assert!(!list.allows(Allowed::Item, "sofie", "tokio"));
        assert_eq!(list.allowances[1].line, 8);
    }

    #[test]
    fn an_appended_entry_reads_back_and_drops_out_by_its_line() {
        let text = format!(
            "# Exceptions.\n{}{}",
            AllowList::item_entry("shop", "Kept", "x"),
            AllowList::item_entry("shop-contract", "Scaffolded", "y")
        );
        let list = parse(&text).unwrap();

        let without = AllowList::without_entries(&text, &[list.allowances[1].line]);

        assert!(list.allows(Allowed::Item, "shop-contract", "Scaffolded"));
        assert_eq!(
            without,
            "# Exceptions.\n\n[[item]]\npackage = \"shop\"\nname = \"Kept\"\nreason = \"x\"\n"
        );
    }

    #[test]
    fn refuses_entries_without_a_reason_and_anything_else() {
        for (text, expected) in [
            (
                "[[item]]\npackage = \"a\"\nname = \"b\"\n",
                "dead-code-allow.toml:1: this entry needs `reason",
            ),
            (
                "package = \"a\"\n",
                "dead-code-allow.toml:1: start an entry",
            ),
            (
                "[[item]]\nwhy = \"a\"\n",
                "dead-code-allow.toml:2: the keys are",
            ),
            (
                "[[item]]\nname = 'a'\n",
                "dead-code-allow.toml:2: write the value",
            ),
            (
                "[[item]]\nname = \"a\"\nname = \"b\"\n",
                "dead-code-allow.toml:3: this key is already set",
            ),
        ] {
            let error = parse(text).err().unwrap_or_default();
            assert!(error.starts_with(expected), "{text}: {error}");
        }
    }
}
