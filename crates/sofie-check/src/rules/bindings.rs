use std::path::{Path, PathBuf};

use sofie_core::listing::{ITEMS, LISTED_FIELDS, PAGE_FIELDS};
use sofie_engine::{Binding, Step};
use sofie_model::{AreaContribution, Code, Function, Module, ReadModel, TypeName, island_bundles};

use super::islands::{PropsGiven, bundled_island, check_props};
use super::{Finding, Rule};
use crate::screens::{Screen, Screens};

/// The read model field `<field>_to` lists the states `<field>` may move to.
const REACHABLE_SUFFIX: &str = "_to";

#[derive(Clone)]
enum Shape<'m> {
    Model(&'m ReadModel),
    List(Box<Shape<'m>>),
    Page(Box<Shape<'m>>),
    Listed(Box<Shape<'m>>),
    Value,
}

struct Rendering<'m> {
    template: PathBuf,
    data: Option<&'m TypeName>,
    supplier: String,
    file: &'m Path,
    line: usize,
}

pub(super) fn check_bindings(modules: &[Module], screens: &Screens) -> Vec<Finding> {
    let mut findings = Vec::new();
    for module in modules {
        for area in &module.areas {
            for route in area.routes.iter().filter(|route| !route.is_resource()) {
                let Some(function) = module.code.function(&route.handler) else {
                    continue;
                };
                if let Some(screen) = &route.screen {
                    let rendering = Rendering {
                        template: area.screen_file(screen),
                        data: function.output.as_ref().map(TypeName::without_result),
                        supplier: format!("loader `{}`", function.path.join("::")),
                        file: &area.routes_file,
                        line: route.line,
                    };
                    findings.extend(check_rendering(
                        modules,
                        &module.code,
                        area,
                        screens,
                        &rendering,
                    ));
                }
                findings.extend(rendered_screens(
                    modules,
                    &module.code,
                    area,
                    screens,
                    function,
                ));
            }
        }
    }
    findings
}

fn rendered_screens(
    modules: &[Module],
    code: &Code,
    area: &AreaContribution,
    screens: &Screens,
    function: &Function,
) -> Vec<Finding> {
    let data = function
        .output
        .as_ref()
        .map(TypeName::without_result)
        .filter(|output| output.name == "Screen")
        .and_then(|screen| screen.arguments.first());
    let mut findings = Vec::new();
    for render in &function.renders {
        let Some(screen) = &render.screen else {
            findings.push(unnamed_render(function, render.line));
            continue;
        };
        let rendering = Rendering {
            template: area.screen_file(screen),
            data,
            supplier: format!("handler `{}`", function.path.join("::")),
            file: &function.file,
            line: render.line,
        };
        if !area.screens.contains(&rendering.template) {
            findings.push(missing_screen(&rendering, screen));
            continue;
        }
        findings.extend(check_rendering(modules, code, area, screens, &rendering));
    }
    findings
}

fn check_rendering(
    modules: &[Module],
    code: &Code,
    area: &AreaContribution,
    screens: &Screens,
    rendering: &Rendering,
) -> Vec<Finding> {
    if !area.screens.contains(&rendering.template) {
        return Vec::new();
    }
    let Ok(Screen {
        bindings: Ok(bindings),
        ..
    }) = screens.screen(&rendering.template)
    else {
        return Vec::new();
    };
    let Some(shape) = rendering.data.and_then(|data| root_shape(code, data)) else {
        return vec![not_a_read_model(rendering, bindings.first())];
    };
    let mut findings = Vec::new();
    for binding in bindings {
        match walk(code, &shape, &binding.steps) {
            Ok(given) => findings.extend(check_island_props(
                modules, &area.area, rendering, binding, &given,
            )),
            Err(problem) => findings.push(wrong_binding(rendering, binding, &problem)),
        }
    }
    findings
}

fn root_shape<'m>(code: &'m Code, data: &TypeName) -> Option<Shape<'m>> {
    let inner = data.arguments.first();
    match (data.name.as_str(), inner) {
        ("Vec", Some(inner)) => Some(Shape::List(Box::new(root_shape(code, inner)?))),
        ("Paginated", Some(inner)) => Some(Shape::Page(Box::new(root_shape(code, inner)?))),
        ("Listed", Some(inner)) => Some(Shape::Listed(Box::new(root_shape(code, inner)?))),
        ("Option", Some(inner)) => root_shape(code, inner),
        (name, _) => code.read_model(name).map(Shape::Model),
    }
}

fn field_shape<'m>(code: &'m Code, ty: &TypeName) -> Shape<'m> {
    match (ty.name.as_str(), ty.arguments.first()) {
        ("Vec", Some(inner)) => Shape::List(Box::new(field_shape(code, inner))),
        ("Option", Some(inner)) => field_shape(code, inner),
        (name, _) => code.read_model(name).map_or(Shape::Value, Shape::Model),
    }
}

fn walk<'m>(code: &'m Code, shape: &Shape<'m>, steps: &[Step]) -> Result<Shape<'m>, String> {
    let Some((step, rest)) = steps.split_first() else {
        return Ok(shape.clone());
    };
    match (shape, step) {
        (Shape::Value, _) => Ok(Shape::Value),
        (Shape::Model(model), Step::Field(field)) => walk_field(code, model, field, rest),
        (Shape::Model(model), Step::Item) => Err(format!(
            "`{}` is one record, not a list to loop over",
            model.name
        )),
        (Shape::List(inner), Step::Item) => walk(code, inner, rest),
        (Shape::List(_), Step::Field(field)) => Err(format!(
            "a list has no field `{field}`; loop over it with `{{#each}}` or a `Table`"
        )),
        (Shape::Page(inner), Step::Field(field)) if field == ITEMS => {
            walk(code, &Shape::List(Box::new(page_item(inner))), rest)
        }
        (Shape::Page(_), Step::Field(field)) if PAGE_FIELDS.contains(&field.as_str()) => {
            Ok(Shape::Value)
        }
        (Shape::Page(_), _) => Err(format!(
            "a page has `{ITEMS}`, {}; loop over its `{ITEMS}`",
            PAGE_FIELDS.join(", ")
        )),
        (Shape::Listed(inner), Step::Item) => walk(code, &page_item(inner), rest),
        (Shape::Listed(inner), Step::Field(field)) if field == ITEMS => {
            walk(code, &Shape::List(Box::new(page_item(inner))), rest)
        }
        (Shape::Listed(_), Step::Field(field)) if LISTED_FIELDS.contains(&field.as_str()) => {
            Ok(Shape::Value)
        }
        (Shape::Listed(_), Step::Field(field)) => Err(format!(
            "a listing has no field `{field}`; it has `{ITEMS}`, {}",
            LISTED_FIELDS.join(", ")
        )),
    }
}

fn page_item<'m>(inner: &Shape<'m>) -> Shape<'m> {
    match inner {
        Shape::Model(model) => Shape::Model(model),
        _ => Shape::Value,
    }
}

fn walk_field<'m>(
    code: &'m Code,
    model: &ReadModel,
    field: &str,
    rest: &[Step],
) -> Result<Shape<'m>, String> {
    if let Some(declared) = model.fields.iter().find(|declared| declared.name == field) {
        return walk(code, &field_shape(code, &declared.ty), rest);
    }
    let reachable = field
        .strip_suffix(REACHABLE_SUFFIX)
        .is_some_and(|base| model.fields.iter().any(|declared| declared.name == base));
    if reachable {
        return Ok(Shape::Value);
    }
    let fields: Vec<&str> = model
        .fields
        .iter()
        .map(|declared| declared.name.as_str())
        .collect();
    Err(format!(
        "read model `{}` has no field `{field}`; its fields are {}",
        model.name,
        fields.join(", ")
    ))
}

/// An embedded island's props are checked like a route island's, against the read model given.
fn check_island_props(
    modules: &[Module],
    area: &str,
    rendering: &Rendering,
    binding: &Binding,
    shape: &Shape,
) -> Vec<Finding> {
    let Some(tag) = &binding.island else {
        return Vec::new();
    };
    let bundles = island_bundles(modules);
    let Some((island, declared)) = bundled_island(&bundles, area, tag)
        .and_then(|island| Some((island, island.props.as_ref()?)))
    else {
        return Vec::new();
    };
    let given = format!(
        "{} gives it `{}` from {}, a",
        rendering.template.display(),
        written(&binding.steps),
        rendering.supplier
    );
    if let Shape::Model(model) = shape {
        return check_props(island, declared, model, &PropsGiven::Screen(given));
    }
    vec![Finding {
        rule: Rule::IslandProps,
        file: rendering.template.clone(),
        line: binding.line,
        message: format!(
            "island `{tag}` reads props, but `{}` is no read model, so its props cannot be \
             checked",
            written(&binding.steps)
        ),
        fix: "give the island one record of a `#[derive(ReadModel)]`; its fields are the props"
            .to_owned(),
    }]
}

fn written(steps: &[Step]) -> String {
    let mut text = "data".to_owned();
    for step in steps {
        match step {
            Step::Field(field) => {
                text.push('.');
                text.push_str(field);
            }
            Step::Item => text.push_str("[…]"),
        }
    }
    text
}

fn wrong_binding(rendering: &Rendering, binding: &Binding, problem: &str) -> Finding {
    Finding {
        rule: Rule::ScreenBinding,
        file: rendering.template.clone(),
        line: binding.line,
        message: format!(
            "`{}` at column {} cannot be filled from the data of {}: {problem}, so it would \
             render empty",
            written(&binding.steps),
            binding.column,
            rendering.supplier
        ),
        fix: "bind a field the read model declares, or add the field to it".to_owned(),
    }
}

fn not_a_read_model(rendering: &Rendering, first: Option<&Binding>) -> Finding {
    let data = rendering
        .data
        .map_or_else(|| "nothing".to_owned(), ToString::to_string);
    let answers = format!(
        "{} answers `{data}`, which is no read model of this module",
        rendering.supplier
    );
    let (file, line, message) = match first {
        Some(first) => (
            rendering.template.clone(),
            first.line,
            format!(
                "the screen binds `{}`, but {answers}, so `sofie gen` cannot check its bindings",
                written(&first.steps)
            ),
        ),
        None => (
            rendering.file.to_path_buf(),
            rendering.line,
            format!(
                "{} is shown, but {answers}, so the screen has nothing to show and \
                 `sofie gen` cannot check what it binds",
                rendering.template.display()
            ),
        ),
    };
    Finding {
        rule: Rule::ScreenBinding,
        file,
        line,
        message,
        fix: "answer with a `#[derive(ReadModel)]` struct, or a `Vec`, `Option`, `Paginated` \
              or `Listed` of one"
            .to_owned(),
    }
}

fn missing_screen(rendering: &Rendering, screen: &str) -> Finding {
    Finding {
        rule: Rule::MissingScreen,
        file: rendering.file.to_path_buf(),
        line: rendering.line,
        message: format!(
            "{} renders screen `{screen}`, but {} does not exist, so the request would fail",
            rendering.supplier,
            rendering.template.display()
        ),
        fix: format!(
            "create {} or name an existing screen",
            rendering.template.display()
        ),
    }
}

fn unnamed_render(function: &Function, line: usize) -> Finding {
    Finding {
        rule: Rule::MissingScreen,
        file: function.file.clone(),
        line,
        message: format!(
            "handler `{}` renders a screen whose name is not a string literal, so `sofie gen` \
             cannot check that it exists or what it binds",
            function.path.join("::")
        ),
        fix: "write the name inline: `Screen::render(\"name\", data)`".to_owned(),
    }
}
