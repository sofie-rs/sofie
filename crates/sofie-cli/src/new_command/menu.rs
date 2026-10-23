use std::path::PathBuf;

use sofie_model::naming::path_parameters;
use sofie_model::{App, AreaContribution, KnownArea, Method, Module, Route};

use super::edits::area_type;
use super::{Edits, checked_name, find_module};
use crate::gen_command::GenError;
use crate::source_edits::{add_call, read};

pub struct NewMenuEntry {
    pub module: String,
    pub area: String,
    pub handler: String,
    pub label: String,
}

pub(super) fn new_menu_entry(
    edits: &mut Edits,
    app: &App,
    entry: &NewMenuEntry,
) -> Result<(), GenError> {
    let module = find_module(app, &entry.module)?;
    let contribution = contribution(module, &entry.area)?;
    let area = app.area(&entry.area).ok_or_else(|| {
        GenError::Usage(format!(
            "the app has no area `{}`; declare it in a contract first",
            entry.area
        ))
    })?;
    let handler = checked_name(&entry.handler)?;
    page(contribution, handler)?;
    let label = entry.label.trim();
    if label.is_empty() {
        return Err(GenError::Usage(
            "a menu entry needs a label, as `Orders`".to_owned(),
        ));
    }
    let contract = module.contract.as_ref().ok_or_else(|| {
        GenError::Usage(format!(
            "module `{}` has no contract, whose generated `routes` a menu entry links to; add \
             `contract/` as `sofie new module` writes it",
            module.krate.crate_name
        ))
    })?;
    let route = format!("routes::{}::{handler}()", contribution.area);
    write_entry(
        edits,
        module,
        contribution,
        &area,
        &route,
        &format!("entry({label:?}, {}::{route})", contract.crate_name),
    )?;
    edits.depend(&module.krate, contract)?;
    edits.enable_framework_feature(&module.krate, "menu")
}

fn contribution<'app>(
    module: &'app Module,
    area: &str,
) -> Result<&'app AreaContribution, GenError> {
    module
        .areas
        .iter()
        .find(|contribution| contribution.area == area)
        .ok_or_else(|| {
            let known: Vec<&str> = module
                .areas
                .iter()
                .map(|contribution| contribution.area.as_str())
                .collect();
            GenError::Usage(format!(
                "module `{}` routes nothing in area `{area}`; its areas: {}",
                module.krate.crate_name,
                known.join(", ")
            ))
        })
}

/// Matches the check the app runs at start, which refuses an entry no page of the area without parameters answers.
fn page(contribution: &AreaContribution, handler: &str) -> Result<(), GenError> {
    let area = &contribution.area;
    let routed: Vec<&Route> = contribution
        .routes
        .iter()
        .filter(|route| route.handler.last().is_some_and(|last| last == handler))
        .collect();
    let Some(page) = routed.iter().find(|route| route.method == Method::Get) else {
        return Err(match routed.first() {
            Some(write) => GenError::Usage(format!(
                "`{handler}` answers {} {} in area `{area}`, which is no page: a menu entry \
                 links to a `get` route",
                write.method.name(),
                write.path
            )),
            None => GenError::Usage(format!(
                "area `{area}` routes no handler `{handler}`; its pages: {}",
                page_handlers(contribution).join(", ")
            )),
        });
    };
    let parameters = path_parameters(&page.path);
    if !parameters.is_empty() {
        return Err(GenError::Usage(format!(
            "a menu entry links to a page of the area without parameters; GET {} takes `{}`",
            page.path,
            parameters.join("`, `")
        )));
    }
    Ok(())
}

fn page_handlers(contribution: &AreaContribution) -> Vec<&str> {
    contribution
        .routes
        .iter()
        .filter(|route| route.method == Method::Get && path_parameters(&route.path).is_empty())
        .filter_map(|route| route.handler.last().map(String::as_str))
        .collect()
}

fn write_entry(
    edits: &mut Edits,
    module: &Module,
    contribution: &AreaContribution,
    area: &KnownArea,
    route: &str,
    call: &str,
) -> Result<(), GenError> {
    let file = menu_file(contribution);
    if file.is_file() {
        if read(&file)?.contains(route) {
            return Err(GenError::Usage(format!(
                "{} has an entry for `{route}` already",
                file.display()
            )));
        }
        return edits.edit_rust(&file, |text| {
            add_call(text, "menu", call).ok_or_else(|| {
                GenError::Usage(format!(
                    "{} has no `pub fn menu(menu: &mut Menu<…>) {{ … }}` to add the entry to",
                    file.display()
                ))
            })
        });
    }
    let (area_import, area_type) = area_type(area);
    edits.create_rust(
        &file,
        &format!(
            "{area_import}use sofie::prelude::*;\n\n\
             pub fn menu(menu: &mut Menu<{area_type}>) {{\n    menu.{call};\n}}\n"
        ),
    )?;
    edits.declare(&module.krate, &["areas", &contribution.area, "menu"])
}

fn menu_file(contribution: &AreaContribution) -> PathBuf {
    contribution.menu.as_ref().map_or_else(
        || contribution.routes_file.with_file_name("menu.rs"),
        |menu| menu.file.clone(),
    )
}
