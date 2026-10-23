use sofie_model::naming::{IDENTITY_PACKAGE, crate_name};
use sofie_model::{App, Module, Tier};

use super::events::heard_by;
use super::markdown::{code, codes, mermaid, table};
use crate::sorted;

pub(super) fn architecture(app: &App) -> String {
    let mut text = String::from("# Architecture\n\n## Modules\n\n");
    text.push_str(&modules_table(app));
    text.push_str("\nDependencies flow downward: Experience → Core → Platform.\n\n");
    text.push_str(&mermaid("flowchart TB", &dependency_lines(app)));
    text.push_str("\n## Areas\n\n");
    text.push_str(&areas_table(app));
    text.push_str("\n## Contracts\n\n");
    text.push_str(&contracts_table(app));
    text
}

fn modules_table(app: &App) -> String {
    let rows: Vec<[String; 4]> = sorted::modules(app)
        .into_iter()
        .map(|module| {
            let name = &module.krate.crate_name;
            let manifest = module.manifest.as_ref();
            [
                format!("[{name}](modules/{name}.md)"),
                manifest
                    .map_or("—", |manifest| manifest.tier.name())
                    .to_owned(),
                manifest
                    .and_then(|manifest| manifest.context.as_deref())
                    .map_or("—".to_owned(), code),
                module
                    .contract
                    .as_ref()
                    .map_or("—".to_owned(), |contract| code(&contract.crate_name)),
            ]
        })
        .collect();
    table(["Module", "Tier", "Context", "Contract"], &rows)
}

fn dependency_lines(app: &App) -> Vec<String> {
    let modules = sorted::modules(app);
    let mut lines = Vec::new();
    for tier in [Tier::Experience, Tier::Core, Tier::Platform] {
        let members: Vec<&&Module> = modules
            .iter()
            .filter(|module| module.manifest.as_ref().map(|manifest| manifest.tier) == Some(tier))
            .collect();
        if members.is_empty() {
            continue;
        }
        lines.push(format!("subgraph {}", tier.name()));
        lines.extend(
            members
                .iter()
                .map(|module| format!("    {}", module.krate.crate_name)),
        );
        lines.push("end".to_owned());
    }
    let identity = app.depends_on(IDENTITY_PACKAGE).then(|| {
        let name = crate_name(IDENTITY_PACKAGE);
        lines.extend([
            "subgraph Framework".to_owned(),
            format!("    {name}"),
            "end".to_owned(),
        ]);
        name
    });
    for module in &modules {
        let from = &module.krate.crate_name;
        for other in &modules {
            if !std::ptr::eq(*module, *other) && depends_on(module, other) {
                lines.push(format!("{from} --> {}", other.krate.crate_name));
            }
        }
        if let Some(identity) = &identity
            && module.krate.depends_on(IDENTITY_PACKAGE)
        {
            lines.push(format!("{from} --> {identity}"));
        }
    }
    lines
}

fn depends_on(module: &Module, other: &Module) -> bool {
    let packages: Vec<&str> = std::iter::once(&other.krate)
        .chain(&other.contract)
        .map(|krate| krate.package.as_str())
        .collect();
    module
        .krate
        .dependencies
        .iter()
        .any(|dependency| packages.contains(&dependency.package.as_str()))
}

fn areas_table(app: &App) -> String {
    let rows: Vec<[String; 5]> = app
        .areas()
        .iter()
        .map(|area| {
            let routing: Vec<&str> = sorted::modules(app)
                .into_iter()
                .filter(|module| module.areas.iter().any(|routed| routed.area == area.name))
                .map(|module| module.krate.crate_name.as_str())
                .collect();
            [
                code(area.name),
                code(area.prefix().unwrap_or("?")),
                area.definition
                    .map_or("`Visitor`".to_owned(), |definition| {
                        code(&definition.principal.join("::"))
                    }),
                area.contract.map_or("the framework".to_owned(), |package| {
                    code(&crate_name(package))
                }),
                codes(&routing),
            ]
        })
        .collect();
    table(
        [
            "Area",
            "Prefix",
            "Principal",
            "Defined by",
            "Modules routing in",
        ],
        &rows,
    )
}

fn contracts_table(app: &App) -> String {
    let rows: Vec<[String; 5]> = sorted::modules(app)
        .into_iter()
        .filter_map(|module| {
            let contract = module.contract.as_ref()?;
            let areas: Vec<&str> = module
                .defined_areas
                .iter()
                .map(|area| area.name.as_str())
                .collect();
            let events: Vec<String> = module
                .events
                .iter()
                .map(|event| {
                    let heard = heard_by(app, event).len();
                    format!("{event} ({heard} listening)")
                })
                .collect();
            let provided: Vec<String> = module
                .implementations
                .iter()
                .chain(&module.extensions)
                .map(|provision| provision.contract.join("::"))
                .collect();
            Some([
                code(&contract.crate_name),
                codes(&areas),
                codes(&events),
                codes(&module.contract_exports),
                codes(&provided),
            ])
        })
        .collect();
    table(
        [
            "Contract",
            "Areas",
            "Events",
            "Generated exports",
            "Implemented traits",
        ],
        &rows,
    )
}
