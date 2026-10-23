use std::path::Path;

use sofie_check::Screens;
use sofie_model::naming::{RouteSite, bound_aggregate, path_parameters, snake_case};
use sofie_model::{Aggregate, App, KnownArea, Method, Module};

/// A uuid no record has, for a route called before any record exists.
const NO_RECORD: &str = "\"00000000-0000-4000-8000-000000000000\"";

pub(super) struct Making {
    pub(super) statements: String,
}

#[derive(Clone, Copy)]
pub(super) struct Requests<'a> {
    pub(super) app: &'a App,
    pub(super) module: &'a Module,
    pub(super) screens: &'a Screens,
}

impl Requests<'_> {
    pub(super) fn crate_name(&self) -> &str {
        &self.module.krate.crate_name
    }

    pub(super) fn path(&self, aggregate: &Aggregate) -> String {
        format!("{}::{}", self.crate_name(), aggregate.path.join("::"))
    }

    pub(super) fn two_records(&self, aggregate: &Aggregate, derived: bool) -> Option<Making> {
        if derived {
            let path = self.path(aggregate);
            return Some(Making {
                statements: format!(
                    "let first = app.fixture::<{path}>().await.id().uuid();\n\
                     let second = app.fixture::<{path}>().await.id().uuid();\n"
                ),
            });
        }
        let site = self.creating_route(&aggregate.name, None)?;
        let area = self.app.area(&site.area.area)?;
        let principal = match site.permission(self.app) {
            Some(permission) => granted_principal(self.app, &area, "member", &[permission])?,
            None => principal(self.app, &area, "member").unwrap_or_default(),
        };
        let first = self.create_through(&site, "member", "first")?;
        let second = self.create_through(&site, "member", "second")?;
        Some(Making {
            statements: format!("{principal}{first}{second}"),
        })
    }

    pub(super) fn creating_route(
        &self,
        aggregate: &str,
        area: Option<&str>,
    ) -> Option<RouteSite<'_>> {
        self.app.route_sites().into_iter().find(|site| {
            std::ptr::eq(site.module, self.module)
                && area.is_none_or(|area| site.area.area == area)
                && site.route.method == Method::Post
                && !site.route.path.contains('{')
                && self.creates(site, aggregate)
        })
    }

    fn creates(&self, site: &RouteSite, aggregate: &str) -> bool {
        if let Some((resource, action)) = self.resource(site) {
            return action == "Create" && resource.name == aggregate;
        }
        self.module
            .code
            .function(&site.route.handler)
            .and_then(|function| function.output.as_ref())
            .is_some_and(|output| output.without_result().name == aggregate)
    }

    /// A resource's route, which sofie serves, as its aggregate and the `Create` or `Update`
    /// trait whose input it takes; `None` for a resource route posting no input.
    fn resource(&self, site: &RouteSite) -> Option<(&Aggregate, &'static str)> {
        if !site.route.is_resource() {
            return None;
        }
        let handler = site.route.handler.last()?;
        let (action, name) = [("create_", "Create"), ("update_", "Update")]
            .into_iter()
            .find_map(|(prefix, action)| Some((action, handler.strip_prefix(prefix)?)))?;
        let aggregate = self
            .module
            .code
            .aggregates
            .iter()
            .find(|aggregate| snake_case(&aggregate.name) == name)?;
        Some((aggregate, action))
    }

    /// The permission a hand-written factory's aggregate is created behind, for an app whose
    /// scaffold cannot grant it (no sofie-identity, or an area it does not sign members in to).
    pub(super) fn creation_permission(&self, aggregate: &str) -> Option<(String, String)> {
        let site = self.creating_route(aggregate, None)?;
        Some((site.name(), site.permission(self.app)?))
    }

    /// What a call of `site` needs: its own permission and those of the routes creating the
    /// records its path names.
    fn permissions(&self, site: &RouteSite) -> Vec<String> {
        let created = path_parameters(&site.route.path)
            .into_iter()
            .filter_map(|parameter| bound_aggregate(self.module, parameter))
            .filter(|aggregate| writes_factory(&aggregate.file))
            .filter_map(|aggregate| {
                self.creating_route(&aggregate.name, Some(&site.area.area))?
                    .permission(self.app)
            });
        let mut permissions: Vec<String> = site
            .permission(self.app)
            .into_iter()
            .chain(created)
            .collect();
        permissions.sort_unstable();
        permissions.dedup();
        permissions
    }

    pub(super) fn create_through(
        &self,
        site: &RouteSite,
        principal: &str,
        variable: &str,
    ) -> Option<String> {
        let request = self.request(site, &[])?;
        let caller = if self.app.area(&site.area.area)?.is_public() {
            "app".to_owned()
        } else {
            format!("app.as_(&{principal})")
        };
        Some(format!(
            "let {variable} = {caller}.request(&{request}){}.await.record_id();\n",
            self.body_of(site)
        ))
    }

    pub(super) fn make_parameter(
        &self,
        site: &RouteSite,
        parameter: &str,
        principal: &str,
        variable: &str,
    ) -> Option<String> {
        let aggregate = bound_aggregate(self.module, parameter)?;
        if !writes_factory(&aggregate.file) {
            let owner = if aggregate.is_owned() {
                format!(".owned_by({principal}.id())")
            } else {
                String::new()
            };
            return Some(format!(
                "let {variable} = app.fixture::<{}>(){owner}.await.id().uuid();\n",
                self.path(aggregate)
            ));
        }
        let creating = self.creating_route(&aggregate.name, Some(&site.area.area))?;
        self.create_through(&creating, principal, variable)
    }

    pub(super) fn body_of(&self, site: &RouteSite) -> String {
        if let Some((aggregate, action)) = self.resource(site) {
            return format!(
                ".json(&sofie_testing::fixture_input::<<{} as sofie::db::{action}>::Input>(&app))",
                self.path(aggregate)
            );
        }
        let code = &self.module.code;
        let input = code
            .function(&site.route.handler)
            .and_then(|function| code.input_of(function));
        match input {
            Some(input) => format!(
                ".json(&sofie_testing::fixture_input::<{}::{}>(&app))",
                self.crate_name(),
                input.path.join("::")
            ),
            None => ".accept_json()".to_owned(),
        }
    }

    pub(super) fn request(&self, site: &RouteSite, arguments: &[&str]) -> Option<String> {
        let contract = self.module.contract.as_ref()?;
        if !self
            .module
            .contract_exports
            .iter()
            .any(|export| export == "routes")
        {
            return None;
        }
        let arguments: Vec<String> = path_parameters(&site.route.path)
            .into_iter()
            .enumerate()
            .map(|(index, parameter)| {
                let given = arguments.get(index).copied();
                let Some(aggregate) = bound_aggregate(self.module, parameter) else {
                    return given.unwrap_or(NO_RECORD).to_owned();
                };
                let uuid =
                    given.map_or_else(|| format!("{NO_RECORD}.parse().unwrap()"), str::to_owned);
                format!(
                    "sofie::db::Id::<{}>::from_uuid({uuid})",
                    self.path(aggregate)
                )
            })
            .collect();
        Some(format!(
            "{}::routes::{}::{}({})",
            contract.crate_name,
            site.area.area,
            site.route.handler.last()?,
            arguments.join(", ")
        ))
    }

    pub(super) fn prepared(&self, site: &RouteSite, area: &KnownArea) -> Option<(String, String)> {
        let parameters = path_parameters(&site.route.path);
        let mut statements = self.member(&[site], area)?;
        let mut variables = Vec::new();
        for (index, parameter) in parameters.iter().enumerate() {
            let variable = format!("record_{index}");
            statements.push_str(&self.make_parameter(site, parameter, "member", &variable)?);
            variables.push(variable);
        }
        let arguments: Vec<&str> = variables.iter().map(String::as_str).collect();
        let request = self.request(site, &arguments)?;
        Some((statements, format!("{}.request(&{request})", caller(area))))
    }

    /// `member`, holding what each of `sites` needs where the area is `Permitted`.
    pub(super) fn member(&self, sites: &[&RouteSite], area: &KnownArea) -> Option<String> {
        if !area.is_permitted() {
            return Some(principal(self.app, area, "member").unwrap_or_default());
        }
        let mut permissions: Vec<String> = sites
            .iter()
            .flat_map(|site| self.permissions(site))
            .collect();
        permissions.sort_unstable();
        permissions.dedup();
        granted_principal(self.app, area, "member", &permissions)
    }
}

pub(super) fn caller(area: &KnownArea) -> &'static str {
    if area.is_public() {
        "app"
    } else {
        "app.as_(&member)"
    }
}

/// Needs `sofie_identity::testing::TestMember` in scope.
pub(super) const SIGNED_UP_MEMBER: &str = "::member(&app)";

pub(super) const GRANTED: &str = "sofie_identity::testing::granted::<";

/// A member of a `Permitted` area holding exactly `permissions`; none where sofie-identity
/// does not sign members in to the area.
pub(super) fn granted_principal(
    app: &App,
    area: &KnownArea,
    variable: &str,
    permissions: &[String],
) -> Option<String> {
    let definition = area.definition?;
    let signs_in =
        app.identity.is_some() && definition.is_permitted() && definition.is_member_area();
    if !signs_in {
        return None;
    }
    let listed: Vec<String> = permissions
        .iter()
        .map(|permission| format!("{permission:?}"))
        .collect();
    Some(format!(
        "let {variable} = {GRANTED}{}>(&app, &[{}]).await;\n",
        area.type_path,
        listed.join(", ")
    ))
}

pub(super) fn principal(app: &App, area: &KnownArea, variable: &str) -> Option<String> {
    let definition = area.definition?;
    if area.is_public() {
        return None;
    }
    let path = &area.type_path;
    let member =
        app.identity.is_some() && !definition.is_permitted() && definition.is_member_area();
    Some(if member {
        format!(
            "let {variable} = sofie_testing::TestPrincipal::<{path}>{SIGNED_UP_MEMBER}.await;\n"
        )
    } else {
        format!("let {variable} = sofie_testing::TestPrincipal::<{path}>::fresh(&app);\n")
    })
}

pub(super) fn writes_factory(file: &Path) -> bool {
    let is_root = file
        .file_stem()
        .is_some_and(|stem| stem == "mod" || stem == "lib" || stem == "main");
    let folder = match file.parent() {
        Some(parent) if is_root => parent.to_path_buf(),
        _ => file.with_extension(""),
    };
    folder.join("factory.rs").is_file() || folder.join("factory").join("mod.rs").is_file()
}
