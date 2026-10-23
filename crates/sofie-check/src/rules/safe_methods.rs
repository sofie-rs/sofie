use sofie_model::{Code, Function, Method, Module, Route, TypeName};

use super::{Finding, Rule};

/// Resolved only where the unit of work commits, so the compiler refuses them on a GET too.
const WRITE_SIDE_ARGUMENTS: [&str; 9] = [
    "Db",
    "Dep",
    "Extensions",
    "Flags",
    "IdempotencyKey",
    "Jobs",
    "Session",
    "ShieldBlocks",
    "SignIns",
];
const QUEUED: &str = "Queued";

pub(super) fn check_safe_methods(modules: &[Module]) -> Vec<Finding> {
    modules
        .iter()
        .flat_map(|module| {
            module
                .areas
                .iter()
                .flat_map(|area| &area.routes)
                .filter(|route| route.method == Method::Get && !route.is_resource())
                .filter_map(|route| writing_get(&module.code, route))
        })
        .collect()
}

fn writing_get(code: &Code, route: &Route) -> Option<Finding> {
    let function = code.function(&route.handler)?;
    let (line, write) = write_of(code, function)?;
    Some(Finding {
        rule: Rule::WritingGet,
        file: function.file.clone(),
        line,
        message: format!(
            "GET {} is answered by `{}`, which {write}: a GET, a screen or an island never writes",
            route.path,
            route.handler.join("::")
        ),
        fix: "take a read model or `Read` and return a reply; move the write into a POST, or \
              into a job a domain event triggers"
            .to_owned(),
    })
}

fn write_of(code: &Code, function: &Function) -> Option<(usize, String)> {
    if let Some(argument) = function
        .arguments
        .iter()
        .find(|argument| is_write_side(code, argument))
    {
        return Some((function.line, format!("takes `{}`", argument.name)));
    }
    if let Some(output) = function
        .output
        .as_ref()
        .map(TypeName::without_result)
        .filter(|output| names_writable(code, output))
    {
        return Some((function.line, format!("returns `{}`", output.name)));
    }
    let queued = function.effects.queued.first()?;
    Some((queued.line, "queues a job".to_owned()))
}

fn is_write_side(code: &Code, argument: &TypeName) -> bool {
    WRITE_SIDE_ARGUMENTS.contains(&argument.name.as_str())
        || is_aggregate(code, &argument.name)
        || code.inputs.iter().any(|input| input.name == argument.name)
}

/// `Id<Order>` names an aggregate without being one.
fn names_writable(code: &Code, ty: &TypeName) -> bool {
    ty.name == QUEUED
        || is_aggregate(code, &ty.name)
        || (ty.name != "Id"
            && ty
                .arguments
                .iter()
                .any(|argument| names_writable(code, argument)))
}

fn is_aggregate(code: &Code, name: &str) -> bool {
    code.aggregates
        .iter()
        .any(|aggregate| aggregate.name == name)
}
