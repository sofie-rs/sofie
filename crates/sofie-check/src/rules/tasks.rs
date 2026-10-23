use sofie_model::App;
use sofie_model::naming::TASK_RUNNERS;

use super::{Finding, Rule};

pub(super) fn check_task_runner(app: &App) -> Option<Finding> {
    let declaring: Vec<&str> = app
        .modules
        .iter()
        .filter(|module| !module.tasks.is_empty())
        .map(|module| module.krate.crate_name.as_str())
        .collect();
    if declaring.is_empty() || runs_tasks(app) {
        return None;
    }
    let krate = &app.app.crate_name;
    Some(Finding {
        rule: Rule::TaskRunner,
        file: app.app.path.join("src/main.rs"),
        line: 1,
        message: format!(
            "the `#[task]`s of {} need `sofie::run_with_tasks`, which the app never calls, so \
             `<app> task` cannot run them",
            declaring.join(", ")
        ),
        fix: format!(
            "call `sofie::run_with_tasks` (or `sofie::run_with_flags_and_tasks`) in place of \
             `sofie::run`, with `{krate}::tasks` after the worker"
        ),
    })
}

fn runs_tasks(app: &App) -> bool {
    app.app
        .sources
        .iter()
        .flat_map(|source| &source.paths)
        .any(|path| match path.segments.as_slice() {
            [krate, function] => krate == "sofie" && TASK_RUNNERS.contains(&function.as_str()),
            _ => false,
        })
}
