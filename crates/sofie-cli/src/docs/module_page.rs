use std::fmt::Write as _;

use sofie_model::naming::{RouteSite, file_stem, job_name, screen_key};
use sofie_model::permissions::module_permissions;
use sofie_model::{App, AreaContribution, Module};

use super::data_model::{data_model, state_machines};
use super::events::{heard_by, raised_by};
use super::markdown::{code, codes, table};
use crate::routes_command::{feature, handler_path, modifiers, owner_scope, shown_path};
use crate::sorted;

pub(super) fn module_page(app: &App, module: &Module) -> String {
    let mut text = format!("# Module `{}`\n\n", module.krate.crate_name);
    text.push_str(&summary(module));
    text.push_str(&areas(app, module));
    text.push_str(&data_model(module));
    text.push_str(&state_machines(module));
    text.push_str(&read_models(module));
    text.push_str(&inputs(module));
    text.push_str(&events(app, module));
    text.push_str(&cross_owner_checks(module));
    text.push_str(&jobs_and_schedules(module));
    text.push_str(&flags(module));
    if app.has_permitted_areas() {
        text.push_str(&permissions(module));
    }
    text.push_str(&configuration(module));
    text.push_str("Request flows through this module are drawn in [flows](../flows.md).\n");
    text
}

fn summary(module: &Module) -> String {
    let mut lines = Vec::new();
    if let Some(manifest) = &module.manifest {
        lines.push(format!("- Tier: {}", manifest.tier.name()));
        lines.push(format!(
            "- Context: {}",
            manifest.context.as_deref().map_or("—".to_owned(), code)
        ));
    }
    lines.push(format!(
        "- Contract: {}",
        module
            .contract
            .as_ref()
            .map_or("—".to_owned(), |contract| code(&contract.crate_name))
    ));
    lines.push(format!("- Migrations: {}", module.migrations.len()));
    format!("{}\n\n", lines.join("\n"))
}

fn areas(app: &App, module: &Module) -> String {
    if module.areas.is_empty() {
        return String::new();
    }
    let mut text = String::from("## Areas and routes\n");
    for area in sorted::areas(module) {
        text.push_str(&area_section(app, module, area));
    }
    text.push('\n');
    text
}

fn area_section(app: &App, module: &Module, area: &AreaContribution) -> String {
    let known = app.area(&area.area);
    let prefix = known
        .as_ref()
        .and_then(|known| known.prefix())
        .unwrap_or("?");
    let principal = known
        .as_ref()
        .and_then(|known| known.definition)
        .map_or("Visitor".to_owned(), |definition| {
            definition.principal.join("::")
        });
    let mut text = format!(
        "\n### Area `{}`\n\nPrefix {}, principal {}, reaching {} records.\n\n",
        area.area,
        code(prefix),
        code(&principal),
        owner_scope(app, &area.area)
    );
    let rows: Vec<[String; 7]> = area
        .routes
        .iter()
        .map(|route| {
            route_row(
                app,
                &RouteSite {
                    module,
                    area,
                    route,
                },
            )
        })
        .collect();
    text.push_str(&table(
        [
            "Method",
            "Path",
            "Name",
            "Handler",
            "Description",
            "Permission",
            "Feature, modifiers",
        ],
        &rows,
    ));
    let screens: Vec<String> = area
        .screens
        .iter()
        .map(|file| screen_key(&module.krate.crate_name, &area.area, &file_stem(file)))
        .collect();
    let _ = writeln!(text, "\nScreens: {}", codes(&screens));
    text
}

fn route_row(app: &App, site: &RouteSite) -> [String; 7] {
    let route = site.route;
    let description = site
        .module
        .code
        .function(&route.handler)
        .and_then(|function| function.doc.clone())
        .unwrap_or_else(|| "—".to_owned());
    let mut gates: Vec<String> = feature(site).into_iter().collect();
    gates.extend(modifiers(route));
    [
        route.method.name().to_owned(),
        code(&shown_path(app, site)),
        code(&site.name()),
        code(&handler_path(site)),
        description,
        site.permission(app)
            .map_or("—".to_owned(), |permission| code(&permission)),
        codes(&gates),
    ]
}

fn read_models(module: &Module) -> String {
    let models = &module.code.read_models;
    if models.is_empty() {
        return String::new();
    }
    let rows: Vec<[String; 3]> = models
        .iter()
        .map(|model| {
            let fields: Vec<String> = model
                .fields
                .iter()
                .map(|field| format!("{}: {}", field.name, field.ty))
                .collect();
            [
                code(&model.name),
                model.from.as_deref().map_or("—".to_owned(), code),
                codes(&fields),
            ]
        })
        .collect();
    format!(
        "## Read models\n\n{}\n",
        table(["Read model", "Reads from", "Fields"], &rows)
    )
}

fn inputs(module: &Module) -> String {
    let inputs = &module.code.inputs;
    if inputs.is_empty() {
        return String::new();
    }
    let rows: Vec<[String; 3]> = inputs
        .iter()
        .map(|input| {
            let fields: Vec<String> = input
                .fields
                .iter()
                .map(|field| {
                    let constraints = input
                        .inherited(&field.name, &module.code)
                        .filter(|_| field.constraints.is_empty())
                        .map_or(&field.constraints, |inherited| &inherited.constraints);
                    if constraints.is_empty() {
                        return format!("{}: {}", field.name, field.ty);
                    }
                    format!("{}: {} ({})", field.name, field.ty, constraints.join(", "))
                })
                .collect();
            [
                code(&input.name),
                input.from.as_deref().map_or("—".to_owned(), code),
                codes(&fields),
            ]
        })
        .collect();
    format!(
        "## Inputs\n\n{}\n",
        table(["Input", "Constraints from", "Fields"], &rows)
    )
}

fn events(app: &App, module: &Module) -> String {
    if module.events.is_empty() {
        return String::new();
    }
    let rows: Vec<[String; 3]> = module
        .events
        .iter()
        .map(|event| {
            [
                code(event),
                codes(&raised_by(app, event)),
                codes(&heard_by(app, event)),
            ]
        })
        .collect();
    format!(
        "## Events\n\n{}\n",
        table(["Event", "Raised by", "Heard by"], &rows)
    )
}

fn cross_owner_checks(module: &Module) -> String {
    let checks = module.code.cross_owner_checks();
    if checks.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = checks
        .iter()
        .map(|check| format!("- {} in {}", code(&check.aggregate), code(&check.caller)))
        .collect();
    format!(
        "## Cross-owner checks\n\nWhether records exist across every owner, audited as `cross_owner_check`:\n\n{}\n\n",
        lines.join("\n")
    )
}

fn jobs_and_schedules(module: &Module) -> String {
    let mut rows: Vec<[String; 3]> = module
        .jobs
        .iter()
        .map(|job| {
            [
                code(&job_name(module, &job.function)),
                "queued by handlers".to_owned(),
                feature_of(module, &job.function),
            ]
        })
        .collect();
    rows.extend(module.schedules.iter().map(|schedule| {
        let mut when = schedule.cadence.to_string();
        if let Some(each) = &schedule.each {
            let _ = write!(when, ", once per record of {}", code(&each.join("::")));
        }
        [
            code(&job_name(module, &schedule.function)),
            when,
            feature_of(module, &schedule.function),
        ]
    }));
    if rows.is_empty() {
        return String::new();
    }
    format!(
        "## Jobs and schedules\n\n{}\n",
        table(["Job", "Runs", "Feature"], &rows)
    )
}

fn feature_of(module: &Module, function: &[String]) -> String {
    module
        .code
        .features
        .gate_on(function)
        .and_then(|declaration| declaration.name.as_deref())
        .map_or("—".to_owned(), code)
}

fn flags(module: &Module) -> String {
    let declarations = &module.code.features.declarations;
    if declarations.is_empty() {
        return String::new();
    }
    let rows: Vec<[String; 4]> = declarations
        .iter()
        .map(|declaration| {
            [
                declaration.name.as_deref().map_or("—".to_owned(), code),
                code(&declaration.constant.join("::")),
                if declaration.variants {
                    "variants".to_owned()
                } else {
                    "on or off".to_owned()
                },
                declaration.added.as_deref().map_or_else(
                    || format!("{} days", declaration.retire_after_days),
                    |added| format!("{} days from {added}", declaration.retire_after_days),
                ),
            ]
        })
        .collect();
    format!(
        "## Feature flags\n\n{}\n",
        table(["Flag", "Constant", "Kind", "Retire after"], &rows)
    )
}

fn permissions(module: &Module) -> String {
    let permissions = module_permissions(module);
    if permissions.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = permissions
        .iter()
        .map(|permission| format!("- {}", code(permission)))
        .collect();
    format!("## Permissions\n\n{}\n\n", lines.join("\n"))
}

fn configuration(module: &Module) -> String {
    let settings = module
        .manifest
        .as_ref()
        .map_or(&[][..], |manifest| manifest.settings.as_slice());
    let keys = module
        .config
        .as_ref()
        .map_or(&[][..], |config| config.keys.as_slice());
    if settings.is_empty() && keys.is_empty() {
        return String::new();
    }
    let mut text = String::from("## Configuration\n\n");
    if !keys.is_empty() {
        let rows: Vec<[String; 4]> = keys
            .iter()
            .map(|key| {
                [
                    code(&key.name),
                    key.kind.description().to_owned(),
                    if key.required { "yes" } else { "no" }.to_owned(),
                    key.doc.join(" "),
                ]
            })
            .collect();
        text.push_str(&table(["Key", "Value", "Required", "Description"], &rows));
        text.push('\n');
    }
    if !settings.is_empty() {
        let _ = writeln!(text, "Settings: {}\n", codes(settings));
    }
    text
}
