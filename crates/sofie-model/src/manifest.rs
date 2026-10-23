use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use syn::{Expr, ExprStruct, Item, ItemConst, Member};

use crate::ModelError;
use crate::syntax::{last_name, line_of, parsed_or_read, string_literal};

pub const MANIFEST_CONSTANT: &str = "MODULE";
const EXAMPLE: &str = "declare `pub const MODULE: sofie::Module = sofie::Module { name: \"order\", \
                       tier: sofie::Tier::Core, context: None, settings: &[], \
                       ..crate::wiring::GENERATED };`";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub name: String,
    pub tier: Tier,
    pub context: Option<String>,
    pub settings: Vec<String>,
    pub file: PathBuf,
    pub line: usize,
    /// The compiler only checks a manifest it builds.
    pub declared: bool,
}

/// Ordered lowest first, so a module may depend on any tier `<=` its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Platform,
    Core,
    Experience,
}

impl Tier {
    const ALL: [Self; 3] = [Self::Platform, Self::Core, Self::Experience];

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Platform => "Platform",
            Self::Core => "Core",
            Self::Experience => "Experience",
        }
    }

    #[must_use]
    pub fn may_depend_on(self, other: Self) -> bool {
        other <= self
    }

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|tier| tier.name() == name)
    }
}

#[must_use]
pub fn manifest_file(directory: &Path) -> PathBuf {
    directory.join("src").join("module.rs")
}

pub(crate) fn is_manifest_constant(file: &Path, name: &str) -> bool {
    name == MANIFEST_CONSTANT && file.ends_with("src/module.rs")
}

pub(crate) fn read_manifest(
    directory: &Path,
    syntax: &BTreeMap<PathBuf, syn::File>,
) -> Result<Option<Manifest>, ModelError> {
    let file = manifest_file(directory);
    if !file.is_file() {
        return Ok(None);
    }
    let declared = syntax.contains_key(&file);
    let parsed = parsed_or_read(syntax, &file)?;
    let constant = parsed
        .items
        .iter()
        .find_map(|item| match item {
            Item::Const(constant) if constant.ident == MANIFEST_CONSTANT => Some(constant),
            _ => None,
        })
        .ok_or_else(|| ModelError::in_file(&file, EXAMPLE))?;
    let mut manifest = manifest_of(constant).map_err(|message| {
        ModelError::new(&file, line_of(constant), format!("{message}; {EXAMPLE}"))
    })?;
    manifest.file = file;
    manifest.declared = declared;
    Ok(Some(manifest))
}

fn manifest_of(constant: &ItemConst) -> Result<Manifest, String> {
    let Expr::Struct(fields) = constant.expr.as_ref() else {
        return Err("the manifest must be a `Module { … }` literal".to_owned());
    };
    let name = field(fields, "name")
        .and_then(string_literal)
        .ok_or("`name` must be a string literal")?;
    let tier = field(fields, "tier")
        .and_then(last_path_segment)
        .as_deref()
        .and_then(Tier::from_name)
        .ok_or("`tier` must be `Tier::Experience`, `Tier::Core` or `Tier::Platform`")?;
    let context = field(fields, "context").map_or(Ok(None), context_of)?;
    let settings = field(fields, "settings").and_then(setting_names).ok_or(
        "`settings` must be `&[]` or `&[Setting { name: \"…\", … }, …]` with literal names",
    )?;
    Ok(Manifest {
        name,
        tier,
        context,
        settings,
        file: PathBuf::new(),
        line: line_of(constant),
        declared: false,
    })
}

fn field<'a>(fields: &'a ExprStruct, name: &str) -> Option<&'a Expr> {
    fields.fields.iter().find_map(|field| match &field.member {
        Member::Named(ident) if ident == name => Some(&field.expr),
        _ => None,
    })
}

fn last_path_segment(expression: &Expr) -> Option<String> {
    let Expr::Path(path) = expression else {
        return None;
    };
    last_name(&path.path)
}

fn context_of(expression: &Expr) -> Result<Option<String>, &'static str> {
    const REFUSAL: &str = "`context` must be `None` or `Some(\"name\")` with a literal name";
    if last_path_segment(expression).as_deref() == Some("None") {
        return Ok(None);
    }
    let Expr::Call(call) = expression else {
        return Err(REFUSAL);
    };
    if last_path_segment(&call.func).as_deref() != Some("Some") || call.args.len() != 1 {
        return Err(REFUSAL);
    }
    call.args
        .first()
        .and_then(string_literal)
        .map(Some)
        .ok_or(REFUSAL)
}

fn setting_names(expression: &Expr) -> Option<Vec<String>> {
    let Expr::Reference(reference) = expression else {
        return None;
    };
    let Expr::Array(settings) = reference.expr.as_ref() else {
        return None;
    };
    settings
        .elems
        .iter()
        .map(|setting| match setting {
            Expr::Struct(setting) => field(setting, "name").and_then(string_literal),
            _ => None,
        })
        .collect()
}
