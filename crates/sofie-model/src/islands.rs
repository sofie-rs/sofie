use std::fs;
use std::path::{Path, PathBuf};

use syn::ExprMethodCall;

use crate::load::{files_with_extension, subdirectories};
use crate::naming::file_stem;
use crate::syntax::string_literal;
use crate::{AreaContribution, ModelError, Module, Route};

const EXTENSION: &str = "svelte";
const PROPS_CALL: &str = "$props()";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Island {
    pub name: String,
    pub file: PathBuf,
    pub area: Option<String>,
    pub props: Option<PropsDeclaration>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropsDeclaration {
    pub names: Vec<String>,
    pub type_name: Option<String>,
    pub line: usize,
}

#[derive(Debug)]
pub struct IslandBundle<'m> {
    pub area: String,
    pub islands: Vec<&'m Island>,
}

/// A module's `web/` islands belong to every area a module routes in.
#[must_use]
pub fn island_bundles(modules: &[Module]) -> Vec<IslandBundle<'_>> {
    let islands: Vec<&Island> = modules.iter().flat_map(|module| &module.islands).collect();
    let shared = islands.iter().any(|island| island.area.is_none());
    let mut areas: Vec<&str> = islands
        .iter()
        .filter_map(|island| island.area.as_deref())
        .chain(
            modules
                .iter()
                .flat_map(|module| &module.areas)
                .filter(|_| shared)
                .map(|area| area.area.as_str()),
        )
        .collect();
    areas.sort_unstable();
    areas.dedup();
    areas
        .into_iter()
        .map(|area| {
            let own = islands
                .iter()
                .filter(|island| island.area.as_deref() == Some(area));
            let everywhere = islands.iter().filter(|island| island.area.is_none());
            IslandBundle {
                area: area.to_owned(),
                islands: own.chain(everywhere).copied().collect(),
            }
        })
        .collect()
}

/// Keyed by the area's name, which a `#[path]` may set apart from its folder's.
pub(crate) fn read_islands(
    directory: &Path,
    areas: &[AreaContribution],
) -> Result<Vec<Island>, ModelError> {
    let mut islands = Vec::new();
    for area_dir in subdirectories(&directory.join("src").join("areas"))? {
        let contribution = areas
            .iter()
            .find(|area| area.routes_file.parent() == Some(area_dir.as_path()));
        let area = contribution
            .map(|area| area.area.as_str())
            .or_else(|| area_dir.file_name().and_then(|name| name.to_str()));
        if let Some(area) = area {
            islands.extend(islands_in(&area_dir.join("islands"), Some(area))?);
        }
    }
    islands.extend(islands_in(&directory.join("web"), None)?);
    Ok(islands)
}

fn islands_in(directory: &Path, area: Option<&str>) -> Result<Vec<Island>, ModelError> {
    files_with_extension(directory, EXTENSION)?
        .into_iter()
        .map(|file| {
            let source = fs::read_to_string(&file).map_err(|error| {
                ModelError::in_file(&file, format!("cannot read the island: {error}"))
            })?;
            Ok(Island {
                name: file_stem(&file),
                area: area.map(str::to_owned),
                props: props_declaration(&source),
                file,
            })
        })
        .collect()
}

fn props_declaration(source: &str) -> Option<PropsDeclaration> {
    let call = source.find(PROPS_CALL)?;
    let before = &source[..call];
    let declaration = before.rfind("let ")?;
    let written = &before[declaration + "let ".len()..];
    let open = written.find('{')?;
    let close = written.rfind('}')?;
    let names = written
        .get(open + 1..close)?
        .split(',')
        .filter_map(prop_name)
        .collect();
    let type_name = written[close + 1..]
        .trim()
        .strip_prefix(':')
        .and_then(|annotation| annotation.trim().strip_suffix('='))
        .map(|annotation| annotation.trim().to_owned());
    Some(PropsDeclaration {
        names,
        type_name,
        line: source[..declaration].matches('\n').count() + 1,
    })
}

fn prop_name(binding: &str) -> Option<String> {
    let binding = binding.trim();
    if binding.is_empty() || binding.starts_with("...") {
        return None;
    }
    let name = binding.split([':', '=']).next()?.trim();
    Some(name.to_owned())
}

/// Names the HTML standard reserves although they contain a hyphen.
const RESERVED_NAMES: [&str; 8] = [
    "annotation-xml",
    "color-profile",
    "font-face",
    "font-face-src",
    "font-face-uri",
    "font-face-format",
    "font-face-name",
    "missing-glyph",
];

/// Must match sofie-engine's `is_island_name`, which refuses the island at boot;
/// sofie-check's tests compare the two.
#[must_use]
pub fn is_island_name(name: &str) -> bool {
    name.starts_with(|first: char| first.is_ascii_lowercase())
        && name.contains('-')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !RESERVED_NAMES.contains(&name)
}

pub(crate) fn island_of(call: &ExprMethodCall, line: usize) -> Result<Route, String> {
    let shape = || {
        "write the island as `.island(\"/path\", \"name\", loader)`: a literal path, the literal \
         name of `islands/<name>.svelte` beside this file and a loader, nothing else"
            .to_owned()
    };
    if call.turbofish.is_some() || call.args.len() != 3 {
        return Err(shape());
    }
    let name = string_literal(&call.args[1])
        .filter(|name| is_island_name(name))
        .ok_or_else(|| {
            "the island name must be a string literal in lowercase with a hyphen, such as \
             `order-chart`, naming `islands/<name>.svelte` beside this file"
                .to_owned()
        })?;
    let mut route = crate::routes::page_route(call, line)?;
    route.island = Some(name);
    Ok(route)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_props_an_island_destructures_and_their_type() {
        let source = "<script lang=\"ts\">\n  import type { OrderSummary } from \"./props\";\n  \
                      let { total, count: orders = 0, ...rest }: OrderSummary = $props();\n</script>\n";

        let declaration = props_declaration(source).unwrap();

        assert_eq!(declaration.names, ["total", "count"]);
        assert_eq!(declaration.type_name.as_deref(), Some("OrderSummary"));
        assert_eq!(declaration.line, 3);
    }

    #[test]
    fn an_island_without_props_or_type_says_so() {
        assert_eq!(props_declaration("<p>static</p>"), None);
        let untyped = props_declaration("<script>let { a } = $props();</script>").unwrap();
        assert_eq!(untyped.names, ["a"]);
        assert_eq!(untyped.type_name, None);
    }

    fn island(name: &str, area: Option<&str>) -> Island {
        Island {
            name: name.to_owned(),
            file: PathBuf::from(format!("{name}.svelte")),
            area: area.map(str::to_owned),
            props: None,
        }
    }

    #[test]
    fn a_web_island_is_in_every_areas_bundle() {
        let mut order = crate::testing::module("order", "modules/order");
        order.islands = vec![
            island("order-chart", Some("member")),
            island("price-tag", None),
        ];
        let console = crate::testing::area(&order, "console", Vec::new());
        order.areas.push(console);
        let modules = [order];

        let bundles = island_bundles(&modules);

        let shown: Vec<(&str, Vec<&str>)> = bundles
            .iter()
            .map(|bundle| {
                let names = bundle
                    .islands
                    .iter()
                    .map(|island| island.name.as_str())
                    .collect();
                (bundle.area.as_str(), names)
            })
            .collect();
        assert_eq!(
            shown,
            [
                ("console", vec!["price-tag"]),
                ("member", vec!["order-chart", "price-tag"]),
            ]
        );
    }

    #[test]
    fn island_names_are_custom_element_names() {
        assert!(is_island_name("order-chart"));
        assert!(!is_island_name("chart"));
        assert!(!is_island_name("Order-chart"));
        assert!(!is_island_name("order_chart"));
        assert!(!is_island_name("font-face"));
    }
}
