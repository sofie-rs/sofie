use sofie_model::naming::pascal_case;
use sofie_model::{App, Crate};

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
}

impl AreaAccess {
    pub const ALL: [Self; 3] = [Self::Public, Self::Owned, Self::Permitted];

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
        }
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        self.parts().0
    }

    fn signs_in(self) -> bool {
        self != Self::Public
    }
}

/// The area in the module's contract, and its first page, which a member area lands on.
pub(super) fn new_area(
    edits: &mut Edits,
    app: &App,
    module: &str,
    name: &str,
    access: AreaAccess,
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
    write_definition(edits, contract, name, &type_name, access)?;
    depend_on_principal(edits, app, contract, access)?;
    let reloaded = sofie_model::load(&app.root).map_err(GenError::Model)?;
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

/// A member area lands on its first parameterless screen; `/` would serve the prefix with a
/// trailing slash only.
const HOME: &str = "/home";

fn prefix(name: &str) -> String {
    format!("/{}", name.replace('_', "-"))
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
) -> Result<(), GenError> {
    let (_, principal_crate, principal) = access.parts();
    let prefix = prefix(name);
    let sign_in = if access.signs_in() {
        format!("{prefix}/sign-in")
    } else {
        "/".to_owned()
    };
    let definition = format!(
        "pub struct {type_name};\n\nimpl Area for {type_name} {{\n    \
         const NAME: &'static str = \"{name}\";\n    const PREFIX: &'static str = \"{prefix}\";\n    \
         const SIGN_IN: &'static str = \"{sign_in}\";\n    type Principal = {principal};\n}}\n"
    );
    let file = module_file(contract, "area");
    let imports = [
        ("use sofie::http", "Area"),
        (&format!("use {principal_crate}"), principal),
    ];
    edits.append(&file, &imports, &definition)?;
    edits.declare(contract, &["area"])
}

/// `sofie gen` then adds sofie-identity's module, sign-in routes and `MemberArea` for an
/// area members sign in to.
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
    }
    edits.depend_on_workspace(&app.app, IDENTITY)?;
    edits.depend(&app.app, contract)
}

const IDENTITY: &str = "sofie-identity";

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
