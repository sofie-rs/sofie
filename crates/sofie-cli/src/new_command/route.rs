use std::path::{Path, PathBuf};

use sofie_model::naming::{
    bound_aggregate, handler_module, pascal_case, path_parameters, snake_case,
};
use sofie_model::{Aggregate, App, KnownArea, Method, Module, check_route_path};

use super::detail_screen::{DetailScreen, back_link, checked_title, screen_source};
use super::edits::{area_type, refuse_function, with_imports};
use super::read_model::{ReadFields, aggregate_module, read_fields};
use super::{Edits, checked_name, checked_type_name, refuse_existing, refuse_prelude_type};
use crate::gen_command::GenError;
use crate::source_edits::{add_call, add_import, read};
use sofie_model::syntax::use_paths;

pub struct NewRoute {
    pub module: String,
    pub area: String,
    pub method: String,
    pub path: String,
    pub handler: String,
    pub screen: Option<String>,
    /// The aggregate's fields a `get`'s read model reads; those a page may show when `None`.
    pub fields: Option<Vec<String>>,
    /// The type a `get` answers; named after the handler when `None`.
    pub read_model: Option<String>,
    /// The page's heading and tab title; humanized from the screen's name when `None`.
    pub title: Option<String>,
    /// A `get` page of the area without parameters to link back to, or `none`; found when `None`.
    pub back: Option<String>,
}

struct Handler<'app> {
    kind: &'static str,
    name: &'app str,
    bound: Option<&'app Aggregate>,
    /// The read model a GET answers.
    answered: Option<String>,
    read: Option<ReadFields>,
}

fn sensitive_values(app: &App) -> Vec<String> {
    let mut values: Vec<String> = app
        .modules
        .iter()
        .flat_map(|module| module.code.telemetry.sensitive_values.iter().cloned())
        .collect();
    values.sort();
    values.dedup();
    values
}

pub(super) fn new_route(edits: &mut Edits, app: &App, route: &NewRoute) -> Result<(), GenError> {
    let module = super::find_module(app, &route.module)?;
    let area = app
        .area(&route.area)
        .ok_or_else(|| unknown_area(app, &route.area))?;
    let method = checked_method(&route.method)?;
    let path = checked_path(&route.path)?;
    refuse_prefixed(&area, path)?;
    let screen = checked_screen(route, method)?;
    let title = route.title.as_deref().map(checked_title).transpose()?;
    refuse_routed(module, &route.area, method, path)?;
    let name = checked_name(&route.handler)?;
    let bound = path_parameters(path)
        .into_iter()
        .find_map(|parameter| bound_aggregate(module, parameter));
    let answers_model = method == Method::Get && (screen.is_some() || bound.is_some());
    let read = match bound {
        Some(aggregate) if method == Method::Get => Some(read_fields(
            &module.krate.path.join("src"),
            aggregate,
            route.fields.as_deref(),
            &sensitive_values(app),
        )?),
        _ => None,
    };
    if route.fields.is_some() && read.is_none() {
        return Err(GenError::Usage(
            "`fields` are read from the aggregate a `get` path names, as `{invoice}`; \
             this route loads none"
                .to_owned(),
        ));
    }
    let handler = Handler {
        kind: handler_module(method),
        name,
        bound,
        answered: answered_model(
            module,
            name,
            route.read_model.as_deref(),
            answers_model,
            bound,
        )?,
        read,
    };
    if let Some(model) = &handler.answered {
        refuse_taken_type(module, &handler, model)?;
    }
    let routes_file = routes_file(module, &route.area);
    let screen_file = screen.map(|screen| {
        routes_file
            .with_file_name("screens")
            .join(format!("{screen}.html"))
    });
    if let Some(file) = &screen_file {
        refuse_existing(file)?;
    }
    let (back, back_note) = match screen {
        Some(_) => back_link(module, &area, path, bound, route.back.as_deref())?,
        None => (None, None),
    };
    let target = write_handler(edits, module, &handler)?;
    let call = match screen {
        Some(screen) => format!("screen(\"{path}\", \"{screen}\", {target})"),
        None => format!(
            "{}(\"{path}\", {target})",
            method.name().to_ascii_lowercase()
        ),
    };
    write_route(edits, module, &area, &routes_file, &handler, &call)?;
    if let (Some(file), Some(screen)) = (&screen_file, screen) {
        let detail = DetailScreen {
            screen,
            title,
            back,
            read: handler.read.as_ref(),
            shows_id: route.fields.iter().flatten().any(|field| field == "id"),
        };
        edits.create(file, &screen_source(&detail))?;
    }
    for left_out in handler.read.iter().flat_map(|read| &read.left_out) {
        edits.note(left_out.to_string());
    }
    if let Some(note) = back_note {
        edits.note(note);
    }
    depend_on_area_contract(edits, app, module, &area)
}

fn checked_screen(route: &NewRoute, method: Method) -> Result<Option<&str>, GenError> {
    let screen = match (&route.screen, method) {
        (None, _) => None,
        (Some(screen), Method::Get) => Some(checked_name(screen)?),
        (Some(_), _) => {
            return Err(GenError::Usage(
                "a screen is shown by a `get` route; route the write without `--screen`".to_owned(),
            ));
        }
    };
    if screen.is_none() && (route.title.is_some() || route.back.is_some()) {
        return Err(GenError::Usage(
            "`title` and `back` shape the page a `get` shows; route it with a `screen`".to_owned(),
        ));
    }
    Ok(screen)
}

fn depend_on_area_contract(
    edits: &mut Edits,
    app: &App,
    module: &Module,
    area: &KnownArea,
) -> Result<(), GenError> {
    let defining = area.contract.and_then(|contract| {
        app.modules
            .iter()
            .filter_map(|candidate| candidate.contract.as_ref())
            .find(|candidate| candidate.package == contract)
    });
    match defining {
        Some(defining) => edits.depend(&module.krate, defining),
        None => Ok(()),
    }
}

fn unknown_area(app: &App, name: &str) -> GenError {
    let known: Vec<&str> = app.areas().iter().map(|area| area.name).collect();
    GenError::Usage(format!(
        "the app has no area `{name}`; its areas: {}",
        known.join(", ")
    ))
}

fn checked_method(method: &str) -> Result<Method, GenError> {
    Method::from_routes_method(&method.to_ascii_lowercase()).ok_or_else(|| {
        GenError::Usage(format!(
            "`{method}` is not a method `sofie new route` takes: get, post, put, patch or delete"
        ))
    })
}

/// What the model would refuse on the next load, refused before anything is written.
fn checked_path(path: &str) -> Result<&str, GenError> {
    let plain = path.starts_with('/')
        && !path
            .chars()
            .any(|character| character.is_whitespace() || matches!(character, '"' | '\\'));
    if !plain {
        return Err(GenError::Usage(format!(
            "`{path}` is not a route path: start it with `/`, without spaces, quotes or backslashes"
        )));
    }
    check_route_path(path).map_err(GenError::Usage)?;
    Ok(path)
}

fn refuse_routed(module: &Module, area: &str, method: Method, path: &str) -> Result<(), GenError> {
    let routed = module
        .areas
        .iter()
        .filter(|contribution| contribution.area == area)
        .flat_map(|contribution| &contribution.routes)
        .find(|existing| existing.method == method && existing.path == path);
    let Some(existing) = routed else {
        return Ok(());
    };
    Err(GenError::Usage(format!(
        "{} {path} is routed already in area `{area}`, to `{}`",
        method.name(),
        existing.handler.join("::")
    )))
}

/// Route paths are relative to the area's prefix, so repeating it would route `/account/account/…`.
fn refuse_prefixed(area: &KnownArea, path: &str) -> Result<(), GenError> {
    let prefix = area.prefix().unwrap_or_default();
    if prefix.is_empty() {
        return Ok(());
    }
    let Some(relative) = path.strip_prefix(prefix) else {
        return Ok(());
    };
    if !relative.is_empty() && !relative.starts_with('/') {
        return Ok(());
    }
    let relative = if relative.is_empty() { "/" } else { relative };
    Err(GenError::Usage(format!(
        "`{path}` repeats area `{}`'s prefix `{prefix}`: route paths are relative to it, so write \
         `{relative}`",
        area.name
    )))
}

/// A GET with a screen or an aggregate answers a read model, so a screen has data `sofie check` can follow.
fn answered_model(
    module: &Module,
    handler: &str,
    named: Option<&str>,
    answers_model: bool,
    read: Option<&Aggregate>,
) -> Result<Option<String>, GenError> {
    if !answers_model {
        if named.is_some() {
            return Err(GenError::Usage(
                "a read model is answered by a `get` with a screen or naming an aggregate, as \
                 `{invoice}`; route it without `read_model`"
                    .to_owned(),
            ));
        }
        return Ok(None);
    }
    let name = if let Some(name) = named {
        checked_type_name(name)?.to_owned()
    } else {
        let name = pascal_case(handler);
        refuse_prelude_type(&name)?;
        name
    };
    let rename = if named.is_some() {
        "another `read_model`"
    } else {
        "another handler name or a `read_model`"
    };
    if read.is_some_and(|aggregate| aggregate.name == name) {
        return Err(GenError::Usage(format!(
            "the read model would be named `{name}`, like the aggregate it reads; choose {rename}"
        )));
    }
    if module.code.read_model(&name).is_some() {
        return Err(GenError::Usage(format!(
            "module `{}` has a read model `{name}` already; choose {rename}",
            module.krate.package
        )));
    }
    Ok(Some(name))
}

/// The read model joins the handler's file, so it may not shadow a type the module declares
/// or a name that file or the read model's own imports bring in.
fn refuse_taken_type(module: &Module, handler: &Handler, model: &str) -> Result<(), GenError> {
    let code = &module.code;
    let declared = code
        .aggregates
        .iter()
        .map(|declared| &declared.name)
        .chain(code.enums.iter().map(|declared| &declared.name))
        .chain(code.inputs.iter().map(|declared| &declared.name));
    if declared.into_iter().any(|name| name == model) {
        return Err(GenError::Usage(format!(
            "`{model}` is a type of module `{}` already; choose another `read_model`",
            module.krate.package
        )));
    }
    let file = module
        .krate
        .path
        .join("src")
        .join(format!("{}.rs", handler.kind));
    let in_file = if file.is_file() {
        file_names(&file)?
    } else {
        Vec::new()
    };
    let imported = handler
        .read
        .iter()
        .flat_map(|read| &read.imports)
        .filter_map(|(_, named)| named.rsplit(' ').next());
    if in_file
        .iter()
        .map(String::as_str)
        .chain(imported)
        .any(|name| name == model)
    {
        return Err(GenError::Usage(format!(
            "`{model}` is a name the handler's file declares or imports already; choose another \
             `read_model`"
        )));
    }
    Ok(())
}

pub(super) fn file_names(file: &Path) -> Result<Vec<String>, GenError> {
    let parsed = syn::parse_file(&read(file)?)
        .map_err(|error| GenError::Usage(format!("{}: {error}", file.display())))?;
    let mut names = Vec::new();
    for item in &parsed.items {
        match item {
            syn::Item::Use(used) => {
                names.extend(
                    use_paths(&used.tree)
                        .into_iter()
                        .filter_map(|used| used.binding),
                );
            }
            syn::Item::Struct(declared) => names.push(declared.ident.to_string()),
            syn::Item::Enum(declared) => names.push(declared.ident.to_string()),
            syn::Item::Type(declared) => names.push(declared.ident.to_string()),
            _ => {}
        }
    }
    Ok(names)
}

fn routes_file(module: &Module, area: &str) -> PathBuf {
    module
        .areas
        .iter()
        .find(|contribution| contribution.area == area)
        .map_or_else(
            || {
                module
                    .krate
                    .path
                    .join("src")
                    .join("areas")
                    .join(area)
                    .join("routes.rs")
            },
            |contribution| contribution.routes_file.clone(),
        )
}

/// A module keeping one file per handler (`src/actions/`) gets one more; otherwise the handler joins `actions.rs`.
fn write_handler(
    edits: &mut Edits,
    module: &Module,
    handler: &Handler,
) -> Result<String, GenError> {
    let source = module.krate.path.join("src");
    let aggregate = handler.bound.map(|bound| {
        let module = aggregate_module(&source, bound).join("::");
        (format!("use {module}"), bound.name.clone())
    });
    let named = aggregate
        .iter()
        .chain(handler.read.iter().flat_map(|read| &read.imports))
        .map(|(path, name)| (path.as_str(), name.as_str()));
    let imports =
        crate::sorted::first_seen(std::iter::once(("use sofie::prelude", "*")).chain(named));
    let stub = stub(handler);
    if source.join(handler.kind).is_dir() {
        let file = source
            .join(handler.kind)
            .join(format!("{}.rs", handler.name));
        edits.create_rust(&file, &format!("{}\n{stub}", with_imports("", &imports)))?;
        edits.declare(&module.krate, &[handler.kind, handler.name])?;
        return Ok(format!(
            "{}::{}::{}",
            handler.kind, handler.name, handler.name
        ));
    }
    let file = source.join(format!("{}.rs", handler.kind));
    refuse_function(&file, handler.name)?;
    edits.append(&file, &imports, &stub)?;
    edits.declare(&module.krate, &[handler.kind])?;
    Ok(format!("{}::{}", handler.kind, handler.name))
}

fn stub(handler: &Handler) -> String {
    let name = handler.name;
    match (&handler.answered, handler.bound) {
        (Some(model), Some(aggregate)) => {
            let (from, binding) = (&aggregate.name, snake_case(&aggregate.name));
            let fields = handler
                .read
                .as_ref()
                .map(ReadFields::declarations)
                .unwrap_or_default();
            format!(
                "#[derive(ReadModel)]\n#[read(from = {from})]\npub struct {model} {{\n{fields}}}\n\n\
                 pub async fn {name}({binding}: {model}) -> Result<{model}> {{\n    Ok({binding})\n}}\n"
            )
        }
        (Some(model), None) => format!(
            "#[derive(ReadModel)]\npub struct {model} {{}}\n\n\
             pub async fn {name}(_read: Read) -> Result<{model}> {{\n    Ok({model} {{}})\n}}\n"
        ),
        (None, Some(aggregate)) => {
            let (aggregate, binding) = (&aggregate.name, snake_case(&aggregate.name));
            format!(
                "pub async fn {name}({binding}: {aggregate}) -> Result<{aggregate}> {{\n    Ok({binding})\n}}\n"
            )
        }
        (None, None) => format!("pub async fn {name}() -> Result<()> {{\n    Ok(())\n}}\n"),
    }
}

fn write_route(
    edits: &mut Edits,
    module: &Module,
    area: &KnownArea,
    routes_file: &Path,
    handler: &Handler,
    call: &str,
) -> Result<(), GenError> {
    if routes_file.is_file() {
        return edits.edit_rust(routes_file, |text| {
            let routed = add_call(text, "routes", call).ok_or_else(|| {
                GenError::Usage(format!(
                    "{} has no `pub fn routes(routes: &mut Routes<…>) {{ … }}` to add the route to",
                    routes_file.display()
                ))
            })?;
            Ok(add_import(&routed, "use crate", handler.kind))
        });
    }
    let (area_import, area_type) = area_type(area);
    edits.create_rust(
        routes_file,
        &format!(
            "use sofie::prelude::*;\n{area_import}\nuse crate::{};\n\n\
             pub fn routes(routes: &mut Routes<{area_type}>) {{\n    routes.{call};\n}}\n",
            handler.kind
        ),
    )?;
    edits.declare(&module.krate, &["areas", area.name, "routes"])
}
