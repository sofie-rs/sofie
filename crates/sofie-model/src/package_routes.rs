use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use syn::visit::Visit;
use syn::{Attribute, Expr, ExprMethodCall, GenericArgument, Item, Meta};

use crate::syntax::{first_type_argument, read_file, string_literal, type_last_name, use_paths};
use crate::{App, Code, Function, KnownArea, ModelError};

const ROUTE_TABLE: &str = "ROUTES";
/// A row naming no mount is served by the package's own `routes` function.
pub(crate) const DEFAULT_MOUNT: &str = "routes";

/// A row of a package's `ROUTES` table, kept when its features are on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedRoute {
    pub mount: String,
    pub method: String,
    pub path: String,
    pub handler: String,
    pub permission: Option<String>,
}

/// A package's route function mounted in one of the app's areas.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteMount {
    pub mount: String,
    /// The area type's last segment, as written.
    pub area_type: String,
}

/// A package's route where the app serves it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServedRoute<'app> {
    pub area: String,
    pub path: String,
    pub name: String,
    pub route: &'app ListedRoute,
}

/// A framework package whose routes the app mounts in its areas.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutedPackage {
    pub crate_name: String,
    pub routes: Vec<ListedRoute>,
    pub mounts: Vec<RouteMount>,
}

/// The `ROUTES` tables the package's root exports, each read without compiling.
pub(crate) fn read_route_tables(
    directory: &Path,
    features: &[String],
) -> Result<Vec<ListedRoute>, ModelError> {
    let source = directory.join("src");
    let root = read_file(&source.join("lib.rs"))?;
    let mut routes = Vec::new();
    for item in &root.items {
        let Item::Use(exported) = item else {
            continue;
        };
        if !exported
            .attrs
            .iter()
            .all(|attribute| enabled(attribute, features))
        {
            continue;
        }
        for used in use_paths(&exported.tree) {
            if let [module, table] = used.segments.as_slice()
                && table == ROUTE_TABLE
            {
                routes.extend(read_route_table(&module_file(&source, module), features)?);
            }
        }
    }
    Ok(routes)
}

fn module_file(source: &Path, module: &str) -> PathBuf {
    let file = source.join(format!("{module}.rs"));
    if file.is_file() {
        return file;
    }
    source.join(module).join("mod.rs")
}

pub(crate) fn read_route_table(
    file: &Path,
    features: &[String],
) -> Result<Vec<ListedRoute>, ModelError> {
    let parsed = read_file(file)?;
    let rows = parsed.items.iter().find_map(|item| match item {
        Item::Const(constant) if constant.ident == ROUTE_TABLE => Some(constant.expr.as_ref()),
        _ => None,
    });
    let Some(Expr::Reference(reference)) = rows else {
        return Err(ModelError::in_file(
            file,
            "found no `const ROUTES: &[ListedRoute]`",
        ));
    };
    let Expr::Array(array) = reference.expr.as_ref() else {
        return Err(ModelError::in_file(
            file,
            "`ROUTES` is not an array literal",
        ));
    };
    let mut routes = Vec::new();
    for row in &array.elems {
        let Expr::Call(call) = row else {
            return Err(ModelError::in_file(file, "a `ROUTES` row is not a call"));
        };
        if !call
            .attrs
            .iter()
            .all(|attribute| enabled(attribute, features))
        {
            continue;
        }
        let arguments: Option<Vec<String>> = call.args.iter().map(string_literal).collect();
        let row = match arguments.as_deref() {
            Some([method, path, handler]) => listed(DEFAULT_MOUNT, method, path, handler, None),
            Some([mount, method, path, handler]) => listed(mount, method, path, handler, None),
            Some([mount, method, path, handler, permission]) => {
                listed(mount, method, path, handler, Some(permission))
            }
            _ => {
                return Err(ModelError::in_file(
                    file,
                    "a `ROUTES` row holds other than literals",
                ));
            }
        };
        routes.push(row);
    }
    Ok(routes)
}

fn listed(
    mount: &str,
    method: &str,
    path: &str,
    handler: &str,
    permission: Option<&String>,
) -> ListedRoute {
    ListedRoute {
        mount: mount.to_owned(),
        method: method.to_owned(),
        path: path.to_owned(),
        handler: handler.to_owned(),
        permission: permission.cloned(),
    }
}

/// Only `cfg` predicates over features: a package gates its routes by nothing else.
fn enabled(attribute: &Attribute, features: &[String]) -> bool {
    if !attribute.path().is_ident("cfg") {
        return true;
    }
    attribute
        .parse_args::<Meta>()
        .is_ok_and(|predicate| holds(&predicate, features))
}

fn holds(predicate: &Meta, features: &[String]) -> bool {
    match predicate {
        Meta::NameValue(pair) if pair.path.is_ident("feature") => {
            string_literal(&pair.value).is_some_and(|feature| features.contains(&feature))
        }
        Meta::List(list) => {
            let Ok(inner) = list.parse_args_with(
                syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated,
            ) else {
                return false;
            };
            let mut inner = inner.iter();
            if list.path.is_ident("any") {
                return inner.any(|predicate| holds(predicate, features));
            }
            if list.path.is_ident("all") {
                return inner.all(|predicate| holds(predicate, features));
            }
            list.path.is_ident("not")
                && inner
                    .next()
                    .is_some_and(|predicate| !holds(predicate, features))
        }
        _ => false,
    }
}

/// `.area::<Console>("sofie_identity", sofie_identity::permitted_routes::<Console>)` in the
/// app's own code.
pub(crate) fn read_mounts(app: &BTreeMap<PathBuf, syn::File>, crate_name: &str) -> Vec<RouteMount> {
    let mut found = Mounts {
        crate_name: Some(crate_name),
        mounts: Vec::new(),
    };
    for syntax in app.values() {
        found.visit_file(syntax);
    }
    found.mounts
}

/// `app.area::<Web>(module, routes)` in the package's root: what its `Module` mounts itself.
pub(crate) fn read_module_mounts(directory: &Path) -> Result<Vec<RouteMount>, ModelError> {
    let mut found = Mounts {
        crate_name: None,
        mounts: Vec::new(),
    };
    found.visit_file(&read_file(&directory.join("src").join("lib.rs"))?);
    Ok(found.mounts)
}

/// `crate_name` is `None` in the package's own code, which names its functions unqualified.
struct Mounts<'name> {
    crate_name: Option<&'name str>,
    mounts: Vec<RouteMount>,
}

impl Mounts<'_> {
    fn mount(&self, call: &ExprMethodCall) -> Option<RouteMount> {
        let [module, Expr::Path(function)] = call.args.iter().collect::<Vec<_>>()[..] else {
            return None;
        };
        let segments: Vec<&syn::PathSegment> = function.path.segments.iter().collect();
        let own = match self.crate_name {
            Some(crate_name) => {
                let [first, rest @ ..] = segments.as_slice() else {
                    return None;
                };
                if string_literal(module).as_deref() != Some(crate_name)
                    || first.ident != crate_name
                {
                    return None;
                }
                rest
            }
            None => segments.as_slice(),
        };
        let area_type = own
            .last()
            .and_then(|last| type_last_name(first_type_argument(last)?))
            .or_else(|| turbofish_type(call))?;
        let mount = own
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect::<Vec<_>>()
            .join("::");
        Some(RouteMount { mount, area_type })
    }
}

fn turbofish_type(call: &ExprMethodCall) -> Option<String> {
    match call.turbofish.as_ref()?.args.first()? {
        GenericArgument::Type(ty) => type_last_name(ty),
        _ => None,
    }
}

impl<'ast> Visit<'ast> for Mounts<'_> {
    fn visit_expr_method_call(&mut self, call: &'ast ExprMethodCall) {
        if call.method == "area"
            && let Some(mount) = self.mount(call)
            && !self.mounts.contains(&mount)
        {
            self.mounts.push(mount);
        }
        syn::visit::visit_expr_method_call(self, call);
    }
}

impl App {
    #[must_use]
    pub fn package_routes(&self) -> PackageRoutes<'_> {
        PackageRoutes {
            app: self,
            areas: self.areas(),
        }
    }
}

/// The framework packages' routes where the app's areas serve them.
#[derive(Clone, Debug)]
pub struct PackageRoutes<'app> {
    pub(crate) app: &'app App,
    pub(crate) areas: Vec<KnownArea<'app>>,
}

impl<'app> PackageRoutes<'app> {
    /// Those of the packages other than sofie-identity.
    #[must_use]
    pub fn other_packages(&self) -> Vec<ServedRoute<'app>> {
        self.app
            .packages
            .iter()
            .flat_map(|package| self.mounted(&package.crate_name, &package.routes, &package.mounts))
            .collect()
    }

    pub(crate) fn mounted(
        &self,
        crate_name: &str,
        routes: &'app [ListedRoute],
        mounts: impl IntoIterator<Item = &'app RouteMount>,
    ) -> Vec<ServedRoute<'app>> {
        let mut served = Vec::new();
        for mount in mounts {
            let Some(area) = self
                .areas
                .iter()
                .find(|area| area.type_path.rsplit("::").next() == Some(mount.area_type.as_str()))
            else {
                continue;
            };
            let Some(prefix) = area.prefix() else {
                continue;
            };
            let mounted = routes.iter().filter(|route| route.mount == mount.mount);
            for route in mounted {
                let path = format!("{prefix}{}", route.path);
                served.push(served_route(crate_name, area.name, area.name, path, route));
            }
        }
        served
    }
}

/// Named `<crate>.<area>.<handler>`; a member route is named per member area it signs in to,
/// so each area's names stay unique.
pub(crate) fn served_route<'app>(
    crate_name: &str,
    area: &str,
    named_for: &str,
    path: String,
    route: &'app ListedRoute,
) -> ServedRoute<'app> {
    ServedRoute {
        area: area.to_owned(),
        path,
        name: format!("{crate_name}.{named_for}.{}", route.handler),
        route,
    }
}

impl ServedRoute<'_> {
    /// The crate serving it, which its name starts with.
    #[must_use]
    pub fn package(&self) -> &str {
        self.name.split('.').next().unwrap_or_default()
    }
}

impl ListedRoute {
    #[must_use]
    pub fn function<'code>(&self, code: &'code Code) -> Option<&'code Function> {
        code.functions
            .iter()
            .find(|function| function.path.last() == Some(&self.handler))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use super::{read_module_mounts, read_mounts, read_route_table, read_route_tables};
    use crate::testing::TemporaryDirectory;

    fn kept_by(
        features: &[&str],
        read: impl Fn(&[String]) -> Vec<super::ListedRoute>,
    ) -> Vec<String> {
        let features: Vec<String> = features.iter().map(|&name| name.to_owned()).collect();
        read(&features)
            .into_iter()
            .map(|route| format!("{} {}", route.mount, route.handler))
            .collect()
    }

    #[test]
    fn keeps_the_rows_whose_features_are_on() {
        let directory = TemporaryDirectory::new("route-table");
        let file = directory.write(
            "route_table.rs",
            "pub const ROUTES: &[ListedRoute] = &[\n\
             member(\"GET\", \"{sign_in}\", \"sign_in_page\"),\n\
             #[cfg(any(feature = \"password-reset\", feature = \"email-verification\"))]\n\
             member(\"GET\", \"{prefix}/link-sent\", \"link_sent_page\"),\n\
             #[cfg(feature = \"token-screens\")]\n\
             permitted(\"tokens::screen::routes\", \"GET\", \"/tokens\", \"list_tokens\", \"t.*.read\"),\n\
             ];\n",
        );
        let read = |features: &[String]| read_route_table(&file, features).unwrap();

        assert_eq!(kept_by(&[], read), ["routes sign_in_page"]);
        assert_eq!(
            kept_by(&["email-verification", "token-screens"], read),
            [
                "routes sign_in_page",
                "routes link_sent_page",
                "tokens::screen::routes list_tokens"
            ]
        );
    }

    #[test]
    fn reads_every_table_the_root_exports_while_its_features_are_on() {
        let directory = TemporaryDirectory::new("route-tables");
        directory.write(
            "src/lib.rs",
            "mod public;\n#[cfg(feature = \"admin\")]\nmod admin;\n\n\
             pub use public::{ROUTES, routes};\n\
             #[cfg(feature = \"admin\")]\npub use admin::ROUTES as ADMIN_ROUTES;\n",
        );
        directory.write(
            "src/public.rs",
            "pub const ROUTES: &[ListedRoute] = &[listed(\"GET\", \"/pages/{slug}\", \"page\")];\n",
        );
        directory.write(
            "src/admin/mod.rs",
            "pub const ROUTES: &[ListedRoute] = &[\n\
             listed(\"admin_routes\", \"POST\", \"/pages/{page}/publish\", \"publish\"),\n];\n",
        );
        let read = |features: &[String]| read_route_tables(directory.root(), features).unwrap();

        assert_eq!(kept_by(&[], read), ["routes page"]);
        assert_eq!(
            kept_by(&["admin"], read),
            ["routes page", "admin_routes publish"]
        );
    }

    #[test]
    fn the_framework_packages_list_their_routes() {
        let package = |name: &str| Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(name);
        let handlers = |name: &str, features: &[&str]| {
            kept_by(features, |features| {
                read_route_tables(&package(name), features).unwrap()
            })
        };

        assert!(handlers("sofie-identity", &[]).contains(&"routes sign_in_page".to_owned()));
        assert_eq!(
            handlers("sofie-tenancy", &[]),
            [
                "organization_routes list_members",
                "organization_routes invite_member",
                "organization_routes make_owner",
                "organization_routes make_admin",
                "organization_routes make_member",
                "organization_routes withdraw_invitation",
                "organization_routes remove_member",
                "organization_routes hand_over_organization",
                "organization_routes leave_organization",
                "organization_routes rename_organization",
                "organization_routes delete_organization",
                "member_routes list_organizations",
                "member_routes found_organization",
                "member_routes enter_organization",
                "member_routes invitation_page",
                "member_routes accept_invitation",
            ]
        );
        assert_eq!(
            handlers("sofie-content", &[]),
            ["routes page", "routes sitemap"]
        );
        assert_eq!(handlers("sofie-content", &["admin"]).len(), 10);
        assert_eq!(
            read_module_mounts(&package("sofie-content"))
                .unwrap()
                .into_iter()
                .map(|mount| (mount.mount, mount.area_type))
                .collect::<Vec<_>>(),
            [("routes".to_owned(), "Web".to_owned())]
        );
    }

    #[test]
    fn finds_the_routes_the_app_mounts_from_a_package() {
        let main = syn::parse_str(
            "fn app(db: Db) -> App {\n    wiring::app(db)\n        \
             .area::<Console>(\"sofie_identity\", sofie_identity::permitted_routes::<Console>)\n        \
             .area::<Web>(\"sofie_content\", sofie_content::routes)\n        \
             .area(\"sofie_tenancy\", sofie_tenancy::member_routes::<Member, Staff>)\n        \
             .area::<Console>(\"shop\", shop::routes)\n}\n",
        )
        .unwrap();
        let app = BTreeMap::from([(PathBuf::from("src/main.rs"), main)]);
        let mounts = |crate_name: &str| -> Vec<(String, String)> {
            read_mounts(&app, crate_name)
                .into_iter()
                .map(|mount| (mount.mount, mount.area_type))
                .collect()
        };
        let pair = |mount: &str, area: &str| (mount.to_owned(), area.to_owned());

        assert_eq!(
            mounts("sofie_identity"),
            [pair("permitted_routes", "Console")]
        );
        assert_eq!(mounts("sofie_content"), [pair("routes", "Web")]);
        assert_eq!(mounts("sofie_tenancy"), [pair("member_routes", "Member")]);
    }
}
