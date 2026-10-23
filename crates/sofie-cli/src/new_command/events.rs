use sofie_model::{App, Crate, Module};

use super::Edits;
use super::edits::{module_file, refuse_function};
use crate::gen_command::GenError;
use crate::source_edits::add_import;

const PRELUDE: (&str, &str) = ("use sofie::prelude", "*");

pub(super) fn new_event(
    edits: &mut Edits,
    app: &App,
    module: &Module,
    name: &str,
) -> Result<(), GenError> {
    let contract = module.contract.as_ref().ok_or_else(|| {
        GenError::Usage(format!(
            "module `{}` has no contract crate (`contract/`), where its events are declared",
            module.krate.crate_name
        ))
    })?;
    if module.events.iter().any(|event| event == name) {
        return Err(GenError::Usage(format!(
            "module `{}` declares the event `{name}` already",
            module.krate.crate_name
        )));
    }
    edits.append(
        &module_file(contract, "events"),
        &[PRELUDE],
        &format!("#[derive(Event)]\npub struct {name} {{}}\n"),
    )?;
    let library = contract.path.join("src").join("lib.rs");
    edits.declare(contract, &["events"])?;
    edits.edit_rust(&library, |text| {
        Ok(add_import(text, "pub use events", name))
    })?;
    edits.depend_on_framework(contract)?;
    edits.allow_unused(&app.root, &contract.package, name, "event")
}

pub(super) fn new_listener(
    edits: &mut Edits,
    app: &App,
    module: &Module,
    event: &str,
    handler: &str,
) -> Result<(), GenError> {
    let contract = event_contract(app, event)?;
    let file = module_file(&module.krate, "listeners");
    refuse_function(&file, handler)?;
    if module
        .listeners
        .iter()
        .any(|listener| listener.function.last().is_some_and(|last| last == handler))
    {
        return Err(GenError::Usage(format!(
            "{} has a function `{handler}` already; choose another name",
            file.display()
        )));
    }
    let import = format!("use {}", contract.crate_name);
    edits.append(
        &file,
        &[PRELUDE, (&import, event)],
        &format!(
            "#[on]\npub async fn {handler}(_event: {event}) -> Result<()> {{\n    Ok(())\n}}\n"
        ),
    )?;
    edits.declare(&module.krate, &["listeners"])?;
    edits.depend(&module.krate, contract)?;
    edits.stop_allowing_unused(&app.root, &contract.package, event)
}

/// Only an event its contract declares may be listened to; one declared in a module crate is a finding already.
fn event_contract<'app>(app: &'app App, event: &str) -> Result<&'app Crate, GenError> {
    let mut declaring = app.modules.iter().filter_map(|module| {
        let contract = module.contract.as_ref()?;
        let in_contract = module.events.iter().any(|name| name == event)
            && module
                .module_events
                .iter()
                .all(|declaration| declaration.name != event);
        in_contract.then_some(contract)
    });
    match (declaring.next(), declaring.next()) {
        (Some(contract), None) => Ok(contract),
        (Some(first), Some(second)) => Err(GenError::Usage(format!(
            "both `{}` and `{}` declare an event `{event}`; rename one",
            first.package, second.package
        ))),
        (None, _) => {
            let known: Vec<&str> = app
                .modules
                .iter()
                .flat_map(|module| &module.events)
                .map(String::as_str)
                .collect();
            Err(GenError::Usage(format!(
                "no contract declares an event `{event}`; the app's events: {}",
                known.join(", ")
            )))
        }
    }
}
