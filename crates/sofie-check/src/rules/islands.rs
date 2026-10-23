use std::collections::HashSet;
use std::path::Path;

use sofie_model::{
    AreaContribution, Island, IslandBundle, Module, ReadModel, Route, TypeName, island_bundles,
};

use super::{Finding, Rule};
use crate::screens::Screens;

pub(super) fn check_islands(modules: &[Module], screens: &Screens) -> Vec<Finding> {
    let bundles = island_bundles(modules);
    let mut findings = duplicate_tags(&bundles);
    for module in modules {
        for area in &module.areas {
            for route in &area.routes {
                findings.extend(check_island_route(module, area, route));
            }
            for file in &area.screens {
                let Ok(screen) = screens.screen(file) else {
                    continue;
                };
                findings.extend(check_screen_islands(
                    &bundles,
                    &area.area,
                    file,
                    &screen.source,
                ));
            }
        }
    }
    findings
}

fn check_island_route(module: &Module, area: &AreaContribution, route: &Route) -> Vec<Finding> {
    let Some(name) = &route.island else {
        return Vec::new();
    };
    let Some(island) = own_island(module, &area.area, name) else {
        return vec![Finding {
            rule: Rule::Island,
            file: area.routes_file.clone(),
            line: route.line,
            message: format!(
                "route {} shows island `{name}`, but the module has no \
                 `src/areas/{}/islands/{name}.svelte` or `web/{name}.svelte`",
                route.path, area.area
            ),
            fix: format!("add `src/areas/{}/islands/{name}.svelte`", area.area),
        }];
    };
    let Some(declared) = &island.props else {
        return Vec::new();
    };
    let output = module
        .code
        .function(&route.handler)
        .and_then(|function| function.output.as_ref())
        .map(TypeName::without_result);
    let Some(model) = output.and_then(|output| module.code.read_model(&output.name)) else {
        return vec![props_finding(
            island,
            declared.line,
            format!(
                "island `{name}` reads props, but the loader `{}` of route {} returns no read \
                 model, so its props cannot be checked",
                route.handler.join("::"),
                route.path
            ),
            "return a `#[derive(ReadModel)]` from the loader; its fields are the props".to_owned(),
        )];
    };
    check_props(island, declared, model, &PropsGiven::Loader)
}

/// Where an island's read model comes from.
pub(super) enum PropsGiven {
    Loader,
    /// Described, as in "orders.html gives it `data.chart` from loader `…`".
    Screen(String),
}

/// An island embedded in a screen may type its props inline, as one several screens share does.
pub(super) fn check_props(
    island: &Island,
    declared: &sofie_model::PropsDeclaration,
    model: &ReadModel,
    given: &PropsGiven,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    let (typed_inline, given) = match given {
        PropsGiven::Loader => (false, "its loader returns"),
        PropsGiven::Screen(described) => (declared.type_name.is_none(), described.as_str()),
    };
    if !typed_inline && declared.type_name.as_deref() != Some(model.name.as_str()) {
        let written = declared
            .type_name
            .as_deref()
            .map_or_else(|| "no type".to_owned(), |name| format!("`{name}`"));
        findings.push(props_finding(
            island,
            declared.line,
            format!(
                "island `{}` declares its props as {written}, but {given} read model `{}`",
                island.name, model.name
            ),
            format!(
                "write `let {{ … }}: {} = $props();`, importing the type from the app's \
                 `web/gen/types.ts`",
                model.name
            ),
        ));
    }
    for name in &declared.names {
        if model.fields.iter().any(|field| &field.name == name) {
            continue;
        }
        let fields: Vec<&str> = model
            .fields
            .iter()
            .map(|field| field.name.as_str())
            .collect();
        findings.push(props_finding(
            island,
            declared.line,
            format!(
                "island `{}` reads prop `{name}`, which read model `{}` does not have; its \
                 fields are {}",
                island.name,
                model.name,
                fields.join(", ")
            ),
            format!("read one of the fields of `{}`", model.name),
        ));
    }
    findings
}

fn check_screen_islands(
    bundles: &[IslandBundle],
    area: &str,
    screen: &Path,
    source: &str,
) -> Vec<Finding> {
    island_names(source)
        .into_iter()
        .filter(|(name, _)| bundled_island(bundles, area, name).is_none())
        .map(|(name, line)| Finding {
            rule: Rule::Island,
            file: screen.to_path_buf(),
            line,
            message: format!(
                "`<Island name=\"{name}\">` names no island of area `{area}`: no module has \
                 `src/areas/{area}/islands/{name}.svelte` or `web/{name}.svelte`"
            ),
            fix: format!("add `islands/{name}.svelte` to a module's `{area}` area"),
        })
        .collect()
}

fn island_names(source: &str) -> Vec<(String, usize)> {
    let mut names = Vec::new();
    for (start, _) in source.match_indices("<Island") {
        let tag = &source[start..];
        let tag = &tag[..tag.find('>').unwrap_or(tag.len())];
        let Some(name) = tag
            .split_once("name=\"")
            .and_then(|(_, rest)| rest.split_once('"'))
            .map(|(name, _)| name.to_owned())
        else {
            continue;
        };
        names.push((name, source[..start].matches('\n').count() + 1));
    }
    names
}

fn duplicate_tags(bundles: &[IslandBundle]) -> Vec<Finding> {
    let mut findings = Vec::new();
    for bundle in bundles {
        let mut seen = HashSet::new();
        for island in &bundle.islands {
            if seen.insert(island.name.as_str()) {
                continue;
            }
            findings.push(Finding {
                rule: Rule::Island,
                file: island.file.clone(),
                line: 1,
                message: format!(
                    "island `{}` is in area `{}`'s bundle twice; a page defines each tag once",
                    island.name, bundle.area
                ),
                fix: "rename one of the two files".to_owned(),
            });
        }
    }
    findings
}

pub(super) fn bundled_island<'m>(
    bundles: &[IslandBundle<'m>],
    area: &str,
    name: &str,
) -> Option<&'m Island> {
    bundles
        .iter()
        .filter(|bundle| bundle.area == area)
        .flat_map(|bundle| &bundle.islands)
        .find(|island| island.name == name)
        .copied()
}

fn own_island<'m>(module: &'m Module, area: &str, name: &str) -> Option<&'m Island> {
    module
        .islands
        .iter()
        .find(|island| island.name == name && island.area.as_deref().is_none_or(|own| own == area))
}

pub(super) fn props_finding(island: &Island, line: usize, message: String, fix: String) -> Finding {
    Finding {
        rule: Rule::IslandProps,
        file: island.file.clone(),
        line,
        message,
        fix,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use sofie_model::testing::{area, field, function, module, route, type_name};
    use sofie_model::{Island, Method, Module, PropsDeclaration, ReadModel};

    use super::{Rule, check_islands, island_names};
    use crate::Screens;

    fn board(names: &[&str], written: Option<&str>, has_file: bool) -> Module {
        let mut board = module("board", "/demo/modules/board");
        let mut island_route = route(Method::Get, "/", "queries::latest_note", 5);
        island_route.island = Some("note-editor".to_owned());
        let web = area(&board, "web", vec![island_route]);
        board.areas.push(web);
        let mut loader = function(
            "queries::latest_note",
            "/demo/modules/board/src/queries.rs",
            9,
        );
        loader.output = Some(type_name("Result<NoteDraft>"));
        board.code.functions.push(loader);
        board.code.read_models.push(ReadModel {
            name: "NoteDraft".to_owned(),
            from: Some("Note".to_owned()),
            fields: vec![field("title", "String", 3), field("body", "String", 4)],
            file: PathBuf::from("/demo/modules/board/src/queries.rs"),
            line: 2,
        });
        if has_file {
            board.islands.push(Island {
                name: "note-editor".to_owned(),
                file: PathBuf::from("/demo/modules/board/src/areas/web/islands/note-editor.svelte"),
                area: Some("web".to_owned()),
                props: Some(PropsDeclaration {
                    names: names.iter().map(|name| (*name).to_owned()).collect(),
                    type_name: written.map(str::to_owned),
                    line: 4,
                }),
            });
        }
        board
    }

    fn found(modules: &[Module]) -> Vec<(Rule, String)> {
        check_islands(modules, &Screens::read(modules))
            .into_iter()
            .map(|finding| (finding.rule, finding.message))
            .collect()
    }

    #[test]
    fn props_declared_as_the_loaders_read_model_pass() {
        assert_eq!(
            found(&[board(&["title", "body"], Some("NoteDraft"), true)]),
            []
        );
    }

    #[test]
    fn props_of_another_type_or_field_are_found() {
        let findings = found(&[board(&["title", "text"], Some("Note"), true)]);

        assert_eq!(findings.len(), 2, "{findings:?}");
        assert!(
            findings[0].1.contains("declares its props as `Note`"),
            "{findings:?}"
        );
        assert!(findings[1].1.contains("reads prop `text`"), "{findings:?}");
        assert!(findings.iter().all(|(rule, _)| *rule == Rule::IslandProps));
    }

    #[test]
    fn a_routed_island_needs_its_file() {
        let findings = found(&[board(&[], None, false)]);

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].0, Rule::Island);
        assert!(
            findings[0]
                .1
                .contains("`src/areas/web/islands/note-editor.svelte`")
        );
    }

    #[test]
    fn a_tag_is_in_one_areas_bundle_once() {
        let mut first = board(&["title"], Some("NoteDraft"), true);
        let mut shared = first.islands[0].clone();
        shared.area = None;
        shared.file = PathBuf::from("/demo/modules/board/web/note-editor.svelte");
        first.islands.push(shared);

        let findings = found(&[first]);

        assert!(
            findings.iter().any(|(rule, message)| *rule == Rule::Island
                && message.contains("in area `web`'s bundle twice")),
            "{findings:?}"
        );
    }

    #[test]
    fn finds_each_island_a_template_names() {
        let source = "<Section title=\"x\">\n  <Island id=\"c\" name=\"order-chart\" props={data} />\n\
                      <Island props={data} name=\"map-view\"/></Section>";

        assert_eq!(
            island_names(source),
            [("order-chart".to_owned(), 2), ("map-view".to_owned(), 3)]
        );
    }
}
