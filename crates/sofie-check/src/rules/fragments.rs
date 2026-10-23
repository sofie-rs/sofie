use std::io;
use std::path::Path;

use sofie_engine::IdPattern;
use sofie_model::{AreaContribution, Module, Route, Then};

use super::{Finding, Rule};
use crate::screens::{Screen, Screens};

pub(super) fn check_fragments(modules: &[Module], screens: &Screens) -> Vec<Finding> {
    let mut findings: Vec<Finding> = screens
        .refused_components()
        .iter()
        .map(|(file, message)| Finding {
            rule: Rule::ScreenTemplate,
            file: file.clone(),
            line: 1,
            message: format!("{message}; no screen can write it"),
            fix: "name the file `web/<Name>.html` in PascalCase, unlike any catalog component \
                  and any other module's"
                .to_owned(),
        })
        .collect();
    for area in modules.iter().flat_map(|module| &module.areas) {
        let mut parsed = Vec::new();
        for file in &area.screens {
            match element_ids(file, screens.screen(file)) {
                Ok(ids) => parsed.push((file.as_path(), ids)),
                Err(finding) => findings.push(finding),
            }
        }
        findings.extend(
            area.routes
                .iter()
                .filter_map(|route| missing_fragment(area, route, &parsed)),
        );
    }
    findings
}

fn element_ids(file: &Path, screen: &io::Result<Screen>) -> Result<Vec<IdPattern>, Finding> {
    let screen = screen.as_ref().map_err(|error| Finding {
        rule: Rule::ScreenTemplate,
        file: file.to_path_buf(),
        line: 1,
        message: format!("the screen cannot be read ({error}), so it could not be checked"),
        fix: "make the template a readable UTF-8 file".to_owned(),
    })?;
    let template = screen.template.as_ref().map_err(|error| Finding {
        rule: Rule::ScreenTemplate,
        file: file.to_path_buf(),
        line: error.line,
        message: format!(
            "the screen does not parse at column {}: {}; every route rendering it would fail",
            error.column, error.message
        ),
        fix: "fix the template so the screen engine accepts it".to_owned(),
    })?;
    Ok(template.element_ids())
}

fn missing_fragment(
    area: &AreaContribution,
    route: &Route,
    parsed: &[(&Path, Vec<IdPattern>)],
) -> Option<Finding> {
    let Some(Then::Screen { screen, fragment }) = &route.then else {
        return None;
    };
    let template = area.screen_file(screen);
    let (_, ids) = parsed.iter().find(|(file, _)| *file == template)?;
    if ids.iter().any(|id| id.could_be(fragment)) {
        return None;
    }
    Some(Finding {
        rule: Rule::ThenScreenFragment,
        file: area.routes_file.clone(),
        line: route.line,
        message: format!(
            "{} {} refreshes element `{fragment}` of screen `{screen}`, but {} renders no \
             element with that id, so the refresh would fail after the action commits",
            route.method.name(),
            route.path,
            template.display()
        ),
        fix: format!(
            "give an element of {} `id=\"{fragment}\"`, or refresh an element it renders",
            template.display()
        ),
    })
}
