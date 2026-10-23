use std::fs;
use std::path::{Path, PathBuf};

use syn::visit::Visit;
use syn::{Expr, ExprMethodCall};

use crate::load::rust_files;
use crate::naming::screen_key;
use crate::syntax::{parse_tokens, string_literal, tokenize};
use crate::{Identity, KnownArea, ModelError};

const SHIPPED_CALL: &str = "shipped";
const SCREEN_CALL: &str = "screen";

/// A screen a sofie-identity route renders from a template file of its own:
/// `.shipped(path, name, (include_str!(file), describe), loader).name(handler)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShippedScreen {
    pub handler: String,
    pub name: String,
    pub file: PathBuf,
    pub source: String,
}

/// A shipped screen in the area the app mounts its routes in, under the key it renders as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountedScreen<'app> {
    pub key: String,
    pub screen: &'app ShippedScreen,
}

/// Only files that call `shipped` are parsed, so loading an app stays fast.
pub(crate) fn read_shipped(directory: &Path) -> Result<Vec<ShippedScreen>, ModelError> {
    let mut screens = Vec::new();
    for file in rust_files(&directory.join("src"))? {
        let text = fs::read_to_string(&file).map_err(|error| {
            ModelError::in_file(&file, format!("cannot read the file: {error}"))
        })?;
        if !text.contains(".shipped(") {
            continue;
        }
        let syntax = parse_tokens(&file, tokenize(&file, &text)?)?;
        let mut calls = ShippedCalls::default();
        calls.visit_file(&syntax);
        for (handler, name, template) in calls.found {
            let template = file.parent().unwrap_or(directory).join(template);
            let source = fs::read_to_string(&template).map_err(|error| {
                ModelError::in_file(&template, format!("cannot read the template: {error}"))
            })?;
            screens.push(ShippedScreen {
                handler,
                name,
                file: template,
                source,
            });
        }
    }
    Ok(screens)
}

/// A package route rendering one of its `src/screens` templates:
/// `.screen(path, name, handler).name("…")` or `.name_for::<A>("…")`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenRoute {
    pub screen: String,
    pub handler: String,
}

/// Only calls naming the screen and route by literals are read; a screen named by a variable
/// has no route here.
pub(crate) fn read_screen_routes(directory: &Path) -> Result<Vec<ScreenRoute>, ModelError> {
    let mut found = ScreenCalls::default();
    for file in rust_files(&directory.join("src"))? {
        let text = fs::read_to_string(&file).map_err(|error| {
            ModelError::in_file(&file, format!("cannot read the file: {error}"))
        })?;
        if !text.contains(".screen(") {
            continue;
        }
        found.visit_file(&parse_tokens(&file, tokenize(&file, &text)?)?);
    }
    Ok(found.routes)
}

#[derive(Default)]
struct ScreenCalls {
    routes: Vec<ScreenRoute>,
}

impl<'ast> Visit<'ast> for ScreenCalls {
    fn visit_expr_method_call(&mut self, call: &'ast ExprMethodCall) {
        if (call.method == "name" || call.method == "name_for")
            && let Expr::MethodCall(screen) = call.receiver.as_ref()
            && screen.method == SCREEN_CALL
            && let Some(handler) = call.args.first().and_then(string_literal)
            && let Some(name) = screen.args.iter().nth(1).and_then(string_literal)
        {
            self.routes.push(ScreenRoute {
                screen: name,
                handler,
            });
        }
        syn::visit::visit_expr_method_call(self, call);
    }
}

#[derive(Default)]
struct ShippedCalls {
    /// Handler, screen name and the template file as `include_str!` names it.
    found: Vec<(String, String, String)>,
}

impl<'ast> Visit<'ast> for ShippedCalls {
    fn visit_expr_method_call(&mut self, call: &'ast ExprMethodCall) {
        if call.method == "name"
            && let Expr::MethodCall(shipped) = call.receiver.as_ref()
            && shipped.method == SHIPPED_CALL
            && let Some(handler) = call.args.first().and_then(string_literal)
            && let Some((name, template)) = shipped_arguments(shipped)
        {
            self.found.push((handler, name, template));
        }
        syn::visit::visit_expr_method_call(self, call);
    }
}

fn shipped_arguments(call: &ExprMethodCall) -> Option<(String, String)> {
    let [_, name, Expr::Tuple(template), _] = call.args.iter().collect::<Vec<_>>()[..] else {
        return None;
    };
    let Some(Expr::Macro(included)) = template.elems.first() else {
        return None;
    };
    if !included.mac.path.is_ident("include_str") {
        return None;
    }
    let file = included.mac.parse_body::<syn::LitStr>().ok()?.value();
    Some((string_literal(name)?, file))
}

impl Identity {
    /// Read on demand: only `explain` needs them.
    pub fn screen_routes(&self) -> Result<Vec<ScreenRoute>, ModelError> {
        read_screen_routes(&self.directory)
    }

    /// The shipped screens of the routes the app's code mounts in `area`.
    #[must_use]
    pub fn mounted_screens(&self, area: &KnownArea<'_>) -> Vec<MountedScreen<'_>> {
        let area_type = area.type_path.rsplit("::").next().unwrap_or_default();
        self.mounts
            .iter()
            .filter(|mount| mount.area_type == area_type)
            .flat_map(|mount| {
                self.routes
                    .iter()
                    .filter(|route| route.mount == mount.mount)
            })
            .filter_map(|route| self.shipped_by(&route.handler))
            .map(|screen| MountedScreen {
                key: screen_key(crate::identity::IDENTITY_CRATE, area.name, &screen.name),
                screen,
            })
            .collect()
    }

    fn shipped_by(&self, handler: &str) -> Option<&ShippedScreen> {
        self.shipped.iter().find(|screen| screen.handler == handler)
    }
}

#[cfg(test)]
mod tests {
    use super::read_shipped;
    use crate::testing::TemporaryDirectory;

    #[test]
    fn a_shipped_screen_is_read_with_its_handler_and_template() {
        let directory = TemporaryDirectory::new("shipped");
        directory.write("src/sessions/screen.html", "<p>Sessions</p>");
        directory.write(
            "src/sessions/screen.rs",
            "pub fn routes(routes: &mut Routes<A>) {\n\
             routes.shipped(\"/sessions\", \"sessions\", (include_str!(\"screen.html\"), None), \
             own).name(\"list_sessions\");\n\
             routes.post(\"/sessions/others\", end).name(\"end_other_sessions\");\n}\n",
        );
        directory.write("src/lib.rs", "pub mod sessions;\n");

        let screens = read_shipped(directory.root()).unwrap();

        assert_eq!(screens.len(), 1);
        assert_eq!(screens[0].handler, "list_sessions");
        assert_eq!(screens[0].name, "sessions");
        assert_eq!(screens[0].source, "<p>Sessions</p>");
        assert!(screens[0].file.ends_with("src/sessions/screen.html"));
    }

    #[test]
    fn a_package_screen_is_read_with_the_route_rendering_it() {
        let directory = TemporaryDirectory::new("screen-routes");
        directory.write(
            "src/lib.rs",
            "pub fn routes<A: MemberArea>(routes: &mut Routes<Web>) {\n\
             routes.screen(A::SIGN_IN, \"sign_in\", page::<A>).name_for::<A>(\"sign_in_page\");\n\
             routes.screen(\"/pages/{slug}\", \"page\", show).name(\"page\").signed();\n\
             routes.screen(&asking, ask_screen, ask).name_for::<A>(ask_name);\n}\n",
        );

        let routes = super::read_screen_routes(directory.root()).unwrap();

        let found: Vec<(&str, &str)> = routes
            .iter()
            .map(|route| (route.screen.as_str(), route.handler.as_str()))
            .collect();
        assert_eq!(found, [("sign_in", "sign_in_page"), ("page", "page")]);
    }

    #[test]
    fn sofie_identity_s_shipped_screens_are_found_by_their_handlers() {
        let identity = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../sofie-identity");

        let screens = read_shipped(&identity).unwrap();

        let mut handlers: Vec<&str> = screens
            .iter()
            .map(|screen| screen.handler.as_str())
            .collect();
        handlers.sort_unstable();
        assert_eq!(
            handlers,
            [
                "account_data_page",
                "list_blocks",
                "list_connections",
                "list_flags",
                "list_impersonations",
                "list_passkeys",
                "list_sessions",
                "list_settings",
                "list_tokens",
                "two_factor_page",
            ]
        );
    }

    #[test]
    fn a_shipped_screen_is_mounted_in_the_area_the_app_adds_its_routes_to() {
        use super::ShippedScreen;
        use crate::testing::{app, define_area, module};
        use crate::{Identity, ListedRoute, RouteMount};

        let mut notes = module("notes", "/app/modules/notes");
        define_area(&mut notes, "board");
        let mut demo = app("/app", vec![notes]);
        let route = |mount: &str, handler: &str| ListedRoute {
            mount: mount.to_owned(),
            method: "GET".to_owned(),
            path: format!("/{handler}"),
            handler: handler.to_owned(),
            permission: None,
        };
        let shipped = |handler: &str, name: &str| ShippedScreen {
            handler: handler.to_owned(),
            name: name.to_owned(),
            file: format!("{name}.html").into(),
            source: String::new(),
        };
        demo.identity = Some(Identity {
            routes: vec![
                route("sessions::routes", "list_sessions"),
                route("passkeys::routes", "list_passkeys"),
            ],
            mounts: vec![RouteMount {
                mount: "sessions::routes".to_owned(),
                area_type: "Member".to_owned(),
            }],
            shipped: vec![
                shipped("list_sessions", "sessions"),
                shipped("list_passkeys", "passkeys"),
            ],
            ..Identity::default()
        });

        let areas = demo.areas();
        let identity = demo.identity.as_ref().unwrap();

        let mounted = identity.mounted_screens(&areas[1]);

        let keys: Vec<&str> = mounted.iter().map(|screen| screen.key.as_str()).collect();
        assert_eq!(keys, ["sofie_identity/board/sessions"]);
        assert_eq!(identity.mounted_screens(&areas[0]), []);
    }
}
