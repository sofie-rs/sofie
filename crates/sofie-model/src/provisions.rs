use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use syn::ext::IdentExt;
use syn::{Fields, Item, ItemImpl, Type};

use crate::areas::{associated_type, constant_value};
use crate::handlers::HandlerScope;
use crate::syntax::{last_name, line_of, public_uses, segments, string_literal};
use crate::{Crate, Provision};

pub(crate) struct ImplSite {
    pub(crate) trait_path: Vec<String>,
    pub(crate) ty: Vec<String>,
    pub(crate) module_path: Vec<String>,
    pub(crate) file: PathBuf,
    pub(crate) line: usize,
    pub(crate) vendor: Option<String>,
    pub(crate) settings: Option<String>,
}

pub(crate) fn impl_site(
    implementation: &ItemImpl,
    file: &Path,
    module_path: &[String],
) -> Option<ImplSite> {
    let (trait_path, _) = implementation.trait_.as_ref()?;
    let Type::Path(ty) = implementation.self_ty.as_ref() else {
        return None;
    };
    if !implementation.generics.params.is_empty() {
        return None;
    }
    Some(ImplSite {
        trait_path: segments(trait_path),
        ty: segments(&ty.path),
        module_path: module_path.to_vec(),
        file: file.to_path_buf(),
        line: line_of(implementation),
        vendor: constant_value(implementation, "VENDOR").and_then(string_literal),
        settings: associated_type(implementation, "Settings").and_then(last_name),
    })
}

pub(crate) fn unit_struct(item: &Item, module_path: &[String]) -> Option<Vec<String>> {
    match item {
        Item::Struct(structure) if matches!(structure.fields, Fields::Unit) => {
            let name = structure.ident.unraw().to_string();
            Some([module_path, std::slice::from_ref(&name)].concat())
        }
        _ => None,
    }
}

pub(crate) struct RootExports {
    named: BTreeMap<Vec<String>, String>,
    globs: Vec<Vec<String>>,
}

impl RootExports {
    pub(crate) fn of(root: Option<&syn::File>) -> Self {
        let items = root.map(|root| root.items.as_slice()).unwrap_or_default();
        let mut exports = Self {
            named: BTreeMap::new(),
            globs: Vec::new(),
        };
        for used in public_uses(items) {
            let mut path = used.segments;
            if matches!(path.first().map(String::as_str), Some("crate" | "self")) {
                path.remove(0);
            }
            match used.binding {
                Some(binding) => {
                    exports.named.insert(path, binding);
                }
                None => exports.globs.push(path),
            }
        }
        exports
    }

    pub(crate) fn public(&self, path: &[String]) -> Vec<String> {
        if let Some(name) = self.named.get(path) {
            return vec![name.clone()];
        }
        match path.split_last() {
            Some((name, module)) if self.globs.iter().any(|glob| glob == module) => {
                vec![name.clone()]
            }
            _ => path.to_vec(),
        }
    }
}

pub(crate) fn provisions(
    sites: &[ImplSite],
    units: &[Vec<String>],
    syntax: &BTreeMap<PathBuf, syn::File>,
    exports: &RootExports,
    krate: &Crate,
    contract: Option<&str>,
) -> (Vec<Provision>, Vec<Provision>) {
    let (mut implementations, mut extensions) = (Vec::new(), Vec::new());
    for site in sites {
        let items = syntax.get(&site.file).map_or(&[][..], |file| &file.items);
        let scope = HandlerScope::new(site.module_path.clone(), items);
        let Ok(trait_path) = scope.resolve(&site.trait_path) else {
            continue;
        };
        let ty = scope.within(&site.ty);
        let Some(first) = trait_path.first().filter(|_| trait_path.len() > 1) else {
            continue;
        };
        let provision = Provision {
            unit: units.contains(&ty),
            ty: exports.public(&ty),
            contract: trait_path.clone(),
            file: site.file.clone(),
            line: site.line,
        };
        if Some(first.as_str()) == contract {
            implementations.push(provision);
        } else if site
            .module_path
            .first()
            .is_some_and(|module| module == "extensions")
            && krate
                .dependencies
                .iter()
                .any(|dependency| &dependency.crate_name == first)
        {
            extensions.push(provision);
        }
    }
    (implementations, extensions)
}
