use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use syn::{Item, ItemTrait};

use crate::config::settings_keys;
use crate::handlers::HandlerScope;
use crate::naming::snake_case;
use crate::provisions::{ImplSite, RootExports};
use crate::syntax::{has_attribute, line_of};
use crate::{ConfigKey, ModelError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gateway {
    pub name: String,
    pub path: Vec<String>,
    pub adapters: Vec<GatewayAdapter>,
    pub file: PathBuf,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GatewayAdapter {
    pub ty: Vec<String>,
    pub is_adapter: bool,
    pub vendor: Option<String>,
    pub settings: Vec<ConfigKey>,
    pub file: PathBuf,
    pub line: usize,
}

pub(crate) struct GatewayTrait {
    path: Vec<String>,
    file: PathBuf,
    line: usize,
}

pub(crate) fn gateway_trait(
    item: &Item,
    file: &Path,
    module_path: &[String],
) -> Option<GatewayTrait> {
    let Item::Trait(ItemTrait { attrs, ident, .. }) = item else {
        return None;
    };
    has_attribute(attrs, "gateway").then(|| GatewayTrait {
        path: [module_path, &[ident.to_string()]].concat(),
        file: file.to_path_buf(),
        line: line_of(item),
    })
}

pub(crate) fn gateways(
    traits: Vec<GatewayTrait>,
    sites: &[ImplSite],
    syntax: &BTreeMap<PathBuf, syn::File>,
    exports: &RootExports,
    module: &str,
) -> Result<Vec<Gateway>, ModelError> {
    let resolved: Vec<(Vec<String>, Vec<String>, &ImplSite)> = sites
        .iter()
        .map(|site| {
            let items = syntax.get(&site.file).map_or(&[][..], |file| &file.items);
            let scope = HandlerScope::new(site.module_path.clone(), items);
            (scope.callee(&site.trait_path), scope.within(&site.ty), site)
        })
        .collect();
    let adapters: Vec<(&Vec<String>, &ImplSite)> = resolved
        .iter()
        .filter(|(trait_path, _, _)| trait_path.last().is_some_and(|name| name == "Adapter"))
        .map(|(_, ty, site)| (ty, *site))
        .collect();
    let mut gateways = Vec::new();
    for found in traits {
        let mut implementations = Vec::new();
        for (_, ty, site) in resolved.iter().filter(|(path, _, _)| *path == found.path) {
            let adapter = adapters.iter().find(|(adapter, _)| *adapter == ty);
            let vendor = adapter.and_then(|(_, adapter)| adapter.vendor.clone());
            let settings = match (adapter, &vendor) {
                (Some((_, adapter)), Some(vendor)) => {
                    let prefix = format!("{module}_{vendor}").to_ascii_uppercase();
                    settings_keys(syntax, adapter.settings.as_deref(), &prefix)?
                }
                _ => Vec::new(),
            };
            implementations.push(GatewayAdapter {
                ty: exports.public(ty),
                is_adapter: adapter.is_some(),
                vendor,
                settings,
                file: site.file.clone(),
                line: site.line,
            });
        }
        gateways.push(Gateway {
            name: snake_case(found.path.last().map_or("", String::as_str)),
            adapters: implementations,
            path: exports.public(&found.path),
            file: found.file,
            line: found.line,
        });
    }
    Ok(gateways)
}
