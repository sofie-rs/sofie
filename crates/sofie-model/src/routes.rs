use std::path::Path;

use syn::{Expr, ExprMethodCall, Pat, Stmt};

use crate::handlers::HandlerScope;
use crate::naming::{is_plain_name, snake_case};
use crate::syntax::{
    builder_function, last_name, line_of, segments, single_argument, string_literal,
};
use crate::{Method, ModelError, Modifier, Route, Then};

const SIGNATURE: &str = "`pub fn routes(routes: &mut Routes<Area>)`";

pub(crate) fn read_routes(
    file: &Path,
    syntax: &syn::File,
    module_path: Vec<String>,
    deletable: &[String],
) -> Result<Vec<Route>, ModelError> {
    let (function, receiver) = builder_function(file, &syntax.items, "routes", SIGNATURE)?;
    let scope = HandlerScope::new(module_path, &syntax.items);
    let mut routes = Vec::new();
    for statement in &function.block.stmts {
        routes.extend(routes_of(file, &receiver, statement, "", deletable)?);
    }
    for route in &mut routes {
        check_route_path(&route.path)
            .map_err(|message| ModelError::new(file, route.line, message))?;
        if !route.is_resource() {
            route.handler = scope
                .resolve(&route.handler)
                .map_err(|message| ModelError::new(file, route.line, message))?;
        }
    }
    Ok(routes)
}

const RESOURCE_ROOT: &str = "sofie";

impl Route {
    #[must_use]
    pub fn is_resource(&self) -> bool {
        self.handler.first().map(String::as_str) == Some(RESOURCE_ROOT)
    }
}

fn routes_of(
    file: &Path,
    receiver: &str,
    statement: &Stmt,
    prefix: &str,
    deletable: &[String],
) -> Result<Vec<Route>, ModelError> {
    let line = line_of(statement);
    let error = |message: String| ModelError::new(file, line, message);
    let Stmt::Expr(Expr::MethodCall(outermost), _) = statement else {
        return Err(error(statements_message(receiver)));
    };
    let (base, modifiers) =
        call_chain(outermost, receiver).ok_or_else(|| error(statements_message(receiver)))?;
    match base.method.to_string().as_str() {
        "group" if modifiers.is_empty() => group_of(file, base, prefix, deletable),
        "resource" => resource_of(base, &modifiers, prefix, line, deletable).map_err(error),
        _ => {
            let mut route = route_of(base, &modifiers, line).map_err(error)?;
            route.path.insert_str(0, prefix);
            Ok(vec![route])
        }
    }
}

fn group_of(
    file: &Path,
    call: &ExprMethodCall,
    prefix: &str,
    deletable: &[String],
) -> Result<Vec<Route>, ModelError> {
    let line = line_of(call);
    let (Some(path), Some(Expr::Closure(closure)), 2) =
        (call.args.first(), call.args.get(1), call.args.len())
    else {
        return Err(ModelError::new(file, line, GROUP_SHAPE.to_owned()));
    };
    let path = path_of(path).map_err(|message| ModelError::new(file, line, message))?;
    let (Some(Pat::Ident(receiver)), 1, Expr::Block(body)) = (
        closure.inputs.first(),
        closure.inputs.len(),
        closure.body.as_ref(),
    ) else {
        return Err(ModelError::new(file, line, GROUP_SHAPE.to_owned()));
    };
    let receiver = receiver.ident.to_string();
    let prefix = format!("{prefix}{path}");
    let mut routes = Vec::new();
    for statement in &body.block.stmts {
        routes.extend(routes_of(file, &receiver, statement, &prefix, deletable)?);
    }
    Ok(routes)
}

const GROUP_SHAPE: &str = "write the group as `.group(\"/path\", |routes| { … })`: a literal \
     path and a closure of route statements";

const RESOURCE_SCREENS: [&str; 5] = ["Index", "Show", "Create", "Edit", "Delete"];

fn resource_of(
    call: &ExprMethodCall,
    modifiers: &[&ExprMethodCall],
    prefix: &str,
    line: usize,
    deletable: &[String],
) -> Result<Vec<Route>, String> {
    let aggregate = resource_type(call)?;
    let name = snake_case(&aggregate);
    let kept = match modifiers {
        // sofie-http deletes softly only, so by default only a soft-deleting aggregate's records.
        [] => RESOURCE_SCREENS
            .into_iter()
            .filter(|screen| *screen != "Delete" || deletable.contains(&name))
            .map(str::to_owned)
            .collect(),
        [only] if only.method == "only" => kept_screens(only)?,
        _ => return Err("a resource takes only `.only([Index, Show, …])`".to_owned()),
    };
    let plural = sofie_core::plural(&name);
    let at = format!("{prefix}/{}", plural.replace('_', "-"));
    let record = format!("{at}/{{{name}}}");
    let route = |method, path: String, handler: String, screen: Option<&str>| Route {
        screen: screen.map(|screen| format!("{name}_{screen}")),
        ..new_route(
            method,
            path,
            vec![RESOURCE_ROOT.to_owned(), "resource".to_owned(), handler],
            line,
        )
    };
    let routes_of = |screen: &str| match screen {
        "Index" => vec![route(
            Method::Get,
            at.clone(),
            format!("list_{plural}"),
            Some("index"),
        )],
        "Show" => vec![route(
            Method::Get,
            record.clone(),
            format!("show_{name}"),
            Some("show"),
        )],
        "Create" => vec![
            route(
                Method::Get,
                format!("{at}/new"),
                format!("new_{name}"),
                Some("create"),
            ),
            route(
                Method::Post,
                format!("{at}/new"),
                format!("create_{name}"),
                None,
            ),
        ],
        "Edit" => vec![
            route(
                Method::Get,
                format!("{record}/edit"),
                format!("edit_{name}"),
                Some("edit"),
            ),
            route(
                Method::Post,
                format!("{record}/edit"),
                format!("update_{name}"),
                None,
            ),
        ],
        _ => vec![route(
            Method::Post,
            format!("{record}/delete"),
            format!("delete_{name}"),
            None,
        )],
    };
    Ok(kept.iter().flat_map(|screen| routes_of(screen)).collect())
}

fn resource_type(call: &ExprMethodCall) -> Result<String, String> {
    let shape = || "write the resource as `.resource::<Aggregate>()`".to_owned();
    let Some(turbofish) = &call.turbofish else {
        return Err(shape());
    };
    match (
        turbofish.args.first(),
        turbofish.args.len(),
        call.args.is_empty(),
    ) {
        (Some(syn::GenericArgument::Type(syn::Type::Path(path))), 1, true) => {
            last_name(&path.path).ok_or_else(shape)
        }
        _ => Err(shape()),
    }
}

fn kept_screens(only: &ExprMethodCall) -> Result<Vec<String>, String> {
    let shape = || {
        format!(
            "write `.only([…])` with a list of {}",
            RESOURCE_SCREENS.join(", ")
        )
    };
    let (Some(Expr::Array(list)), 1) = (only.args.first(), only.args.len()) else {
        return Err(shape());
    };
    list.elems
        .iter()
        .map(|element| match element {
            Expr::Path(path) => last_name(&path.path)
                .filter(|name| RESOURCE_SCREENS.contains(&name.as_str()))
                .ok_or_else(shape),
            _ => Err(shape()),
        })
        .collect()
}

fn route_of(
    base: &ExprMethodCall,
    modifiers: &[&ExprMethodCall],
    line: usize,
) -> Result<Route, String> {
    let mut route = route_call(base, line)?;
    for (index, call) in modifiers.iter().enumerate().rev() {
        apply_modifier(&mut route, call, index == 0)?;
    }
    if route.webhook.as_deref() == Some("") {
        return Err(
            "a webhook is served only once `.verify(Verifier)` names how its deliveries are \
             verified; end the statement with it"
                .to_owned(),
        );
    }
    Ok(route)
}

fn statements_message(receiver: &str) -> String {
    format!(
        "`sofie gen` reads routes without compiling, so this body may only hold \
         `{receiver}.get|post|put|patch|delete(\"/path\", handler)`, \
         `{receiver}.screen(\"/path\", \"name\", loader)`, \
         `{receiver}.island(\"/path\", \"name\", loader)` and \
         `{receiver}.webhook(\"/path\", handler).verify(Verifier)`, \
         `{receiver}.queue(\"/path\", job)`, \
         `{receiver}.resource::<Aggregate>()` and `{receiver}.group(\"/path\", |routes| {{ … }})` \
         statements, followed by \
         `.throttle(…)`, `.signed()`, `.feature(…)`, `.entitled(…)`, `.idempotent()`, \
         `.body_limit(…)` and an action's `.then_screen(\"name\", \"fragment\")`, \
         `.then_redirect(\"/path\")` or `.then_follow()`; move other \
         code into a handler"
    )
}

/// The call on `receiver` and the modifiers chained on it, outermost first.
fn call_chain<'a>(
    outermost: &'a ExprMethodCall,
    receiver: &str,
) -> Option<(&'a ExprMethodCall, Vec<&'a ExprMethodCall>)> {
    let mut modifiers = Vec::new();
    let mut call = outermost;
    while !is_receiver(&call.receiver, receiver) {
        let Expr::MethodCall(inner) = call.receiver.as_ref() else {
            return None;
        };
        modifiers.push(call);
        call = inner;
    }
    Some((call, modifiers))
}

fn route_call(call: &ExprMethodCall, line: usize) -> Result<Route, String> {
    match call.method.to_string().as_str() {
        "screen" => return screen_of(call, line),
        "island" => return crate::islands::island_of(call, line),
        "webhook" => {
            let (path, handler) = arguments_of(call)?;
            let mut route = new_route(Method::Post, path, handler, line);
            route.webhook = Some(String::new());
            return Ok(route);
        }
        "queue" => {
            let (path, handler) = arguments_of(call)?;
            let mut route = new_route(Method::Post, path, handler, line);
            route.modifiers.push(Modifier::Queued);
            return Ok(route);
        }
        _ => {}
    }
    let method = Method::from_routes_method(&call.method.to_string()).ok_or_else(|| {
        format!(
            "unknown route method `{}`; use get, post, put, patch, delete, screen, queue or webhook",
            call.method
        )
    })?;
    let (path, handler) = arguments_of(call)?;
    Ok(new_route(method, path, handler, line))
}

pub(crate) fn new_route(method: Method, path: String, handler: Vec<String>, line: usize) -> Route {
    Route {
        method,
        path,
        handler,
        screen: None,
        island: None,
        then: None,
        modifiers: Vec::new(),
        webhook: None,
        line,
    }
}

/// What each modifier is allowed on mirrors the builders `Routes` returns,
/// so a chain the reader accepts is one the compiler accepts.
fn apply_modifier(route: &mut Route, call: &ExprMethodCall, outermost: bool) -> Result<(), String> {
    let method = call.method.to_string();
    let webhook = route.webhook.is_some();
    let action = route.method != Method::Get && !webhook;
    let allowed = match method.as_str() {
        "then_screen" | "then_redirect" | "then_follow" | "then_redirect_or_follow" => {
            outermost && !webhook
        }
        "verify" => outermost && webhook,
        "throttle" | "signed" | "feature" | "entitled" => !webhook,
        "live" | "cached" => route.screen.is_some(),
        "idempotent" | "file_limit" => action,
        "document" => route.method == Method::Get && route.screen.is_none() && !webhook,
        "body_limit" => action || webhook,
        _ => false,
    };
    if !allowed {
        return Err(misplaced(&method, route));
    }
    match method.as_str() {
        "then_screen" | "then_redirect" | "then_follow" | "then_redirect_or_follow" => {
            route.then = Some(then_of(call, route.method)?);
        }
        "document" => route
            .modifiers
            .push(Modifier::Document(one_argument(call)?)),
        "file_limit" => route
            .modifiers
            .push(Modifier::FileLimit(one_argument(call)?)),
        "verify" => route.webhook = Some(one_argument(call)?),
        "throttle" => route
            .modifiers
            .push(Modifier::Throttle(one_argument(call)?)),
        "body_limit" => route
            .modifiers
            .push(Modifier::BodyLimit(one_argument(call)?)),
        "signed" => route.modifiers.push(no_argument(call, Modifier::Signed)?),
        "feature" => route.modifiers.push(Modifier::Feature(one_argument(call)?)),
        "entitled" => route
            .modifiers
            .push(Modifier::Entitled(one_argument(call)?)),
        "live" => route.modifiers.push(Modifier::Live(one_argument(call)?)),
        "cached" => route.modifiers.push(Modifier::Cached(one_argument(call)?)),
        _ => route
            .modifiers
            .push(no_argument(call, Modifier::Idempotent)?),
    }
    Ok(())
}

fn misplaced(method: &str, route: &Route) -> String {
    let kind = match (&route.webhook, route.method) {
        (Some(_), _) => "a webhook, which takes `.body_limit(…)` and ends with `.verify(…)`",
        (None, Method::Get) => {
            "a page, which takes `.throttle(…)`, `.signed()`, `.feature(…)` and `.entitled(…)`, \
             as a screen `.live(…)` and `.cached(…)`, and otherwise `.document(…)`"
        }
        (None, _) => {
            "an action, which takes `.throttle(…)`, `.signed()`, `.feature(…)`, `.entitled(…)`, \
             `.idempotent()`, `.body_limit(…)` and `.file_limit(…)`, then optionally \
             `.then_screen(…)`, `.then_redirect(…)`, `.then_redirect_or_follow(…)` or \
             `.then_follow()` last"
        }
    };
    format!("`.{method}(…)` is not a route modifier `sofie gen` accepts here: this route is {kind}")
}

fn one_argument(call: &ExprMethodCall) -> Result<String, String> {
    single_argument(call).ok_or_else(|| format!("`.{}(…)` takes exactly one argument", call.method))
}

fn no_argument<Written>(call: &ExprMethodCall, written: Written) -> Result<Written, String> {
    if call.args.is_empty() {
        return Ok(written);
    }
    Err(format!("`.{}()` takes no argument", call.method))
}

/// Only an action has a change to show, so only an action may refresh a
/// screen or send a browser on.
fn then_of(call: &ExprMethodCall, method: Method) -> Result<Then, String> {
    if method == Method::Get {
        return Err(format!(
            "`.{}(…)` follows an action: `post`, `put`, `patch` or `delete`",
            call.method
        ));
    }
    let literals: Vec<String> = call.args.iter().filter_map(string_literal).collect();
    if call.method == "then_redirect" {
        return then_redirect_of(&literals, call.args.len());
    }
    if call.method == "then_redirect_or_follow" {
        return match then_redirect_of(&literals, call.args.len())? {
            Then::Redirect(path) => Ok(Then::RedirectOrFollow(path)),
            other => Ok(other),
        };
    }
    if call.method == "then_follow" {
        return no_argument(call, Then::Follow);
    }
    match literals.as_slice() {
        [screen, fragment]
            if call.args.len() == 2 && is_plain_name(screen) && is_plain_name(fragment) =>
        {
            Ok(Then::Screen {
                screen: screen.clone(),
                fragment: fragment.clone(),
            })
        }
        _ => Err(
            "write `.then_screen(\"name\", \"fragment\")`: string literals naming a screen of \
             this area and the id of an element in it (letters, digits, `_` and `-`)"
                .to_owned(),
        ),
    }
}

/// A path of this app only, so the redirect cannot send anyone elsewhere; browsers drop tabs
/// and line breaks from a URL, which would turn `/\t/host` into `//host`.
fn then_redirect_of(literals: &[String], arguments: usize) -> Result<Then, String> {
    match literals {
        [path]
            if arguments == 1
                && path.starts_with('/')
                && !path.starts_with("//")
                && !path.contains(|character: char| {
                    character == '\\' || character.is_whitespace() || character.is_control()
                }) =>
        {
            Ok(Then::Redirect(path.clone()))
        }
        _ => Err(
            "write `.then_redirect(\"/path\")`: one string literal, a path of this app \
             starting with a single `/`, without whitespace or `\\`"
                .to_owned(),
        ),
    }
}

fn is_receiver(expression: &Expr, receiver: &str) -> bool {
    matches!(expression, Expr::Path(path) if path.qself.is_none() && path.path.is_ident(receiver))
}

fn arguments_of(call: &ExprMethodCall) -> Result<(String, Vec<String>), String> {
    if call.turbofish.is_some() || call.args.len() != 2 {
        return Err(format!(
            "write the route as `.{}(\"/path\", handler)`: a literal path and a handler, nothing else",
            call.method
        ));
    }
    Ok((path_of(&call.args[0])?, handler_of(&call.args[1])?))
}

fn screen_of(call: &ExprMethodCall, line: usize) -> Result<Route, String> {
    if call.turbofish.is_some() || call.args.len() != 3 {
        return Err(
            "write the screen as `.screen(\"/path\", \"name\", loader)`: a literal path, a \
             literal template name and a loader, nothing else"
                .to_owned(),
        );
    }
    let screen = string_literal(&call.args[1])
        .filter(|name| is_plain_name(name))
        .ok_or_else(|| {
            "the screen name must be a string literal of letters, digits, `_` and `-`, naming \
             `screens/<name>.html` beside this file"
                .to_owned()
        })?;
    let mut route = page_route(call, line)?;
    route.screen = Some(screen);
    Ok(route)
}

pub(crate) fn page_route(call: &ExprMethodCall, line: usize) -> Result<Route, String> {
    Ok(new_route(
        Method::Get,
        path_of(&call.args[0])?,
        handler_of(&call.args[2])?,
        line,
    ))
}

fn path_of(expression: &Expr) -> Result<String, String> {
    let path = string_literal(expression).ok_or_else(|| {
        "the route path must be a string literal, since `sofie gen` reads it without compiling; write it inline"
            .to_owned()
    })?;
    if !path.starts_with('/') {
        return Err(format!(
            "the route path `{path}` must start with `/`, since it is joined to the area's prefix"
        ));
    }
    Ok(path)
}

/// What the router would refuse only at boot, refused before the build.
/// What the router serves and a browser can request; the CLI writes no route this refuses.
pub fn check_route_path(path: &str) -> Result<(), String> {
    if path.contains(char::is_whitespace) {
        return Err(format!("the route path `{path}` holds whitespace"));
    }
    let segments: Vec<&str> = path.split('/').collect();
    for (index, segment) in segments.iter().enumerate() {
        // A browser resolves `.` and `..` before it sends the path, so no request reaches one.
        if matches!(*segment, "." | "..") {
            return Err(format!(
                "the route path `{path}` has the segment `{segment}`, which a browser resolves \
                 away, so no request reaches it"
            ));
        }
        if !segment.contains(['{', '}']) {
            continue;
        }
        let Some(parameter) = parameter(segment) else {
            return Err(format!(
                "the route path `{path}` has a malformed parameter in `{segment}`; write a \
                 parameter as a whole segment, `{{name}}` or `{{*rest}}`"
            ));
        };
        if parameter.starts_with('*') && index + 1 < segments.len() {
            return Err(format!(
                "the route path `{path}` has the catch-all `{segment}` before its end; a \
                 catch-all takes the rest of the path, so it must be the last segment"
            ));
        }
    }
    Ok(())
}

fn parameter(segment: &str) -> Option<&str> {
    let name = segment.strip_prefix('{')?.strip_suffix('}')?;
    let plain = name.strip_prefix('*').unwrap_or(name);
    let well_formed = !plain.is_empty()
        && plain
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_');
    well_formed.then_some(name)
}

fn handler_of(expression: &Expr) -> Result<Vec<String>, String> {
    handler_path(expression).ok_or_else(|| {
        "the handler must be a plain path to a function, such as `actions::place_order`".to_owned()
    })
}

fn handler_path(expression: &Expr) -> Option<Vec<String>> {
    let Expr::Path(path) = expression else {
        return None;
    };
    let plain = path.qself.is_none()
        && path.path.leading_colon.is_none()
        && path
            .path
            .segments
            .iter()
            .all(|segment| segment.arguments.is_none());
    plain.then(|| segments(&path.path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(body: &str) -> Vec<Route> {
        let source = format!("pub fn routes(routes: &mut Routes<Console>) {{\n{body}\n}}");
        let syntax = syn::parse_file(&source).unwrap();
        read_routes(
            Path::new("routes.rs"),
            &syntax,
            Vec::new(),
            &["order".to_owned()],
        )
        .unwrap()
    }

    fn shown(routes: &[Route]) -> Vec<String> {
        routes
            .iter()
            .map(|route| {
                format!(
                    "{} {} {}",
                    route.method.name(),
                    route.path,
                    route.handler.join("::")
                )
            })
            .collect()
    }

    #[test]
    fn a_resource_is_read_as_the_routes_it_adds() {
        let routes = read(
            "routes.group(\"/orders/{order}\", |lines| {\n\
                 lines.resource::<OrderLine>().only([Resource::Index, Create]);\n\
             });\n\
             routes.resource::<Category>().only([Show]);\n\
             routes.resource::<Tag>();\n\
             routes.resource::<Order>();",
        );

        assert_eq!(
            shown(&routes),
            [
                "GET /orders/{order}/order-lines sofie::resource::list_order_lines",
                "GET /orders/{order}/order-lines/new sofie::resource::new_order_line",
                "POST /orders/{order}/order-lines/new sofie::resource::create_order_line",
                "GET /categories/{category} sofie::resource::show_category",
                "GET /tags sofie::resource::list_tags",
                "GET /tags/{tag} sofie::resource::show_tag",
                "GET /tags/new sofie::resource::new_tag",
                "POST /tags/new sofie::resource::create_tag",
                "GET /tags/{tag}/edit sofie::resource::edit_tag",
                "POST /tags/{tag}/edit sofie::resource::update_tag",
                "GET /orders sofie::resource::list_orders",
                "GET /orders/{order} sofie::resource::show_order",
                "GET /orders/new sofie::resource::new_order",
                "POST /orders/new sofie::resource::create_order",
                "GET /orders/{order}/edit sofie::resource::edit_order",
                "POST /orders/{order}/edit sofie::resource::update_order",
                "POST /orders/{order}/delete sofie::resource::delete_order",
            ]
        );
        assert_eq!(routes[0].screen.as_deref(), Some("order_line_index"));
        assert_eq!(routes[2].screen, None);
    }

    #[test]
    fn a_resource_keeps_only_screens_it_has() {
        let source = "pub fn routes(routes: &mut Routes<Console>) { routes.resource::<Order>().only([Archive]); }";
        let syntax = syn::parse_file(source).unwrap();
        let error = read_routes(
            Path::new("routes.rs"),
            &syntax,
            Vec::new(),
            &["order".to_owned()],
        )
        .unwrap_err();

        assert!(
            error.message.contains("Index, Show, Create, Edit, Delete"),
            "{error}"
        );
    }

    #[test]
    fn a_screen_is_live_and_cached_and_an_action_is_neither() {
        let routes = read(
            "routes.screen(\"/notes\", \"notes\", queries::notes)\n\
                 .live(\"notes\")\n\
                 .cached(Duration::from_secs(60));",
        );
        let source = "pub fn routes(routes: &mut Routes<Console>) { routes.post(\"/notes\", actions::write).cached(TIME); }";
        let syntax = syn::parse_file(source).unwrap();
        let error = read_routes(Path::new("routes.rs"), &syntax, Vec::new(), &[]).unwrap_err();

        assert_eq!(
            routes[0].modifiers,
            [
                Modifier::Live("\"notes\"".to_owned()),
                Modifier::Cached("Duration::from_secs(60)".to_owned())
            ]
        );
        assert!(
            error
                .message
                .contains("`.cached(…)` is not a route modifier"),
            "{error}"
        );
    }

    #[test]
    fn a_path_the_router_would_refuse_is_refused() {
        let refused = |body: &str| {
            let source = format!("pub fn routes(routes: &mut Routes<Console>) {{ {body} }}");
            let syntax = syn::parse_file(&source).unwrap();
            read_routes(Path::new("routes.rs"), &syntax, Vec::new(), &[])
                .unwrap_err()
                .message
        };

        let messages = [
            refused("routes.get(\"orders\", queries::orders);"),
            refused("routes.get(\"/orders/{id\", queries::order);"),
            refused("routes.get(\"/orders/{*rest}/summary\", queries::summary);"),
            refused(
                "routes.group(\"/files/{*rest}\", |files| { files.get(\"/raw\", queries::raw); });",
            ),
            refused("routes.get(\"/orders/{}\", queries::order);"),
            refused("routes.get(\"/my orders\", queries::orders);"),
            refused("routes.get(\"/orders/../admin\", queries::orders);"),
            refused("routes.get(\"/orders/./all\", queries::orders);"),
        ];

        let expected = [
            "must start with `/`",
            "malformed parameter",
            "before its end",
            "before its end",
            "malformed parameter",
            "holds whitespace",
            "`..`",
            "`.`",
        ];
        for (message, expected) in messages.iter().zip(expected) {
            assert!(message.contains(expected), "{message}");
        }
    }

    #[test]
    fn a_redirect_that_a_browser_would_read_as_another_host_is_refused() {
        for target in [
            "//evil.example",
            "/\\\\evil.example",
            "/\\t/evil.example",
            "/\\n/evil.example",
            "/a b",
        ] {
            let source = format!(
                "pub fn routes(routes: &mut Routes<Console>) {{ routes.post(\"/notes\", actions::write).then_redirect(\"{target}\"); }}"
            );
            let syntax = syn::parse_file(&source).unwrap();

            let error = read_routes(Path::new("routes.rs"), &syntax, Vec::new(), &[]).unwrap_err();

            assert!(error.message.contains("then_redirect"), "{target}: {error}");
        }
        let routes =
            read("routes.post(\"/notes\", actions::write).then_redirect(\"/notes/done\");");
        assert_eq!(
            routes[0].then,
            Some(Then::Redirect("/notes/done".to_owned()))
        );
    }

    #[test]
    fn a_queued_job_is_read_as_a_post_of_its_action() {
        let routes = read("routes.queue(\"/exports\", actions::export).throttle(rate);");

        assert_eq!(shown(&routes), ["POST /exports actions::export"]);
        assert_eq!(
            routes[0].modifiers,
            [Modifier::Queued, Modifier::Throttle("rate".to_owned())]
        );
    }

    #[test]
    fn a_billed_action_reads_its_entitlement_and_following_the_provider() {
        let routes = read(
            "routes.get(\"/exports\", queries::exports).entitled(plans::EXPORT);\n\
             routes.post(\"/checkout\", actions::checkout).entitled(plans::PRO).then_follow();",
        );

        assert_eq!(
            routes[0].modifiers,
            [Modifier::Entitled("plans::EXPORT".to_owned())]
        );
        assert_eq!(
            routes[1].modifiers,
            [Modifier::Entitled("plans::PRO".to_owned())]
        );
        assert_eq!(routes[1].then, Some(Then::Follow));
        let source = "pub fn routes(routes: &mut Routes<Console>) { \
                      routes.get(\"/plans\", queries::plans).then_follow(); }";
        let error = read_routes(
            Path::new("routes.rs"),
            &syn::parse_file(source).unwrap(),
            Vec::new(),
            &[],
        )
        .unwrap_err();
        assert!(error.message.contains("follows an action"), "{error}");
    }

    #[test]
    fn documents_file_limits_and_a_redirect_that_may_follow_are_read() {
        let routes = read(
            "routes.get(\"/sitemap.xml\", queries::sitemap).document(Media::Xml);\n\
             routes.post(\"/avatar\", actions::upload).file_limit(1_000_000);\n\
             routes.post(\"/sign-in\", actions::sign_in).then_redirect_or_follow(\"/home\");",
        );

        assert_eq!(
            routes[0].modifiers,
            [Modifier::Document("Media::Xml".to_owned())]
        );
        assert_eq!(
            routes[1].modifiers,
            [Modifier::FileLimit("1_000_000".to_owned())]
        );
        assert_eq!(
            routes[2].then,
            Some(Then::RedirectOrFollow("/home".to_owned()))
        );
    }
}
