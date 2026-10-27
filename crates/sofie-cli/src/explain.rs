use std::path::{Path, PathBuf};

use sofie_check::{Screen, Screens};
use sofie_engine::{Binding, Step};

use sofie_model::naming::{RouteSite, file_stem, job_name, screen_key, snake_case, task_name};
use sofie_model::{
    Aggregate, App, AreaContribution, Code, Effects, EnumDeclaration, FeatureDeclaration,
    FieldDeclaration, Function, InputDeclaration, Module, ReadModel, ServedRoute, TypeName,
};
use sofie_values::Seed;

use crate::flows::{Flow, endpoint_flow};
use crate::inputs::{Field, Kind, valid_object};

use crate::routes_command::{feature, handler_path, modifiers, owner_scope, shown_path};
use crate::tasks_command::task_arguments;

mod packages;

#[must_use]
pub fn explain(app: &App, name: &str) -> Vec<String> {
    let mut sections = Vec::new();
    let sites = app.route_sites();
    sections.extend(
        sites
            .iter()
            .filter(|site| site.name() == name || handler_path(site) == name)
            .map(|site| route(app, site)),
    );
    sections.extend(identity_route(app, name));
    sections.extend(
        app.package_routes()
            .other_packages()
            .iter()
            .filter(|served| served.name == name)
            .map(|served| package_route_lines(app, served).join("\n")),
    );
    sections.extend(packages::identity_screen(app, name));
    sections.extend(crate::components::component(name, || {
        sofie_model::framework_features(&app.root).ok()
    }));
    sections.extend(area(app, name, &sites));
    if app.app.crate_name == name || app.app.package == name {
        sections.push(app_section(app));
    }
    for module in &app.modules {
        if module.krate.crate_name == name || module.krate.package == name {
            sections.push(format!(
                "{}\n{}",
                module_section(app, module),
                framework_features_line(app)
            ));
        }
        sections.extend(event(module, name));
        sections.extend(job(module, name));
        sections.extend(task(module, name));
        sections.extend(
            module
                .code
                .read_models
                .iter()
                .filter(|model| model.name == name)
                .map(|model| read_model(module, model, &sites)),
        );
        sections.extend(
            module
                .code
                .aggregates
                .iter()
                .filter(|aggregate| aggregate.name == name)
                .map(|aggregate| aggregate_section(module, aggregate)),
        );
        sections.extend(screen(module, name, &sites).map(|summary| {
            let mut lines = vec![summary];
            lines.extend(screen_data(app, module, name, &sites));
            lines.join("\n")
        }));
        sections.extend(flag(module, name, &sites));
    }
    sections
}

fn route(app: &App, site: &RouteSite) -> String {
    let route = site.route;
    let mut lines = vec![
        format!("route {}", site.name()),
        format!("  area: {}", site.area.area),
        format!("  {} {}", route.method.name(), shown_path(app, site)),
        format!("  handler: {}", handler_path(site)),
        format!(
            "  declared: {}:{}",
            site.area.routes_file.display(),
            route.line
        ),
        format!(
            "  permission: {}",
            site.permission(app)
                .unwrap_or_else(|| "none (its area admits without one)".to_owned())
        ),
        format!("  owner scope: {}", owner_scope(app, &site.area.area)),
    ];
    lines.extend(feature(site).map(|flag| format!("  feature: {flag}")));
    let declared = modifiers(route);
    if !declared.is_empty() {
        lines.push(format!("  modifiers: {}", declared.join(" ")));
    }
    if let Some(function) = site.module.code.function(&route.handler) {
        lines.push(format!(
            "  function: {}",
            located(&function.file, function.line)
        ));
        lines.extend(answer(function));
        lines.extend(arguments(function));
        lines.extend(input(&site.module.code, function));
        lines.extend(query(&site.module.code, function));
        lines.extend(flow_lines(&endpoint_flow(app, site)));
    }
    for screen in shown_screens(site) {
        lines.push(screen_line(site, &screen));
    }
    lines.join("\n")
}

/// A resource's screen is built in until the area has a file of its key.
fn screen_line(site: &RouteSite, screen: &str) -> String {
    let file = site.area.screen_file(screen);
    if !site.route.is_resource() || site.area.screens.contains(&file) {
        return format!("  screen: {}", file.display());
    }
    format!(
        "  screen: {screen}, built in; {} replaces it (`sofie scaffold screen {}/{}/{screen}` \
         writes the built-in one there)",
        file.display(),
        site.module.krate.crate_name,
        site.area.area
    )
}

fn flow_lines(flow: &Flow) -> Vec<String> {
    let edges = flow.edges.iter().map(|edge| {
        format!(
            "  flow: {} {} {}",
            edge.from.label(),
            edge.kind.verb(),
            edge.to.label()
        )
    });
    let unresolved = flow.unresolved.iter().map(|unresolved| {
        format!(
            "  flow unresolved: {}: {}",
            located(&unresolved.file, unresolved.line),
            unresolved.reason
        )
    });
    edges.chain(unresolved).collect()
}

fn identity_route(app: &App, name: &str) -> Option<String> {
    let all = match app.package_routes().identity() {
        Ok(all) => all,
        Err(error) => {
            return Some(format!(
                "sofie-identity's routes could not be read: {error}"
            ));
        }
    };
    let served = all.iter().find(|served| served.name == name)?;
    let mut lines = package_route_lines(app, served);
    let identity = app.identity.as_ref()?;
    match identity.code() {
        Ok(code) => lines.extend(identity_function_lines(&code, served, &all)),
        Err(error) => lines.push(format!(
            "  sofie-identity's code could not be read: {error}"
        )),
    }
    Some(lines.join("\n"))
}

fn identity_function_lines(code: &Code, served: &ServedRoute, all: &[ServedRoute]) -> Vec<String> {
    let Some(function) = served.route.function(code) else {
        return Vec::new();
    };
    let mut lines = vec![format!(
        "  function: {}",
        located(&function.file, function.line)
    )];
    lines.extend(answer(function));
    lines.extend(unit_answer(function));
    lines.extend(taken_address(served).map(str::to_owned));
    lines.extend(session_lines(code, served, all, function));
    lines.extend(arguments(function));
    if code.input_of(function).is_some() {
        lines.extend(input(code, function));
    }
    lines
}

/// `()` replies nothing, which a JSON caller gets as 204 No Content.
fn unit_answer(function: &Function) -> Option<String> {
    let mut answered = function.output.as_ref()?;
    while answered.name == "Result" {
        answered = answered.arguments.first()?;
    }
    (answered.name == "()")
        .then(|| "  answers a JSON caller: 204 No Content, without a body".to_owned())
}

/// Sign-up hides which addresses have accounts, so its answer cannot tell a caller it created nothing.
fn taken_address(served: &ServedRoute) -> Option<&'static str> {
    (served.route.handler == "sign_up").then_some(
        "  a taken address: answered exactly like a new one, and nothing is created; sign up \
         with a fresh address",
    )
}

fn session_lines(
    code: &Code,
    served: &ServedRoute,
    all: &[ServedRoute],
    function: &Function,
) -> Vec<String> {
    let signs_in = reaches_session(code, &function.effects, "sign_in");
    let signs_out = reaches_session(code, &function.effects, "sign_out");
    let mut lines = vec![
        if signs_in {
            "  signs in: yes, its handler reaches `Session::sign_in`"
        } else {
            "  signs in: no, its handler never reaches `Session::sign_in`"
        }
        .to_owned(),
    ];
    if signs_out {
        lines.push("  signs out: yes, its handler reaches `Session::sign_out`".to_owned());
    }
    if !signs_in && !signs_out && served.route.method == "POST" {
        let sign_ins: Vec<String> = all
            .iter()
            .filter(|other| other.area == served.area && other.route.method == "POST")
            .filter(|other| {
                other
                    .route
                    .function(code)
                    .is_some_and(|signing| reaches_session(code, &signing.effects, "sign_in"))
            })
            .map(|other| format!("`{}` (POST {})", other.name, other.path))
            .collect();
        if !sign_ins.is_empty() {
            lines.push(format!("  to sign in after it: {}", sign_ins.join(", ")));
        }
    }
    lines
}

fn package_route_lines(app: &App, served: &ServedRoute) -> Vec<String> {
    vec![
        format!("route {}", served.name),
        format!("  area: {}", served.area),
        format!("  {} {}", served.route.method, served.path),
        format!(
            "  handler: `{}`, added by {}::{}",
            served.route.handler,
            served.package(),
            served.route.mount
        ),
        format!(
            "  permission: {}",
            served
                .route
                .permission
                .as_deref()
                .unwrap_or("none (its area admits without one)")
        ),
        format!("  owner scope: {}", owner_scope(app, &served.area)),
    ]
}

/// Every way of signing in or out ends in a `Session` method, so a handler that reaches one
/// through the functions and methods it calls does it.
fn reaches_session(code: &Code, effects: &Effects, method: &str) -> bool {
    let mut seen: Vec<&Effects> = Vec::new();
    let mut pending = vec![effects];
    while let Some(effects) = pending.pop() {
        if seen.iter().any(|done| std::ptr::eq(*done, effects)) {
            continue;
        }
        seen.push(effects);
        let calls_session = effects
            .method_calls
            .iter()
            .any(|call| call.method == method && call.receiver.as_deref() == Some("Session"));
        if calls_session {
            return true;
        }
        pending.extend(callees(code, effects));
    }
    false
}

fn callees<'code>(code: &'code Code, effects: &Effects) -> Vec<&'code Effects> {
    let functions = effects
        .calls
        .iter()
        .filter_map(|call| code.function(&call.path))
        .map(|function| &function.effects);
    let methods = code
        .methods
        .iter()
        .filter(|method| {
            let by_receiver = effects.method_calls.iter().any(|call| {
                call.method == method.name && call.receiver.as_deref() == Some(&method.owner)
            });
            let by_path = effects.calls.iter().any(|call| {
                call.path
                    .ends_with(&[method.owner.clone(), method.name.clone()])
            });
            by_receiver || by_path
        })
        .map(|method| &method.effects);
    functions.chain(methods).collect()
}

fn answer(function: &Function) -> Option<String> {
    let output = function.output.as_ref()?;
    Some(format!("  answers: {output}"))
}

fn arguments(function: &Function) -> Option<String> {
    if function.arguments.is_empty() {
        return None;
    }
    let written: Vec<String> = function.arguments.iter().map(ToString::to_string).collect();
    Some(format!("  arguments: {}", written.join(", ")))
}

fn input(code: &Code, function: &Function) -> Vec<String> {
    let Some(input) = code.input_of(function) else {
        return vec![lone_value_input(function)];
    };
    let mut lines = vec![format!(
        "  input: {} ({}), a JSON object of",
        input.name,
        located(&input.file, input.line)
    )];
    lines.extend(
        input
            .fields
            .iter()
            .map(|field| format!("    {}", input_field(field, &code.enums))),
    );
    lines.push(format!("  a body it takes: {}", example_body(code, input)));
    lines
}

/// From the same fixtures as `sofie fuzz`'s valid bodies, so the example is one the route accepts.
fn example_body(code: &Code, input: &InputDeclaration) -> serde_json::Value {
    let fields: Vec<Field> = input
        .fields
        .iter()
        .map(|field| Field::of(&field.name, &field.ty, &code.enums))
        .collect();
    serde_json::Value::Object(valid_object(&fields, &mut Seed::new(1)))
}

fn query(code: &Code, function: &Function) -> Vec<String> {
    let Some(query) = code.query_of(function) else {
        return Vec::new();
    };
    let mut lines = vec![format!(
        "  query: {} ({}), parameters of the query string",
        query.name,
        located(&query.file, query.line)
    )];
    lines.extend(
        query
            .fields
            .iter()
            .map(|field| format!("    {}", input_field(field, &code.enums))),
    );
    lines
}

/// `#[derive(Value)]` reads one field named after its type; the model does not know which types derive it.
fn lone_value_input(function: &Function) -> String {
    let optional =
        function
            .arguments
            .iter()
            .find_map(|argument| match argument.arguments.as_slice() {
                [inner] if argument.name == "Option" && inner.arguments.is_empty() => Some(inner),
                _ => None,
            });
    match optional {
        Some(value) => format!(
            "  input: no `Input` type; if `{value}` is a `#[derive(Value)]` type, it reads the \
             optional field `{}` and refuses any other field as not accepted",
            snake_case(&value.name)
        ),
        None => "  input: no `Input` type; a `#[derive(Value)]` argument would read the field \
                 named after its type in snake_case"
            .to_owned(),
    }
}

fn input_field(field: &FieldDeclaration, enums: &[EnumDeclaration]) -> String {
    let read = Field::of(&field.name, &field.ty, enums);
    let ty = match field.ty.arguments.as_slice() {
        [inner] if read.optional => inner,
        _ => &field.ty,
    };
    let presence = if read.optional {
        "optional"
    } else {
        "required"
    };
    let shape = json_shape(&read.kind)
        .map(|shape| format!(", as {shape}"))
        .unwrap_or_default();
    format!("{}: {ty}, {presence}{shape}", field.name)
}

/// Only where the Rust type does not already say it.
fn json_shape(kind: &Kind) -> Option<String> {
    match kind {
        Kind::Text | Kind::Password | Kind::Integer | Kind::Boolean => None,
        Kind::Uuid => Some("a UUID string".to_owned()),
        Kind::Enum(names) => Some(
            names
                .iter()
                .map(|name| format!("\"{name}\""))
                .collect::<Vec<_>>()
                .join(" or "),
        ),
        Kind::Value(valued) => Some(valued.samples().0.join(" or ")),
        Kind::List(item) => Some(format!(
            "a list of {}",
            json_shape(item).unwrap_or_else(|| "values".to_owned())
        )),
    }
}

fn shown_screens(site: &RouteSite) -> Vec<String> {
    let mut named: Vec<&String> = site.route.screen.iter().collect();
    if let Some(sofie_model::Then::Screen { screen, .. }) = &site.route.then {
        named.push(screen);
    }
    if let Some(function) = site.module.code.function(&site.route.handler) {
        named.extend(
            function
                .renders
                .iter()
                .filter_map(|render| render.screen.as_ref()),
        );
    }
    crate::sorted::first_seen(named.into_iter().cloned())
}

fn area(app: &App, name: &str, sites: &[RouteSite]) -> Option<String> {
    let known = app.area(name)?;
    let mut lines = vec![format!("area {name}")];
    match known.definition {
        Some(definition) => {
            lines.push(format!("  type: {}", known.type_path));
            lines.push(format!(
                "  defined: {}",
                located(&definition.file, definition.line)
            ));
            lines.push(format!("  principal: {}", definition.principal.join("::")));
        }
        None => lines.push("  defined: by the framework".to_owned()),
    }
    lines.push(format!(
        "  prefix: {}",
        known.prefix().unwrap_or("(not a literal)")
    ));
    lines.push(format!("  owner scope: {}", owner_scope(app, name)));
    lines.extend(
        sites
            .iter()
            .filter(|site| site.area.area == name)
            .map(|site| format!("  route: {}", site.name())),
    );
    Some(lines.join("\n"))
}

pub(crate) fn app_section(app: &App) -> String {
    let mut lines = vec![
        format!("app {}", app.app.package),
        format!("  path: {}", app.app.path.display()),
    ];
    lines.extend(
        crate::sorted::modules(app)
            .into_iter()
            .map(|module| format!("  module: {}", module.krate.package)),
    );
    lines.push(framework_features_line(app));
    lines.join("\n")
}

/// Cargo unifies a package's features across the workspace, so the app and its modules share them.
fn framework_features_line(app: &App) -> String {
    match sofie_model::framework_features(&app.root) {
        Ok(features) if features.is_empty() => "  sofie features: none".to_owned(),
        Ok(features) => format!("  sofie features: {}", features.join(", ")),
        Err(error) => format!("  sofie features could not be read: {error}"),
    }
}

pub(crate) fn module_section(app: &App, module: &Module) -> String {
    let mut lines = vec![
        format!("module {}", module.krate.package),
        format!("  path: {}", module.krate.path.display()),
    ];
    lines.extend(
        module
            .manifest
            .iter()
            .map(|manifest| format!("  tier: {:?}", manifest.tier)),
    );
    let code = &module.code;
    let entries = module
        .areas
        .iter()
        .map(|area| ("area", area.area.clone()))
        .chain(module.events.iter().map(|event| ("event", event.clone())))
        .chain(
            code.aggregates
                .iter()
                .map(|aggregate| ("aggregate", aggregate.name.clone())),
        )
        .chain(
            code.read_models
                .iter()
                .map(|model| ("read model", model.name.clone())),
        )
        .chain(job_functions(module).map(|function| ("job", job_name(module, function))))
        .chain(
            module
                .tasks
                .iter()
                .map(|task| ("task", task_name(module, &task.function))),
        );
    lines.extend(entries.map(|(kind, name)| format!("  {kind}: {name}")));
    let routes = app
        .route_sites()
        .into_iter()
        .filter(|site| std::ptr::eq(site.module, module))
        .count();
    lines.push(format!("  routes: {routes}"));
    lines.join("\n")
}

fn event(module: &Module, name: &str) -> Option<String> {
    let event = module.events.iter().find(|event| *event == name)?;
    let mut lines = vec![
        format!("event {event}"),
        format!("  module: {}", module.krate.package),
    ];
    lines.extend(
        module
            .listeners
            .iter()
            .filter(|listener| listener.event.last() == Some(event))
            .map(|listener| {
                format!(
                    "  heard by: {} at {}",
                    job_name(module, &listener.function),
                    located(&listener.file, listener.line)
                )
            }),
    );
    Some(lines.join("\n"))
}

fn job(module: &Module, name: &str) -> Option<String> {
    if let Some(listener) = module
        .listeners
        .iter()
        .find(|listener| job_name(module, &listener.function) == name)
    {
        return Some(format!(
            "job {name}\n  listens to: {}\n  declared: {}",
            listener.event.join("::"),
            located(&listener.file, listener.line)
        ));
    }
    if let Some(schedule) = module
        .schedules
        .iter()
        .find(|schedule| job_name(module, &schedule.function) == name)
    {
        return Some(format!(
            "job {name}\n  runs: {}\n  declared: {}",
            schedule.cadence,
            located(&schedule.file, schedule.line)
        ));
    }
    let queued = module
        .jobs
        .iter()
        .find(|job| job_name(module, &job.function) == name)?;
    Some(format!(
        "job {name}\n  queued by: `Jobs::queue` or `routes.queue`\n  declared: {}",
        located(&queued.file, queued.line)
    ))
}

fn task(module: &Module, name: &str) -> Option<String> {
    let task = module
        .tasks
        .iter()
        .find(|task| task_name(module, &task.function) == name)?;
    let mut lines = vec![
        format!("task {name}"),
        format!(
            "  function: {}::{}",
            module.krate.crate_name,
            task.function.join("::")
        ),
        format!("  declared: {}", located(&task.file, task.line)),
        format!("  run: <app> task {name} {}", task_arguments(module, task))
            .trim_end()
            .to_owned(),
    ];
    lines.extend(
        module
            .code
            .function(&task.function)
            .and_then(|function| function.doc.as_ref())
            .map(|doc| format!("  description: {doc}")),
    );
    Some(lines.join("\n"))
}

pub(crate) fn read_model(module: &Module, model: &ReadModel, sites: &[RouteSite]) -> String {
    let mut lines = vec![
        format!("read model {}", model.name),
        format!("  module: {}", module.krate.package),
        format!("  declared: {}", located(&model.file, model.line)),
    ];
    lines.extend(model.from.iter().map(|from| format!("  from: {from}")));
    lines.extend(
        model
            .fields
            .iter()
            .map(|field| format!("  field {}: {}", field.name, field.ty)),
    );
    for function in module.code.functions.iter().filter(|function| {
        function
            .output
            .as_ref()
            .is_some_and(|output| mentions(output, &model.name))
    }) {
        lines.push(format!("  answered by: {}", function.path.join("::")));
        lines.extend(
            sites
                .iter()
                .filter(|site| {
                    std::ptr::eq(site.module, module) && site.route.handler == function.path
                })
                .map(|site| format!("  route: {}", site.name())),
        );
    }
    lines.join("\n")
}

pub(crate) fn aggregate_section(module: &Module, aggregate: &Aggregate) -> String {
    let mut lines = vec![
        format!("aggregate {}", aggregate.name),
        format!("  module: {}", module.krate.package),
        format!("  declared: {}", located(&aggregate.file, aggregate.line)),
    ];
    lines.extend(
        aggregate
            .fields
            .iter()
            .map(|field| format!("  field {}: {}", field.name, field.ty)),
    );
    if !aggregate.behaviors.is_empty() {
        lines.push(format!("  behaviors: {}", aggregate.behaviors.join(", ")));
    }
    lines.push(format!("  columns: {}", aggregate.columns().join(", ")));
    lines.extend(
        module
            .code
            .read_models
            .iter()
            .filter(|model| model.from.as_deref() == Some(aggregate.name.as_str()))
            .map(|model| format!("  read model: {}", model.name)),
    );
    lines.join("\n")
}

pub(crate) fn screen(module: &Module, name: &str, sites: &[RouteSite]) -> Option<String> {
    let (area, file) = screen_file(module, name)?;
    let stem = file_stem(file);
    let mut lines = vec![
        format!("screen {name}"),
        format!("  template: {}", file.display()),
    ];
    lines.extend(
        showing_sites(area, &stem, sites).map(|site| format!("  shown by: {}", site.name())),
    );
    Some(lines.join("\n"))
}

pub(crate) fn screen_file<'module>(
    module: &'module Module,
    name: &str,
) -> Option<(&'module AreaContribution, &'module PathBuf)> {
    module.areas.iter().find_map(|area| {
        area.screens.iter().find_map(|file| {
            (screen_key(&module.krate.crate_name, &area.area, &file_stem(file)) == name)
                .then_some((area, file))
        })
    })
}

fn showing_sites<'sites, 'app>(
    area: &'app AreaContribution,
    stem: &'sites str,
    sites: &'sites [RouteSite<'app>],
) -> impl Iterator<Item = &'sites RouteSite<'app>> {
    sites.iter().filter(move |site| {
        std::ptr::eq(site.area, area) && shown_screens(site).iter().any(|shown| shown == stem)
    })
}

/// What a screen's template is given and what it reads, for `explain` alone: the `screens` list stays short.
fn screen_data(app: &App, module: &Module, name: &str, sites: &[RouteSite]) -> Vec<String> {
    let Some((area, file)) = screen_file(module, name) else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    for site in showing_sites(area, &file_stem(file), sites) {
        let Some(function) = module.code.function(&site.route.handler) else {
            continue;
        };
        let Some(output) = function.output.as_ref().map(TypeName::without_result) else {
            continue;
        };
        lines.push(format!(
            "  data from `{}` ({}): {output}",
            function.path.join("::"),
            site.name()
        ));
        let model = std::iter::once(output)
            .chain(&output.arguments)
            .find_map(|candidate| module.code.read_model(&candidate.name));
        lines.extend(model.into_iter().flat_map(|model| {
            model
                .fields
                .iter()
                .map(|field| format!("    {}: {}", field.name, field.ty))
        }));
    }
    let screens = Screens::read(&app.modules);
    match screens.screen(file) {
        Ok(screen) => {
            lines.extend(binding_lines(screen));
            lines.extend(post_lines(app, screen));
            lines.extend(markup_lines(&screen.source, file));
        }
        Err(error) => lines.push(format!("  template unreadable: {error}")),
    }
    lines
}

fn binding_lines(screen: &Screen) -> Option<String> {
    let bindings = match &screen.bindings {
        Ok(bindings) => bindings,
        Err(error) => return Some(format!("  template error: {error}")),
    };
    let paths = crate::sorted::first_seen(bindings.iter().map(binding_path));
    (!paths.is_empty()).then(|| format!("  binds: {}", paths.join(", ")))
}

/// A `<Form>` draws a field for each of its route's input fields, which its markup does not show.
fn post_lines(app: &App, screen: &Screen) -> Vec<String> {
    let mut lines = Vec::new();
    if let Ok(template) = &screen.template {
        for form in template.form_posts() {
            lines.extend(posted_route_lines(
                app,
                &format!("form `{}`", form.id),
                &form.post,
            ));
        }
    }
    if let Ok(actions) = &screen.actions {
        for action in actions {
            let mut label = format!("action on line {}", action.line);
            if !action.fields.is_empty() {
                label = format!("{label} with {}", action.fields.join(", "));
            }
            lines.extend(posted_route_lines(app, &label, &action.post));
        }
    }
    lines
}

fn posted_route_lines(app: &App, label: &str, post: &str) -> Vec<String> {
    let Some(site) = app.posted_route(post) else {
        return vec![package_post_line(app, label, post)];
    };
    let mut lines = vec![format!(
        "  {label} posts route {} ({} {})",
        site.name(),
        site.route.method.name(),
        shown_path(app, &site)
    )];
    let code = &site.module.code;
    if let Some(function) = code.function(&site.route.handler) {
        lines.extend(
            input(code, function)
                .into_iter()
                .map(|line| format!("  {line}")),
        );
    }
    lines
}

fn package_post_line(app: &App, label: &str, post: &str) -> String {
    let routes = app.package_routes();
    let mut served = match routes.identity() {
        Ok(served) => served,
        Err(error) => {
            return format!(
                "  {label} posts {post}; sofie-identity's routes could not be read: {error}"
            );
        }
    };
    served.extend(routes.other_packages());
    match served.iter().find(|route| route.takes_post(post)) {
        Some(route) => format!(
            "  {label} posts package route {} (POST {}); `explain {}` names its input",
            route.name, route.path, route.name
        ),
        None => format!("  {label} posts {post}, which no route serves"),
    }
}

const SHOWN_MARKUP_LINES: usize = 200;

/// Verbatim and unindented, so `edit_screen`'s `find` can be copied from it.
fn markup_lines(source: &str, file: &Path) -> Vec<String> {
    let total = source.lines().count();
    let mut lines = vec![
        format!("  markup ({total} lines; copy `edit_screen`'s `find` from it exactly):"),
        "```html".to_owned(),
    ];
    lines.extend(source.lines().take(SHOWN_MARKUP_LINES).map(str::to_owned));
    lines.push("```".to_owned());
    if total > SHOWN_MARKUP_LINES {
        lines.push(format!(
            "  … {} more lines in {}",
            total - SHOWN_MARKUP_LINES,
            file.display()
        ));
    }
    lines
}

fn binding_path(binding: &Binding) -> String {
    let mut path = "data".to_owned();
    for step in &binding.steps {
        match step {
            Step::Field(field) => {
                path.push('.');
                path.push_str(field);
            }
            Step::Item => path.push_str("[]"),
        }
    }
    path
}

fn flag(module: &Module, name: &str, sites: &[RouteSite]) -> Option<String> {
    let declaration = module
        .code
        .features
        .declarations
        .iter()
        .find(|declaration| declaration.name.as_deref() == Some(name))?;
    let gated =
        |found: Option<&FeatureDeclaration>| found.is_some_and(|found| found == declaration);
    let mut lines = vec![
        format!("flag {name}"),
        format!(
            "  declared: {}",
            located(&declaration.file, declaration.line)
        ),
        format!(
            "  retire after: {} days from {}",
            declaration.retire_after_days,
            declaration.added.as_deref().unwrap_or("an undeclared day")
        ),
    ];
    lines.extend(
        sites
            .iter()
            .filter(|site| {
                std::ptr::eq(site.module, module) && gated(module.route_feature(site.route))
            })
            .map(|site| format!("  hides route: {}", site.name())),
    );
    lines.extend(
        job_functions(module)
            .filter(|function| gated(module.code.features.gate_on(function)))
            .map(|function| format!("  skips job: {}", job_name(module, function))),
    );
    Some(lines.join("\n"))
}

fn job_functions(module: &Module) -> impl Iterator<Item = &Vec<String>> {
    let listeners = module.listeners.iter().map(|listener| &listener.function);
    listeners
        .chain(module.schedules.iter().map(|schedule| &schedule.function))
        .chain(module.jobs.iter().map(|job| &job.function))
}

fn mentions(ty: &TypeName, name: &str) -> bool {
    ty.name == name || ty.arguments.iter().any(|argument| mentions(argument, name))
}

fn located(file: &Path, line: usize) -> String {
    format!("{}:{line}", file.display())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use sofie_model::testing::{
        app, area, define_area, enumeration, field, function, method, module, route, type_name,
    };
    use sofie_model::{
        Call, Code, FeatureDeclaration, FeatureGate, InputDeclaration, ListedRoute, Listener,
        Method, MethodCall, ScreenRender, ServedRoute,
    };

    use super::{explain, identity_function_lines};

    #[test]
    fn a_task_names_its_function_place_and_how_to_run_it() {
        let mut order = module("order", "shop/modules/order");
        order.tasks = vec![sofie_model::Task {
            file: PathBuf::from("shop/modules/order/src/tasks.rs"),
            line: 4,
            function: vec!["tasks".to_owned(), "reindex".to_owned()],
        }];
        let shop = app("shop", vec![order]);

        assert_eq!(
            explain(&shop, "order.reindex"),
            [
                "task order.reindex\n  function: order::tasks::reindex\n  declared: \
              shop/modules/order/src/tasks.rs:4\n  run: <app> task order.reindex"
            ]
        );
        assert!(explain(&shop, "order")[0].contains("\n  task: order.reindex\n"));
    }

    #[test]
    fn a_queued_job_is_listed_explained_and_skipped_by_its_flag() {
        let mut order = module("order", "shop/modules/order");
        order.jobs = vec![sofie_model::Job {
            file: PathBuf::from("shop/modules/order/src/jobs.rs"),
            line: 6,
            function: vec!["jobs".to_owned(), "send_receipt".to_owned()],
        }];
        order.code.features.declarations = vec![FeatureDeclaration {
            constant: vec!["module".to_owned(), "RECEIPTS".to_owned()],
            name: Some("order.receipts".to_owned()),
            added: Some("2026-09-01".to_owned()),
            retire_after_days: 30,
            variants: false,
            file: PathBuf::from("shop/modules/order/src/module.rs"),
            line: 5,
        }];
        order.code.features.gates = vec![FeatureGate {
            function: vec!["jobs".to_owned(), "send_receipt".to_owned()],
            feature: vec!["RECEIPTS".to_owned()],
            file: PathBuf::from("shop/modules/order/src/jobs.rs"),
            line: 5,
        }];
        let shop = app("shop", vec![order]);

        assert!(explain(&shop, "order")[0].contains("\n  job: order.jobs.send_receipt\n"));
        assert_eq!(
            explain(&shop, "order.jobs.send_receipt"),
            [
                "job order.jobs.send_receipt\n  queued by: `Jobs::queue` or `routes.queue`\n  \
              declared: shop/modules/order/src/jobs.rs:6"
            ]
        );
        assert!(
            explain(&shop, "order.receipts")[0].ends_with("\n  skips job: order.jobs.send_receipt")
        );
    }

    #[test]
    fn an_identity_route_states_whether_it_signs_in_and_how_to_after_it() {
        let file = "identity/src/actions.rs";
        let mut sign_up = function("actions::sign_up", file, 3);
        sign_up.output = Some(type_name("Result<Registration>"));
        let mut sign_in = function("actions::sign_in", file, 9);
        sign_in.effects.method_calls = vec![method_call(Some("Door"), "enter")];
        let mut enter = method("Door", "enter", file, 20);
        enter.effects.calls = vec![Call {
            path: vec!["signing_in".to_owned(), "start".to_owned()],
            line: 21,
        }];
        let mut start = function("signing_in::start", file, 30);
        start.effects.method_calls = vec![method_call(Some("Session"), "sign_in")];
        let mut sign_out = function("actions::sign_out", file, 40);
        sign_out.output = Some(type_name("Result<()>"));
        sign_out.effects.method_calls = vec![method_call(Some("Session"), "sign_out")];
        let code = Code {
            functions: vec![sign_up, sign_in, start, sign_out],
            methods: vec![enter],
            ..Code::default()
        };
        let routes = [
            ("sign_up", "/sign-up"),
            ("sign_in", "/sign-in"),
            ("sign_out", "/sign-out"),
        ]
        .map(|(handler, path)| ListedRoute {
            mount: "routes".to_owned(),
            method: "POST".to_owned(),
            path: path.to_owned(),
            handler: handler.to_owned(),
            permission: None,
        });
        let served: Vec<ServedRoute> = routes
            .iter()
            .map(|route| ServedRoute {
                area: "web".to_owned(),
                path: route.path.clone(),
                name: format!("sofie_identity.web.{}", route.handler),
                route,
            })
            .collect();
        let explained =
            |index: usize| identity_function_lines(&code, &served[index], &served).join("\n");

        assert!(
            explained(0).contains(
                "  a taken address: answered exactly like a new one, and nothing is created; \
                 sign up with a fresh address\n  \
                 signs in: no, its handler never reaches `Session::sign_in`\n  \
                 to sign in after it: `sofie_identity.web.sign_in` (POST /sign-in)"
            ),
            "{}",
            explained(0)
        );
        assert!(
            explained(1).contains("  signs in: yes, its handler reaches `Session::sign_in`"),
            "{}",
            explained(1)
        );
        assert!(
            explained(2).contains(
                "  answers a JSON caller: 204 No Content, without a body\n  \
                 signs in: no, its handler never reaches `Session::sign_in`\n  \
                 signs out: yes, its handler reaches `Session::sign_out`"
            ),
            "{}",
            explained(2)
        );
        assert!(
            !explained(1).contains("a taken address"),
            "{}",
            explained(1)
        );
        assert!(
            !explained(2).contains("to sign in after it"),
            "{}",
            explained(2)
        );
    }

    #[test]
    fn a_sofie_identity_screen_names_its_package_template_and_routes() {
        let directory = sofie_model::testing::TemporaryDirectory::new("sofie-explain-identity");
        directory.write(
            "identity/src/lib.rs",
            "pub fn routes<A: MemberArea>(routes: &mut Routes<Web>) {\n    \
             routes.screen(A::SIGN_IN, \"sign_in\", page::<A>).name_for::<A>(\"sign_in_page\");\n}\n",
        );
        directory.write(
            "shop/modules/order/contract/src/area.rs",
            "impl Area for Member {\n    const SIGN_IN: &'static str = \"/sign-in\";\n}\n",
        );
        let mut order = module("order", directory.path("shop/modules/order"));
        define_area(&mut order, "customer");
        let mut shop = app(directory.path("shop"), vec![order]);
        let sign_in_page = ListedRoute {
            mount: "routes".to_owned(),
            method: "GET".to_owned(),
            path: "{sign_in}".to_owned(),
            handler: "sign_in_page".to_owned(),
            permission: None,
        };
        shop.identity = Some(sofie_model::Identity {
            directory: directory.path("identity"),
            screens: vec!["sofie_identity/web/sign_in".to_owned()],
            routes: vec![sign_in_page],
            ..sofie_model::Identity::default()
        });

        let explained = explain(&shop, "sofie_identity/web/sign_in").join("\n");

        assert_eq!(
            explained,
            format!(
                "screen sofie_identity/web/sign_in\n  package: sofie-identity, area web\n  \
                 template: {}\n  \
                 shown by: route sofie_identity.customer.sign_in_page (GET /sign-in)",
                directory
                    .path("identity/src/screens/sign_in.html")
                    .display()
            )
        );
    }

    fn method_call(receiver: Option<&str>, method: &str) -> MethodCall {
        MethodCall {
            receiver: receiver.map(str::to_owned),
            method: method.to_owned(),
            line: 1,
        }
    }

    #[test]
    fn the_app_and_a_module_state_sofie_features() {
        let shop = app("shop", vec![module("order", "shop/modules/order")]);

        let explained = explain(&shop, "shop").join("\n");
        let order = explain(&shop, "order").join("\n");

        assert!(explained.starts_with("app shop\n"), "{explained}");
        assert!(explained.contains("\n  module: order\n"), "{explained}");
        assert!(explained.contains("\n  sofie features"), "{explained}");
        assert!(order.contains("\n  sofie features"), "{order}");
    }

    #[test]
    fn a_route_names_its_permission_and_flag_and_a_flag_what_it_gates() {
        let mut order = module("order", "shop/modules/order");
        let cancel = route(
            Method::Post,
            "/orders/{order}/cancel",
            "actions::cancel_order",
            4,
        );
        order.areas.push(area(&order, "console", vec![cancel]));
        define_area(&mut order, "console");
        order.defined_areas[0].principal = vec!["Permitted".to_owned()];
        order.listeners = vec![Listener {
            file: PathBuf::from("shop/modules/order/src/listeners.rs"),
            line: 3,
            function: vec!["listeners".to_owned(), "audit".to_owned()],
            event: vec!["OrderCancelled".to_owned()],
        }];
        order.code.features.declarations = vec![FeatureDeclaration {
            constant: vec!["module".to_owned(), "SPLIT".to_owned()],
            name: Some("order.split".to_owned()),
            added: Some("2026-09-01".to_owned()),
            retire_after_days: 30,
            variants: false,
            file: PathBuf::from("shop/modules/order/src/module.rs"),
            line: 5,
        }];
        order.code.features.gates = ["actions::cancel_order", "listeners::audit"]
            .map(|function| FeatureGate {
                function: function.split("::").map(str::to_owned).collect(),
                feature: vec!["SPLIT".to_owned()],
                file: PathBuf::from("shop/modules/order/src/actions.rs"),
                line: 2,
            })
            .to_vec();
        let shop = app("shop", vec![order]);

        let route = explain(&shop, "order.console.cancel_order").join("\n");
        let flag = explain(&shop, "order.split").join("\n");

        assert!(
            route.ends_with(
                "  permission: order.order.*.cancel\n  owner scope: any\n  feature: order.split"
            ),
            "{route}"
        );
        assert_eq!(
            flag,
            "flag order.split\n  declared: shop/modules/order/src/module.rs:5\n  \
             retire after: 30 days from 2026-09-01\n  hides route: order.console.cancel_order\n  \
             skips job: order.listeners.audit"
        );
    }

    #[test]
    fn a_resource_screen_is_built_in_until_the_area_has_its_file() {
        let mut exams = module("exams", "app/modules/exams");
        let index = sofie_model::Route {
            screen: Some("question_index".to_owned()),
            handler: ["sofie", "resource", "list_questions"]
                .map(str::to_owned)
                .to_vec(),
            ..route(Method::Get, "/questions", "queries::unused", 3)
        };
        exams.areas.push(area(&exams, "web", vec![index]));
        let file = exams.areas[0].screen_file("question_index");
        let built_in =
            explain(&app("app", vec![exams.clone()]), "exams.web.list_questions").join("\n");
        exams.areas[0].screens.push(file.clone());
        let replaced = explain(&app("app", vec![exams]), "exams.web.list_questions").join("\n");

        assert!(
            built_in.contains(&format!(
                "  screen: question_index, built in; {} replaces it (`sofie scaffold screen \
                 exams/web/question_index`",
                file.display()
            )),
            "{built_in}"
        );
        assert!(
            replaced.ends_with(&format!("  screen: {}", file.display())),
            "{replaced}"
        );
    }

    #[test]
    fn a_route_states_the_fields_its_input_takes_and_their_json_shape() {
        let mut order = module("order", "shop/modules/order");
        let actions = "shop/modules/order/src/actions.rs";
        let routes = vec![
            route(Method::Post, "/orders", "actions::place_order", 3),
            route(
                Method::Post,
                "/orders/{order}/cancel",
                "actions::cancel_order",
                4,
            ),
        ];
        order.areas.push(area(&order, "web", routes));
        let mut place = function("actions::place_order", actions, 12);
        place.arguments = vec![type_name("Principal<Customer>"), type_name("PlaceOrder")];
        let mut cancel = function("actions::cancel_order", actions, 20);
        cancel.arguments = vec![type_name("Order"), type_name("Option<CancelReason>")];
        order.code.functions = vec![place, cancel];
        order.code.enums = vec![enumeration("Speed", &[("Standard", &[]), ("Express", &[])])];
        order.code.inputs = vec![InputDeclaration {
            name: "PlaceOrder".to_owned(),
            path: vec!["actions".to_owned(), "PlaceOrder".to_owned()],
            from: None,
            fields: vec![
                field("total", "Money", 8),
                field("note", "Option<String>", 9),
                field("speed", "Speed", 10),
                field("items", "Vec<Id<Product>>", 11),
                field("at", "LocalDateTime", 12),
            ],
            unconstrained: Vec::new(),
            file: PathBuf::from(actions),
            line: 7,
        }];
        let shop = app("shop", vec![order]);

        let place = explain(&shop, "order.web.place_order").join("\n");
        let cancel = explain(&shop, "order.web.cancel_order").join("\n");

        assert!(
            place.ends_with(
                "  function: shop/modules/order/src/actions.rs:12\n  \
                 arguments: Principal<Customer>, PlaceOrder\n  \
                 input: PlaceOrder (shop/modules/order/src/actions.rs:7), a JSON object of\n    \
                 total: Money, required, as {\"minor\":14990,\"currency\":\"EUR\"} or \
                 {\"amount\":\"12.50\",\"currency\":\"USD\"}\n    \
                 note: String, optional\n    \
                 speed: Speed, required, as \"STANDARD\" or \"EXPRESS\"\n    \
                 items: Vec<Id<Product>>, required, as a list of a UUID string\n    \
                 at: LocalDateTime, required, as \
                 {\"local\":\"2026-10-11T09:30:00\",\"zone\":\"Europe/Istanbul\"} or \
                 {\"local\":\"2026-10-11T09:30\",\"zone\":\"Europe/Istanbul\"}\n  \
                 a body it takes: {\"at\":{\"local\":\"2026-09-07T13:30:00\",\"zone\":\
                 \"Asia/Tokyo\"},\"items\":[\"e099ec6c-d736-4ca5-85e7-bb0f12278575\",\
                 \"491718de-357e-4da8-8b43-5c8e74616796\",\
                 \"6775dc77-0156-4f61-9afc-d44d14cf8bfe\"],\"note\":\"orchard linen\",\
                 \"speed\":\"EXPRESS\",\"total\":{\"currency\":\"USD\",\"minor\":22319}}"
            ),
            "{place}"
        );
        assert!(
            cancel.ends_with(
                "  arguments: Order, Option<CancelReason>\n  \
                 input: no `Input` type; if `CancelReason` is a `#[derive(Value)]` type, it \
                 reads the optional field `cancel_reason` and refuses any other field as not \
                 accepted"
            ),
            "{cancel}"
        );
    }

    #[test]
    fn a_route_lists_its_flow() {
        let mut order = module("order", "shop/modules/order");
        let cancel = route(Method::Post, "/orders/{order}/cancel", "actions::cancel", 4);
        order.areas.push(area(&order, "web", vec![cancel]));
        let file = "shop/modules/order/src/actions.rs";
        let mut handler = function("actions::cancel", file, 3);
        handler.effects.calls = vec![Call {
            path: vec!["actions".to_owned(), "notify".to_owned()],
            line: 4,
        }];
        order.code.functions = vec![handler, function("actions::notify", file, 9)];
        let shop = app("shop", vec![order]);

        let explained = explain(&shop, "order.web.cancel").join("\n");

        assert!(
            explained.contains("\n  flow: order::actions::cancel calls order::actions::notify"),
            "{explained}"
        );
    }

    #[test]
    fn a_screen_shows_its_markup_verbatim_and_bounded() {
        let directory =
            std::env::temp_dir().join(format!("sofie-explain-markup-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let short = directory.join("orders.html");
        let long = directory.join("history.html");
        std::fs::write(
            &short,
            "<Section title=\"Orders\">\n  <p>none</p>\n</Section>\n",
        )
        .unwrap();
        let many = (1..=230)
            .map(|line| format!("<p>{line}</p>"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&long, many).unwrap();
        let mut order = module("order", "shop/modules/order");
        let mut web = area(&order, "web", Vec::new());
        web.screens = vec![short, long.clone()];
        order.areas.push(web);
        let shop = app("shop", vec![order]);

        let orders = explain(&shop, "order/web/orders").join("\n");
        let history = explain(&shop, "order/web/history").join("\n");
        std::fs::remove_dir_all(&directory).unwrap();

        assert!(
            orders.ends_with(
                "  markup (3 lines; copy `edit_screen`'s `find` from it exactly):\n\
                 ```html\n\
                 <Section title=\"Orders\">\n  <p>none</p>\n</Section>\n\
                 ```"
            ),
            "{orders}"
        );
        assert!(history.contains("\n<p>200</p>\n```\n"), "{history}");
        assert!(!history.contains("<p>201</p>"), "{history}");
        assert!(
            history.ends_with(&format!("  … 30 more lines in {}", long.display())),
            "{history}"
        );
    }

    /// The place-order form's markup is a lone Submit button; the money field it draws comes from the posted route's input.
    #[test]
    fn a_screen_names_the_route_each_form_and_action_posts_and_its_body() {
        let directory =
            std::env::temp_dir().join(format!("sofie-explain-posts-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let orders = directory.join("orders.html");
        std::fs::write(
            &orders,
            "<Section title=\"Orders\">\n  \
             <Form id=\"place-order\" post=\"/orders\"><Submit label=\"Place\" /></Form>\n  \
             <Table id=\"orders\" of={data} columns=\"id\" as=\"order\">\n    \
             <Action post=\"/orders/{order.id}/cancel\" label=\"Cancel\" />\n  </Table>\n\
             </Section>\n",
        )
        .unwrap();
        let mut order = module("order", "shop/modules/order");
        let actions = "shop/modules/order/src/actions.rs";
        let mut web = area(
            &order,
            "web",
            vec![
                route(Method::Post, "/orders", "actions::place_order", 3),
                route(
                    Method::Post,
                    "/orders/{order}/cancel",
                    "actions::cancel_order",
                    4,
                ),
            ],
        );
        web.screens = vec![orders];
        order.areas.push(web);
        let mut place = function("actions::place_order", actions, 12);
        place.arguments = vec![type_name("PlaceOrder")];
        let mut cancel = function("actions::cancel_order", actions, 20);
        cancel.arguments = vec![type_name("Order")];
        order.code.functions = vec![place, cancel];
        order.code.inputs = vec![InputDeclaration {
            name: "PlaceOrder".to_owned(),
            path: vec!["actions".to_owned(), "PlaceOrder".to_owned()],
            from: None,
            fields: vec![field("total", "Money", 8)],
            unconstrained: Vec::new(),
            file: PathBuf::from(actions),
            line: 7,
        }];
        let shop = app("shop", vec![order]);

        let explained = explain(&shop, "order/web/orders").join("\n");
        std::fs::remove_dir_all(&directory).unwrap();

        assert!(
            explained.contains(
                "  binds: data, data[].id\n  \
                 form `place-order` posts route order.web.place_order (POST /orders)\n    \
                 input: PlaceOrder (shop/modules/order/src/actions.rs:7), a JSON object of\n      \
                 total: Money, required, as {\"minor\":14990,\"currency\":\"EUR\"} or \
                 {\"amount\":\"12.50\",\"currency\":\"USD\"}\n    \
                 a body it takes: {\"total\":{\"currency\":\"USD\",\"minor\":22319}}\n  \
                 action on line 4 posts route order.web.cancel_order \
                 (POST /orders/{order}/cancel)\n    \
                 input: no `Input` type"
            ),
            "{explained}"
        );
    }

    #[test]
    fn a_route_names_the_parameters_its_query_struct_reads() {
        let mut order = module("order", "shop/modules/order");
        order.areas.push(area(
            &order,
            "web",
            vec![route(Method::Get, "/orders", "pages::orders", 3)],
        ));
        let mut page = function("pages::orders", "shop/modules/order/src/pages.rs", 5);
        page.arguments = vec![type_name("Listing<OrderRow, OrderSearch>")];
        order.code.functions = vec![page];
        order.code.inputs = vec![InputDeclaration {
            name: "OrderSearch".to_owned(),
            path: vec!["pages".to_owned(), "OrderSearch".to_owned()],
            from: None,
            fields: vec![field("text", "String", 2), field("days", "Option<u32>", 3)],
            unconstrained: Vec::new(),
            file: "shop/modules/order/src/pages.rs".into(),
            line: 1,
        }];
        let shop = app("shop", vec![order]);

        let explained = explain(&shop, "order.web.orders").join("\n");

        assert!(
            explained.contains(
                "  query: OrderSearch (shop/modules/order/src/pages.rs:1), parameters of the \
                 query string\n    text: String, required\n    days: u32, optional"
            ),
            "{explained}"
        );
    }

    #[test]
    fn a_screen_rendered_twice_is_named_once_in_first_seen_order() {
        let mut order = module("order", "shop/modules/order");
        let mut orders = route(Method::Get, "/orders", "pages::orders", 3);
        orders.screen = Some("orders".to_owned());
        order.areas.push(area(&order, "web", vec![orders]));
        let mut page = function("pages::orders", "shop/modules/order/src/pages.rs", 5);
        page.renders = ["empty", "orders", "empty"]
            .map(|screen| ScreenRender {
                screen: Some(screen.to_owned()),
                line: 6,
            })
            .to_vec();
        order.code.functions = vec![page];
        let shop = app("shop", vec![order]);

        let explained = explain(&shop, "order.web.orders").join("\n");
        let screens: Vec<&str> = explained
            .lines()
            .filter_map(|line| line.strip_prefix("  screen: "))
            .collect();

        assert_eq!(screens.len(), 2, "{explained}");
        assert!(screens[0].ends_with("orders.html"), "{explained}");
        assert!(screens[1].ends_with("empty.html"), "{explained}");
    }
}
