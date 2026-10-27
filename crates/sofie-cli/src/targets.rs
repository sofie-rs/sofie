use std::collections::BTreeMap;

use sofie_model::naming::{RouteSite, path_parameters};
use sofie_model::{App, Method, SignInPaths, TypeName};

use crate::gen_command::GenError;
use crate::inputs::Field;

pub(crate) const IDENTITY: &str = "sofie_identity";

pub(crate) enum Access {
    Anonymous,
    Member(SignInPaths),
    Unreachable(&'static str),
}

pub(crate) struct Target {
    pub(crate) name: String,
    pub(crate) module: String,
    pub(crate) method: Method,
    pub(crate) path: String,
    pub(crate) area: String,
    pub(crate) member: Option<SignInPaths>,
    pub(crate) fields: Vec<Field>,
}

pub(crate) struct Targets {
    pub(crate) targets: Vec<Target>,
    pub(crate) skipped: Vec<String>,
}

/// Reaching an area as a member works only through sofie-identity, whose
/// sign-in and sign-up routes are targets too.
pub(crate) fn targets(app: &App) -> Result<Targets, GenError> {
    let mut found = Targets {
        targets: Vec::new(),
        skipped: Vec::new(),
    };
    for site in app.route_sites() {
        let Some(path) = site.full_path(app) else {
            let reason = format!("area `{}` has no literal prefix", site.area.area);
            found.skipped.push(format!("{}: {reason}", site.name()));
            continue;
        };
        let member = match access(app, &site.area.area)? {
            Access::Anonymous => None,
            Access::Member(paths) => {
                found.add_identity(paths.clone());
                Some(paths)
            }
            Access::Unreachable(reason) => {
                found.skipped.push(format!(
                    "{}: reached signed out only; {reason}",
                    site.name()
                ));
                None
            }
        };
        found.targets.push(Target {
            name: site.name(),
            module: site.module.krate.crate_name.clone(),
            method: site.route.method,
            path: path.replace("{*", "{"),
            area: site.area.area.clone(),
            member,
            fields: input_fields(&site),
        });
    }
    Ok(found)
}

impl Target {
    /// What `sofie seed` fills and `sofie fuzz` gives the other member records with.
    pub(crate) fn is_creating(&self) -> bool {
        self.not_postable().is_none() && !self.fields.is_empty()
    }

    /// Why a seed file's own inputs cannot be posted here; those need no
    /// fields the model reads.
    pub(crate) fn not_postable(&self) -> Option<&'static str> {
        if self.method != Method::Post {
            return Some("it is not a POST route");
        }
        if !path_parameters(&self.path).is_empty() {
            return Some("its path takes a parameter");
        }
        if self.module == IDENTITY {
            return Some("sofie-identity's routes are not seeded");
        }
        None
    }
}

pub(crate) fn member_areas(targets: &[Target]) -> BTreeMap<&str, &SignInPaths> {
    targets
        .iter()
        .filter_map(|target| Some((target.area.as_str(), target.member.as_ref()?)))
        .collect()
}

impl Targets {
    fn add_identity(&mut self, paths: SignInPaths) {
        let credentials = || {
            vec![
                Field::of("email", &plain("Email"), &[]),
                Field::of("password", &plain("Password"), &[]),
            ]
        };
        let routes = std::iter::once(("sign_in", paths.sign_in))
            .chain(paths.sign_up.map(|path| ("sign_up", path)));
        for (handler, path) in routes {
            if self.targets.iter().any(|target| target.path == path) {
                continue;
            }
            self.targets.push(Target {
                name: format!("{IDENTITY}.{handler}"),
                module: IDENTITY.to_owned(),
                method: Method::Post,
                path,
                area: "web".to_owned(),
                member: None,
                fields: credentials(),
            });
        }
    }
}

fn plain(name: &str) -> TypeName {
    TypeName {
        name: name.to_owned(),
        arguments: Vec::new(),
    }
}

pub(crate) fn input_fields(site: &RouteSite) -> Vec<Field> {
    let code = &site.module.code;
    let Some(input) = code
        .function(&site.route.handler)
        .and_then(|function| code.input_of(function))
    else {
        return Vec::new();
    };
    input
        .fields
        .iter()
        .map(|field| Field::of(&field.name, &field.ty, &code.enums))
        .collect()
}

pub(crate) fn access(app: &App, area: &str) -> Result<Access, GenError> {
    let Some(known) = app.area(area) else {
        return Ok(Access::Unreachable("the area is not declared"));
    };
    if known.is_public() {
        return Ok(Access::Anonymous);
    }
    if known.is_permitted() {
        return Ok(Access::Unreachable(
            "a `Permitted` area needs a permission grant sofie cannot give",
        ));
    }
    let member = known
        .definition
        .filter(|definition| definition.is_member_area());
    let Some(definition) = member.filter(|_| app.identity.is_some()) else {
        return Ok(Access::Unreachable(
            "sofie signs in only to sofie-identity's member areas",
        ));
    };
    match definition.sign_in_paths().map_err(GenError::Model)? {
        Some(paths) => Ok(Access::Member(paths)),
        None => Ok(Access::Unreachable(
            "the area's `SIGN_IN` is not a string literal",
        )),
    }
}
