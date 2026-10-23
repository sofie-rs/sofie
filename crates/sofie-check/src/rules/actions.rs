use std::path::Path;

use sofie_engine::ActionPost;
use sofie_model::{App, InputDeclaration};

use super::{Finding, Rule};
use crate::screens::{Screen, Screens};

/// An `<Action>` posts only its `with` fields, so they must be what the posted route's input reads.
pub(super) fn check_actions(app: &App, screens: &Screens) -> Vec<Finding> {
    let files = app
        .modules
        .iter()
        .flat_map(|module| &module.areas)
        .flat_map(|area| {
            let overrides = area.overrides.iter().map(|replacement| &replacement.file);
            area.screens.iter().chain(overrides)
        });
    let mut findings = Vec::new();
    for file in files {
        let Ok(Screen {
            actions: Ok(actions),
            ..
        }) = screens.screen(file)
        else {
            continue;
        };
        for action in actions {
            if let Some(input) = posted_input(app, &action.post) {
                findings.extend(check_action(file, action, input));
            }
        }
    }
    findings
}

fn posted_input<'app>(app: &'app App, post: &str) -> Option<&'app InputDeclaration> {
    let site = app.posted_route(post)?;
    let code = &site.module.code;
    code.input_of(code.function(&site.route.handler)?)
}

fn check_action(file: &Path, action: &ActionPost, input: &InputDeclaration) -> Vec<Finding> {
    let declares = |name: &str| input.fields.iter().any(|field| field.name == name);
    let unknown = action
        .fields
        .iter()
        .filter(|name| !declares(name))
        .map(|name| unknown_field(file, action, input, name));
    let missing = input
        .fields
        .iter()
        .filter(|field| field.ty.name != "Option" && !action.fields.contains(&field.name))
        .map(|field| missing_field(file, action, input, &field.name));
    unknown.chain(missing).collect()
}

fn unknown_field(
    file: &Path,
    action: &ActionPost,
    input: &InputDeclaration,
    name: &str,
) -> Finding {
    let fields: Vec<&str> = input
        .fields
        .iter()
        .map(|field| field.name.as_str())
        .collect();
    Finding {
        rule: Rule::ActionInput,
        file: file.to_path_buf(),
        line: action.line,
        message: format!(
            "the `Action` posting to `{}` sends `{name}`, but its input `{}` has no field \
             `{name}`, so the route would refuse every click",
            action.post, input.name
        ),
        fix: format!(
            "post only fields `{}` declares: {}",
            input.name,
            fields.join(", ")
        ),
    }
}

fn missing_field(
    file: &Path,
    action: &ActionPost,
    input: &InputDeclaration,
    name: &str,
) -> Finding {
    Finding {
        rule: Rule::ActionInput,
        file: file.to_path_buf(),
        line: action.line,
        message: format!(
            "the `Action` posting to `{}` does not send `{name}`, which its input `{}` \
             requires, so the route would refuse every click",
            action.post, input.name
        ),
        fix: format!(
            "add `{name}` to the action's `with={{{{…}}}}`, or make the field an `Option`"
        ),
    }
}
