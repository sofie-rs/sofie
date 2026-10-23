use sofie_model::naming::{
    RouteSite, SOURCE_LANGUAGE, file_stem, path_parameters, route_parameters,
};
use sofie_model::permissions::module_permissions;
use sofie_model::{App, KnownArea, Module};

use super::{Finding, Rule};

pub(super) fn check_names(app: &App) -> Vec<Finding> {
    let sites = app.route_sites();
    let mut findings: Vec<Finding> = sites
        .iter()
        .filter_map(|site| unusable_parameter(site))
        .collect();
    findings.extend(sites.iter().filter_map(|site| unread_parameter(app, site)));
    let areas = app.areas();
    findings.extend(areas.iter().filter_map(malformed_prefix));
    findings.extend(shared_prefixes(&areas));
    findings.extend(areas.iter().filter_map(|area| {
        let definition = area.definition?;
        definition.prefix.is_none().then(|| Finding {
            rule: Rule::AreaPrefix,
            file: definition.file.clone(),
            line: definition.line,
            message: format!(
                "area `{}` has no literal `PREFIX`, so `sofie gen` cannot name its routes or build \
                 their paths",
                area.name
            ),
            fix: "write `const PREFIX: &'static str = \"/path\";` with a string literal".to_owned(),
        })
    }));
    findings.extend(
        app.modules
            .iter()
            .filter_map(|module| unexported_gen(app, module)),
    );
    findings
}

fn malformed_prefix(area: &KnownArea) -> Option<Finding> {
    let definition = area.definition?;
    let prefix = definition.prefix.as_deref()?;
    if prefix.is_empty() || (prefix.starts_with('/') && !prefix.ends_with('/')) {
        return None;
    }
    Some(Finding {
        rule: Rule::AreaPrefix,
        file: definition.file.clone(),
        line: definition.line,
        message: format!(
            "area `{}` has the prefix `{prefix}`, which serves its routes under paths with a \
             missing or doubled `/`",
            area.name
        ),
        fix: "start the prefix with `/` and end it without one, such as `/docs`".to_owned(),
    })
}

/// A pair under one name is `area-definition`'s finding already.
fn shared_prefixes<'app>(areas: &'app [KnownArea<'app>]) -> impl Iterator<Item = Finding> + 'app {
    areas.iter().enumerate().filter_map(|(index, area)| {
        let definition = area.definition?;
        let prefix = area.prefix()?;
        let earlier = areas[..index]
            .iter()
            .find(|earlier| earlier.name != area.name && earlier.prefix() == Some(prefix))?;
        let place = if prefix.is_empty() {
            "the root".to_owned()
        } else {
            format!("`{prefix}`")
        };
        Some(Finding {
            rule: Rule::AreaPrefix,
            file: definition.file.clone(),
            line: definition.line,
            message: format!(
                "area `{}` serves under {place}, as area `{}` does, so their routes would collide",
                area.name, earlier.name
            ),
            fix: "give each area a prefix of its own".to_owned(),
        })
    })
}

fn unusable_parameter(site: &RouteSite) -> Option<Finding> {
    let parameter = path_parameters(&site.route.path)
        .into_iter()
        .find(|name| !is_identifier(name))?;
    Some(Finding {
        rule: Rule::RouteName,
        file: site.area.routes_file.clone(),
        line: site.route.line,
        message: format!(
            "{} has parameter `{{{parameter}}}`, which the route builder cannot take as an \
             argument",
            site.route.path
        ),
        fix: "name each parameter with lowercase letters, digits and `_`, such as `{order}`"
            .to_owned(),
    })
}

fn unread_parameter(app: &App, site: &RouteSite) -> Option<Finding> {
    let (parameter, _) = route_parameters(app, site.module, &site.route.path)
        .into_iter()
        .find(|(name, read)| read.is_none() && is_identifier(name))?;
    Some(Finding {
        rule: Rule::RouteName,
        file: site.area.routes_file.clone(),
        line: site.route.line,
        message: format!(
            "{} has parameter `{{{parameter}}}`, which names no aggregate of module `{}` and no \
             field of one, so no handler can read it and its route builder cannot type it",
            site.route.path, site.module.krate.package
        ),
        fix: "name the parameter after the aggregate the route loads, such as `{order}`, or \
              after the field a child or listing is kept within"
            .to_owned(),
    })
}

fn is_identifier(name: &str) -> bool {
    name.starts_with(|first: char| first.is_ascii_lowercase() || first == '_')
        && name.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
}

fn unexported_gen(app: &App, module: &Module) -> Option<Finding> {
    let contract = module.contract.as_ref()?;
    let missing = missing_gen_exports(app, module);
    let exported = match missing.as_slice() {
        [] => return None,
        [one] => one.clone(),
        _ => format!("{{{}}}", missing.join(", ")),
    };
    Some(Finding {
        rule: Rule::UndeclaredGen,
        file: contract.path.join("src/lib.rs"),
        line: 1,
        message: format!(
            "`sofie gen` writes the {} of module `{}` into src/gen/mod.rs, which the contract does \
             not export",
            missing.join(" and "),
            module.krate.package
        ),
        fix: format!(
            "add `mod r#gen;` and `pub use r#gen::{exported};` to the contract's src/lib.rs"
        ),
    })
}

#[must_use]
pub fn missing_gen_exports(app: &App, module: &Module) -> Vec<String> {
    let has_routes = module.areas.iter().any(|area| !area.routes.is_empty());
    let has_permissions = app.has_permitted_areas() && !module_permissions(module).is_empty();
    let has_keys = module
        .languages
        .iter()
        .any(|file| file_stem(file) == SOURCE_LANGUAGE);
    [
        ("routes", has_routes),
        ("permissions", has_permissions),
        ("keys", has_keys),
        ("pruned", !module.code.aggregates.is_empty()),
    ]
    .into_iter()
    .filter(|(_, generated)| *generated)
    .map(|(name, _)| name.to_owned())
    .chain(
        module
            .code
            .aggregates
            .iter()
            .filter(|aggregate| !aggregate.exposed.is_empty())
            .flat_map(|aggregate| [aggregate.name.clone(), format!("{}Changed", aggregate.name)]),
    )
    .filter(|name| {
        !(module.contract_declares_gen
            && module.contract_exports.iter().any(|export| export == name))
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use sofie_model::{AreaClient, AreaDefinition, KnownArea};

    use super::malformed_prefix;

    #[test]
    fn a_malformed_prefix_is_not_sent_to_the_root_the_web_area_holds() {
        let definition = AreaDefinition {
            name: "Docs".to_owned(),
            type_path: vec!["Docs".to_owned()],
            prefix: Some("docs".to_owned()),
            client: AreaClient::Html,
            principal: Vec::new(),
            implements_member_area: false,
            opens_sign_up: false,
            file: PathBuf::from("src/areas/docs.rs"),
            line: 1,
        };
        let area = KnownArea {
            name: "docs",
            type_path: "crate::Docs".to_owned(),
            contract: None,
            definition: Some(&definition),
        };

        let finding = malformed_prefix(&area).unwrap();

        assert!(finding.fix.contains("`/docs`"), "{}", finding.fix);
        assert!(!finding.fix.contains("root"), "{}", finding.fix);
    }
}
