use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use syn::{Attribute, Fields, Item, ItemStruct, Type};

use crate::ModelError;
use crate::code::derives;
use crate::syntax::{doc_lines, first_type_argument, line_of, parsed_or_read, type_last_name};

pub const DATABASE_URL: &str = "DATABASE_URL";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleConfig {
    pub type_name: String,
    pub file: PathBuf,
    pub line: usize,
    pub declared: bool,
    pub keys: Vec<ConfigKey>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigKey {
    pub name: String,
    pub doc: Vec<String>,
    pub kind: KeyKind,
    pub required: bool,
    pub fallback: Option<String>,
    /// Declared `Secret<…>`, so it is never logged or printed.
    pub secret: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyKind {
    /// A value type's own rules are checked at boot.
    Text,
    Boolean,
    Integer,
    Count,
    Decimal,
    Switch,
    Address,
}

impl KeyKind {
    fn of(type_name: &str) -> Self {
        match type_name {
            "bool" => Self::Boolean,
            "i8" | "i16" | "i32" | "i64" | "i128" | "isize" => Self::Integer,
            "u8" | "u16" | "u32" | "u64" | "u128" | "usize" => Self::Count,
            "f32" | "f64" => Self::Decimal,
            _ => Self::Text,
        }
    }

    /// Must match how the loader parses it.
    #[must_use]
    pub fn accepts(self, value: &str) -> bool {
        match self {
            Self::Text => true,
            Self::Boolean => value.parse::<bool>().is_ok(),
            Self::Integer => value.parse::<i128>().is_ok(),
            Self::Count => value.parse::<u128>().is_ok(),
            Self::Decimal => value.parse::<f64>().is_ok(),
            Self::Switch => matches!(value, "on" | "off"),
            Self::Address => value.parse::<std::net::SocketAddr>().is_ok(),
        }
    }

    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Boolean => "`true` or `false`",
            Self::Integer => "a whole number",
            Self::Count => "a whole number of 0 or more",
            Self::Decimal => "a number",
            Self::Switch => "`on` or `off`",
            Self::Address => "an address such as 127.0.0.1:8080",
        }
    }
}

pub(crate) fn read_config(
    directory: &Path,
    crate_name: &str,
    syntax: &BTreeMap<PathBuf, syn::File>,
) -> Result<Option<ModuleConfig>, ModelError> {
    let file = directory.join("src").join("config.rs");
    if !file.is_file() {
        return Ok(None);
    }
    let parsed = parsed_or_read(syntax, &file)?;
    let structs = structs_of(&parsed);
    let type_name = config_type_name(crate_name);
    let root = structs.get(&type_name).ok_or_else(|| {
        ModelError::in_file(
            &file,
            format!("declare the module's config as `#[derive(sofie::Config)] pub struct {type_name} {{ … }}`"),
        )
    })?;
    let mut keys = Vec::new();
    Group {
        structs: &structs,
        file: &file,
    }
    .collect(root, &crate_name.to_ascii_uppercase(), false, &mut keys)?;
    Ok(Some(ModuleConfig {
        type_name,
        line: line_of(&root.ident),
        declared: syntax.contains_key(&file),
        file,
        keys,
    }))
}

pub(crate) fn settings_keys(
    syntax: &BTreeMap<PathBuf, syn::File>,
    settings: Option<&str>,
    prefix: &str,
) -> Result<Vec<ConfigKey>, ModelError> {
    let mut keys = Vec::new();
    for (file, parsed) in syntax {
        let structs = structs_of(parsed);
        if let Some(root) = settings.and_then(|name| structs.get(name)) {
            Group {
                structs: &structs,
                file,
            }
            .collect(root, prefix, false, &mut keys)?;
            break;
        }
    }
    Ok(keys)
}

fn structs_of(parsed: &syn::File) -> BTreeMap<String, &ItemStruct> {
    parsed
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Struct(declared) if derives(&declared.attrs, "Config") => {
                Some((declared.ident.to_string(), declared))
            }
            _ => None,
        })
        .collect()
}

#[must_use]
pub fn config_type_name(crate_name: &str) -> String {
    crate::naming::pascal_case(crate_name) + "Config"
}

struct Group<'file> {
    structs: &'file BTreeMap<String, &'file ItemStruct>,
    file: &'file Path,
}

impl Group<'_> {
    fn collect(
        &self,
        declared: &ItemStruct,
        prefix: &str,
        optional: bool,
        keys: &mut Vec<ConfigKey>,
    ) -> Result<(), ModelError> {
        let Fields::Named(fields) = &declared.fields else {
            return Err(ModelError::new(
                self.file,
                line_of(&declared.ident),
                "a config struct has named fields, one per key",
            ));
        };
        for field in &fields.named {
            let Some(ident) = &field.ident else { continue };
            let name = format!("{prefix}_{}", ident.to_string().to_ascii_uppercase());
            let (field_type, is_option) = unwrapped(&field.ty, "Option");
            let (field_type, secret) = unwrapped(field_type, "Secret");
            let optional = optional || is_option;
            let type_name = type_last_name(field_type).unwrap_or_default();
            if let Some(nested) = self.structs.get(&type_name) {
                self.collect(nested, &name, optional, keys)?;
            } else if matches!(type_name.as_str(), "Database" | "Queue") {
                keys.push(ConfigKey {
                    secret: true,
                    ..key(
                        format!("{name}_URL"),
                        &field.attrs,
                        KeyKind::Text,
                        false,
                        Some(DATABASE_URL),
                    )
                });
            } else {
                keys.push(ConfigKey {
                    secret,
                    ..key(name, &field.attrs, KeyKind::of(&type_name), !optional, None)
                });
            }
        }
        Ok(())
    }
}

fn key(
    name: String,
    attributes: &[Attribute],
    kind: KeyKind,
    required: bool,
    fallback: Option<&str>,
) -> ConfigKey {
    ConfigKey {
        name,
        doc: doc_lines(attributes),
        kind,
        required,
        fallback: fallback.map(str::to_owned),
        secret: false,
    }
}

fn unwrapped<'a>(written: &'a Type, wrapper: &str) -> (&'a Type, bool) {
    if let Type::Path(path) = written
        && let Some(last) = path.path.segments.last()
        && last.ident == wrapper
        && let Some(inner) = first_type_argument(last)
    {
        return (inner, true);
    }
    (written, false)
}

#[cfg(test)]
mod tests {
    use super::config_type_name;

    #[test]
    fn the_config_type_is_named_after_the_module() {
        assert_eq!(config_type_name("order"), "OrderConfig");
        assert_eq!(config_type_name("sofie_identity"), "SofieIdentityConfig");
    }
}
