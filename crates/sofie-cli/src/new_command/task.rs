use std::path::PathBuf;

use sofie_model::naming::TASK_RUNNERS;
use sofie_model::{App, Module};

use super::Edits;
use super::edits::module_file;
use crate::gen_command::GenError;
use crate::source_edits::{add_import, read};

pub(super) fn new_task(
    edits: &mut Edits,
    app: &App,
    module: &Module,
    name: &str,
) -> Result<(), GenError> {
    if module
        .tasks
        .iter()
        .any(|task| task.function.last().is_some_and(|last| last == name))
    {
        return Err(GenError::Usage(format!(
            "module `{}` has a task `{name}` already; choose another name",
            module.krate.crate_name
        )));
    }
    run_tasks_from_main(edits, app)?;
    edits.enable_framework_feature(&module.krate, "tasks")?;
    let function = format!("#[task]\npub async fn {name}() -> Result<()> {{\n    Ok(())\n}}\n");
    edits.append(
        &module_file(&module.krate, "tasks"),
        &[("use sofie::prelude", "*")],
        &function,
    )?;
    edits.declare(&module.krate, &["tasks"])
}

/// Once per app: `main` hands the wiring's `tasks` to `sofie::run_with_tasks`.
fn run_tasks_from_main(edits: &mut Edits, app: &App) -> Result<(), GenError> {
    let wiring = tasks_wiring(app)?.map_err(GenError::Usage)?;
    for (file, text) in wiring {
        edits.edit_rust(&file, |_| Ok(text))?;
    }
    Ok(())
}

/// The app's `main.rs` and `lib.rs` handing the wiring's `tasks` to the runner, none when they
/// do already, or why `main.rs` cannot be changed so.
pub(crate) fn tasks_wiring(app: &App) -> Result<Result<Vec<(PathBuf, String)>, String>, GenError> {
    let source = app.app.path.join("src");
    let main = source.join("main.rs");
    let text = read(&main)?;
    if TASK_RUNNERS
        .iter()
        .any(|runner| text.contains(&format!("sofie::{runner}(")))
    {
        return Ok(Ok(Vec::new()));
    }
    let krate = &app.app.crate_name;
    let worker = format!("{krate}::worker");
    let calls = [
        ("sofie::run_with_flags(", "sofie::run_with_flags_and_tasks("),
        ("sofie::run(", "sofie::run_with_tasks("),
    ];
    let call = calls.into_iter().find(|(run, _)| text.contains(run));
    let (Some((run, with_tasks)), true) = (call, text.contains(&worker)) else {
        return Ok(Err(format!(
            "{} does not call `sofie::run` with `{worker}`; call `sofie::run_with_tasks` with \
             `{krate}::tasks` after the worker",
            main.display()
        )));
    };
    let main_text = text.replacen(run, with_tasks, 1).replacen(
        &worker,
        &format!("{worker}, {krate}::tasks"),
        1,
    );
    let library = source.join("lib.rs");
    let library_text = add_import(&read(&library)?, "pub use wiring", "tasks");
    Ok(Ok(vec![(main, main_text), (library, library_text)]))
}
