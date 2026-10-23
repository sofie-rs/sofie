use std::path::{Path, PathBuf};

use syn::{Expr, Item, ItemConst};

use crate::syntax::{last_name, line_of, path_ends_with, string_literal};
use crate::{App, Module};

/// What a plan may hold, each held while it allows more than none.
const KINDS: [&str; 3] = ["Entitlement", "Limit", "Quota"];

/// `const EXPORT: Entitlement = Entitlement::new("project.export");`, or a `Limit` or `Quota`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntitlementDeclaration {
    pub constant: Vec<String>,
    pub name: String,
    pub file: PathBuf,
    pub line: usize,
}

pub(crate) fn entitlement_of(
    item: &Item,
    file: &Path,
    module_path: &[String],
) -> Option<EntitlementDeclaration> {
    let Item::Const(constant) = item else {
        return None;
    };
    let kind = declared_kind(constant)?;
    let Expr::Call(call) = constant.expr.as_ref() else {
        return None;
    };
    let Expr::Path(function) = call.func.as_ref() else {
        return None;
    };
    if !path_ends_with(&function.path, &[kind.as_str(), "new"]) {
        return None;
    }
    Some(EntitlementDeclaration {
        constant: [module_path, &[constant.ident.to_string()]].concat(),
        name: call.args.first().and_then(string_literal)?,
        file: file.to_path_buf(),
        line: line_of(&constant.ident),
    })
}

fn declared_kind(constant: &ItemConst) -> Option<String> {
    let syn::Type::Path(ty) = constant.ty.as_ref() else {
        return None;
    };
    last_name(&ty.path).filter(|kind| KINDS.contains(&kind.as_str()))
}

impl App {
    #[must_use]
    pub fn entitlements(&self) -> Entitlements<'_> {
        Entitlements {
            modules: &self.modules,
        }
    }
}

/// What the app's modules declare a plan may hold.
#[derive(Clone, Copy, Debug)]
pub struct Entitlements<'app> {
    modules: &'app [Module],
}

impl<'app> Entitlements<'app> {
    pub fn iter(self) -> impl Iterator<Item = &'app EntitlementDeclaration> {
        self.modules
            .iter()
            .flat_map(|module| &module.code.entitlements)
    }

    /// `.entitled(…)` as written: a constant, a limit's or quota's `.entitlement()`, or
    /// `Entitlement::new("…")`. A constant is matched by its name, as `sofie gen` does not
    /// resolve imports.
    #[must_use]
    pub fn written(self, written: &str) -> Option<&'app EntitlementDeclaration> {
        let mut expression = syn::parse_str::<Expr>(written).ok()?;
        while let Expr::MethodCall(call) = expression {
            if call.method != "entitlement" || !call.args.is_empty() {
                return None;
            }
            expression = *call.receiver;
        }
        match &expression {
            Expr::Path(constant) => {
                let constant = last_name(&constant.path)?;
                self.iter()
                    .find(|declared| declared.constant.last() == Some(&constant))
            }
            Expr::Call(call) => {
                let name = call.args.first().and_then(string_literal)?;
                self.iter().find(|declared| declared.name == name)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::entitlement_of;
    use crate::testing::{app, module};

    #[test]
    fn reads_entitlements_limits_and_quotas_named_by_a_literal() {
        let file: syn::File = syn::parse_str(
            r#"
            pub const EXPORT: Entitlement = Entitlement::new("project.export");
            pub const PROJECTS: sofie::billing::Limit = Limit::new("project.projects");
            pub const CALLS: Quota = Quota::new("project.api_calls", Period::Month);
            pub const NAMED: Entitlement = Entitlement::new(NAME);
            pub const PRO: Plan = Plan::new("pro");
            "#,
        )
        .unwrap();

        let declared: Vec<(String, String)> = file
            .items
            .iter()
            .filter_map(|item| entitlement_of(item, Path::new("plans.rs"), &["plans".to_owned()]))
            .map(|declaration| (declaration.constant.join("::"), declaration.name))
            .collect();

        let pair = |constant: &str, name: &str| (constant.to_owned(), name.to_owned());
        assert_eq!(
            declared,
            [
                pair("plans::EXPORT", "project.export"),
                pair("plans::PROJECTS", "project.projects"),
                pair("plans::CALLS", "project.api_calls"),
            ]
        );
    }

    #[test]
    fn finds_what_a_route_s_entitlement_names_however_it_is_written() {
        let file: syn::File = syn::parse_str(
            r#"
            pub const EXPORT: Entitlement = Entitlement::new("project.export");
            pub const PROJECTS: Limit = Limit::new("project.projects");
            "#,
        )
        .unwrap();
        let mut project = module("project", "/shop/modules/project");
        project.code.entitlements = file
            .items
            .iter()
            .filter_map(|item| entitlement_of(item, Path::new("plans.rs"), &["plans".to_owned()]))
            .collect();
        let shop = app("/shop", vec![project]);
        let found = |written: &str| {
            shop.entitlements()
                .written(written)
                .map(|declared| declared.name.clone())
        };

        assert_eq!(
            found("crate::plans::EXPORT").as_deref(),
            Some("project.export")
        );
        assert_eq!(
            found("PROJECTS.entitlement()").as_deref(),
            Some("project.projects")
        );
        assert_eq!(
            found("Entitlement::new(\"project.export\")").as_deref(),
            Some("project.export")
        );
        assert_eq!(found("plans::PRO"), None);
        assert_eq!(found("Entitlement::new(\"project.import\")"), None);
    }
}
