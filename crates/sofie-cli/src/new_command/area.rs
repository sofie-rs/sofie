use std::path::Path;

use sofie_model::naming::pascal_case;
use sofie_model::{App, Crate};

use super::account_screen::new_tenancy;
use super::edits::module_file;
use super::route::{NewRoute, file_names, new_route};
use super::{Edits, checked_name, checked_type_name, find_module};
use crate::gen_command::GenError;

/// The variants of `sofie::http::Access`, each fixed by the principal the area is written with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AreaAccess {
    Public,
    Owned,
    Permitted,
    Organization,
}

impl AreaAccess {
    pub const ALL: [Self; 4] = [
        Self::Public,
        Self::Owned,
        Self::Permitted,
        Self::Organization,
    ];

    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|access| access.name() == name)
    }

    /// Its name on the command line, and the crate and type of the area's principal.
    fn parts(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Public => ("public", "sofie::http", "Visitor"),
            Self::Owned => ("owned", "sofie_identity", "Account"),
            Self::Permitted => ("permitted", "sofie_identity", "Permitted"),
            Self::Organization => ("organization", TENANCY, "Organization"),
        }
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        self.parts().0
    }
}

/// The area in the module's contract, and its first page, which a member area lands on.
pub(super) fn new_area(
    edits: &mut Edits,
    app: &App,
    module: &str,
    name: &str,
    access: AreaAccess,
    member: Option<&str>,
) -> Result<(), GenError> {
    let name = checked_name(name)?;
    let type_name = pascal_case(name);
    checked_type_name(&type_name)?;
    if app.area(name).is_some() {
        return Err(GenError::Usage(format!(
            "the app has an area `{name}` already; choose another name"
        )));
    }
    let defining = find_module(app, module)?;
    let contract = defining.contract.as_ref().ok_or_else(|| {
        GenError::Usage(format!(
            "module `{module}` has no contract to define the area in; add `contract/` as \
             `sofie new module` writes it"
        ))
    })?;
    refuse_taken_prefix(app, name)?;
    refuse_taken_type_name(app, contract, name, &type_name)?;
    let sign_in = sign_in(app, name, access, member)?;
    write_definition(edits, contract, name, &type_name, access, &sign_in)?;
    depend_on_principal(edits, app, contract, access)?;
    if access == AreaAccess::Owned {
        let file = module_file(contract, "area");
        let file = file.strip_prefix(&app.root).unwrap_or(&file);
        edits.note(sign_up_hint(file, &type_name, name));
    }
    let reloaded = sofie_model::load(&app.root).map_err(GenError::Model)?;
    if let Some(member) = member {
        new_tenancy(edits, &reloaded, name, member)?;
    }
    new_route(
        edits,
        &reloaded,
        &NewRoute {
            module: module.to_owned(),
            area: name.to_owned(),
            method: "get".to_owned(),
            path: HOME.to_owned(),
            handler: format!("{name}_home"),
            screen: Some("home".to_owned()),
            fields: None,
            read_model: None,
            title: None,
            back: None,
        },
    )
}

/// A permitted area is reached by a grant, so only an owned area is told how to open sign-up.
fn sign_up_hint(file: &Path, type_name: &str, name: &str) -> String {
    format!(
        "sign-up to area `{name}` stays closed until the app implements \
         `sofie_identity::SignUp` for it; to open it, add to {}:\n\
         impl sofie_identity::SignUp for {type_name} {{\n    \
         const PATH: &'static str = \"{}/sign-up\";\n}}",
        file.display(),
        prefix(name)
    )
}

/// A member area lands on its first parameterless screen; `/` would serve the prefix with a
/// trailing slash only.
const HOME: &str = "/home";

fn prefix(name: &str) -> String {
    format!("/{}", name.replace('_', "-"))
}

/// An organization area is entered from its member area's list of organizations, which is
/// where a visitor without one is sent.
fn sign_in(
    app: &App,
    name: &str,
    access: AreaAccess,
    member: Option<&str>,
) -> Result<String, GenError> {
    match (access, member) {
        (AreaAccess::Public, None) => Ok("/".to_owned()),
        (AreaAccess::Owned | AreaAccess::Permitted, None) => {
            Ok(format!("{}/sign-in", prefix(name)))
        }
        (AreaAccess::Organization, Some(member)) => Ok(format!(
            "{}/organizations",
            account_area_prefix(app, member)?
        )),
        (AreaAccess::Organization, None) => Err(GenError::Usage(format!(
            "an organization area is entered from a member area; name it with `--member`, \
             as `sofie new area MODULE {name} organization --member member`"
        ))),
        (_, Some(_)) => Err(GenError::Usage(format!(
            "`--member` names the member area of an organization area; a {} area takes none",
            access.name()
        ))),
    }
}

fn account_area_prefix(app: &App, member: &str) -> Result<String, GenError> {
    app.areas()
        .into_iter()
        .filter(|area| area.name == member)
        .find_map(|area| {
            let definition = area.definition?;
            let owned = definition
                .principal
                .last()
                .is_some_and(|principal| principal == "Account");
            owned.then(|| definition.prefix.clone()).flatten()
        })
        .ok_or_else(|| {
            GenError::Usage(format!(
                "the app defines no area `{member}` of sofie-identity's `Account` to list and \
                 enter organizations from; make one with `sofie new area MODULE {member} owned`"
            ))
        })
}

fn refuse_taken_prefix(app: &App, name: &str) -> Result<(), GenError> {
    let prefix = prefix(name);
    let areas = app.areas();
    let Some(taken) = areas
        .iter()
        .find(|area| area.prefix() == Some(prefix.as_str()))
    else {
        return Ok(());
    };
    Err(GenError::Usage(format!(
        "area `{}` serves under the prefix `{prefix}` already, which area `{name}` would take; \
         choose another name",
        taken.name
    )))
}

/// Every principal is refused, not only this area's: a later area of another access imports
/// its principal into the same file.
fn refuse_taken_type_name(
    app: &App,
    contract: &Crate,
    name: &str,
    type_name: &str,
) -> Result<(), GenError> {
    let file = module_file(contract, "area");
    let in_file = if file.is_file() {
        file_names(&file)?
    } else {
        Vec::new()
    };
    let principals = AreaAccess::ALL.map(|access| access.parts().2);
    if !in_file
        .iter()
        .map(String::as_str)
        .chain(principals)
        .any(|taken| taken == type_name)
    {
        return Ok(());
    }
    Err(GenError::Usage(format!(
        "`{type_name}`, the type area `{name}` would define, is a name {} declares or imports \
         already; choose another name",
        file.strip_prefix(&app.root).unwrap_or(&file).display()
    )))
}

fn write_definition(
    edits: &mut Edits,
    contract: &Crate,
    name: &str,
    type_name: &str,
    access: AreaAccess,
    sign_in: &str,
) -> Result<(), GenError> {
    let (_, principal_crate, principal) = access.parts();
    let prefix = prefix(name);
    let area = format!(
        "pub struct {type_name};\n\nimpl Area for {type_name} {{\n    \
         const NAME: &'static str = \"{name}\";\n    const PREFIX: &'static str = \"{prefix}\";\n    \
         const SIGN_IN: &'static str = \"{sign_in}\";\n    type Principal = {principal};\n}}\n"
    );
    let principal_import = format!("use {principal_crate}");
    let mut imports = vec![
        ("use sofie::http", "Area"),
        (principal_import.as_str(), principal),
    ];
    let mut definition = vec![area];
    if access == AreaAccess::Organization {
        definition.push(format!(
            "\nimpl OrganizationArea for {type_name} {{\n    \
             const HOME: &'static str = \"{prefix}{HOME}\";\n}}\n"
        ));
        imports.push((principal_import.as_str(), "OrganizationArea"));
    }
    let definition = definition.concat();
    let file = module_file(contract, "area");
    edits.append(&file, &imports, &definition)?;
    edits.declare(contract, &["area"])
}

/// `sofie gen` then adds sofie-identity's module, sign-in routes and `MemberArea` for an
/// area members sign in to; an organization area's principal comes from sofie-tenancy, which
/// `new_tenancy` makes the app depend on.
fn depend_on_principal(
    edits: &mut Edits,
    app: &App,
    contract: &Crate,
    access: AreaAccess,
) -> Result<(), GenError> {
    edits.depend_on_framework(contract)?;
    match access {
        AreaAccess::Public => return Ok(()),
        AreaAccess::Owned => edits.depend_on_workspace(contract, IDENTITY)?,
        AreaAccess::Permitted => edits.enable_feature(contract, IDENTITY, "permissions")?,
        AreaAccess::Organization => {
            edits.depend_on_workspace(contract, "sofie-tenancy")?;
            return edits.depend(&app.app, contract);
        }
    }
    edits.depend_on_workspace(&app.app, IDENTITY)?;
    edits.depend(&app.app, contract)
}

const IDENTITY: &str = "sofie-identity";
const TENANCY: &str = "sofie_tenancy";

#[cfg(test)]
mod tests {
    use super::AreaAccess;

    #[test]
    fn each_name_reads_back_as_its_access() {
        for access in AreaAccess::ALL {
            assert_eq!(AreaAccess::parse(access.name()), Some(access));
        }
        assert_eq!(AreaAccess::parse("private"), None);
    }
}
