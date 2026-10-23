use sofie_model::{Module, PrivateItem, WiringSide};

use super::{Finding, Rule};

pub(super) fn check_visibility(modules: &[Module]) -> Vec<Finding> {
    modules
        .iter()
        .flat_map(|module| {
            let wired = wired_paths(module);
            module
                .private_items
                .iter()
                .filter_map(move |private| hiding(module, private, &wired))
        })
        .collect()
}

/// What the wirings name in a module, and which of them names it.
fn wired_paths(module: &Module) -> Vec<(Vec<String>, WiringSide)> {
    let by_side = |function: &Vec<String>| (function.clone(), module.job_side(function));
    let on_app = |function: &Vec<String>| (function.clone(), WiringSide::App);
    let listeners = module
        .listeners
        .iter()
        .map(|listener| by_side(&listener.function));
    let schedules = module.schedules.iter().flat_map(|schedule| {
        let side = module.job_side(&schedule.function);
        std::iter::once(&schedule.function)
            .chain(&schedule.each)
            .map(move |function| (function.clone(), side))
    });
    let queued = module.jobs.iter().map(|job| on_app(&job.function));
    let tasks = module.tasks.iter().map(|task| on_app(&task.function));
    let provided = module
        .implementations
        .iter()
        .chain(&module.extensions)
        .map(|provision| on_app(&provision.ty));
    let area_files = module.areas.iter().flat_map(|area| {
        let menu = area.menu.as_ref().map(|_| "menu");
        std::iter::once("routes").chain(menu).map(|file| {
            let path = ["areas", &area.area, file].map(str::to_owned).to_vec();
            (path, WiringSide::Module)
        })
    });
    listeners
        .chain(schedules)
        .chain(queued)
        .chain(tasks)
        .chain(provided)
        .chain(area_files)
        .collect()
}

/// The module's own wiring reaches a private module or function directly in the crate
/// root and a `pub(crate)` one anywhere; the app's reaches nothing private.
fn hides(private: &PrivateItem, path: &[String], side: WiringSide) -> bool {
    path.starts_with(&private.path)
        && (side == WiringSide::App || (private.path.len() > 1 && !private.crate_visible))
}

fn hiding(
    module: &Module,
    private: &PrivateItem,
    wired: &[(Vec<String>, WiringSide)],
) -> Option<Finding> {
    let hidden: Vec<&(Vec<String>, WiringSide)> = wired
        .iter()
        .filter(|(path, side)| hides(private, path, *side))
        .collect();
    if hidden.is_empty() {
        return None;
    }
    let names: Vec<String> = hidden
        .iter()
        .map(|(path, _)| format!("`{}::{}`", module.krate.crate_name, path.join("::")))
        .collect();
    let by_app = hidden.iter().any(|(_, side)| *side == WiringSide::App);
    let (wiring, fix) = if by_app {
        ("the app's wiring", ", and every module above it")
    } else {
        ("the module's src/wiring.rs", "")
    };
    let name = private.path.join("::");
    Some(Finding {
        rule: Rule::Visibility,
        file: private.file.clone(),
        line: private.line,
        message: format!(
            "`{name}` is not `pub`, yet {wiring} names {} through it, so the app would not \
             compile",
            names.join(", ")
        ),
        fix: format!("make `{name}` `pub`{fix}"),
    })
}
