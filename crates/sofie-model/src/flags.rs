use std::path::{Path, PathBuf};

use syn::{Expr, Item, ItemConst, ItemFn, Lit};

use crate::syntax::{is_attribute, line_of, segments, string_literal};
use crate::{MenuEntry, Modifier, Module, Route};

const DEFAULT_RETIRE_AFTER_DAYS: u32 = 90;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FeatureCode {
    pub declarations: Vec<FeatureDeclaration>,
    pub gates: Vec<FeatureGate>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeatureDeclaration {
    pub constant: Vec<String>,
    pub name: Option<String>,
    pub added: Option<String>,
    pub retire_after_days: u32,
    pub variants: bool,
    pub file: PathBuf,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeatureGate {
    pub function: Vec<String>,
    pub feature: Vec<String>,
    pub file: PathBuf,
    pub line: usize,
}

impl FeatureCode {
    pub(crate) fn collect(&mut self, item: &Item, file: &Path, module_path: &[String]) {
        match item {
            Item::Const(constant) => {
                self.declarations
                    .extend(declaration_of(constant, file, module_path));
            }
            Item::Fn(function) => self.gates.extend(gate_of(function, file, module_path)),
            _ => {}
        }
    }

    /// `sofie gen` does not resolve imports, so the crate's one constant of that name is taken.
    #[must_use]
    pub fn declaration_of(&self, gate: &FeatureGate) -> Option<&FeatureDeclaration> {
        self.declared_once(gate.feature.last()?)
    }

    #[must_use]
    pub fn gate_on(&self, function: &[String]) -> Option<&FeatureDeclaration> {
        let gate = self.gates.iter().find(|gate| gate.function == function)?;
        self.declaration_of(gate)
    }

    pub(crate) fn declaration_written(&self, written: &str) -> Option<&FeatureDeclaration> {
        self.declared_once(written.rsplit("::").next()?.trim())
    }

    fn declared_once(&self, constant: &str) -> Option<&FeatureDeclaration> {
        let mut found = self.declarations.iter().filter(|declaration| {
            declaration.constant.last().map(String::as_str) == Some(constant)
        });
        let first = found.next()?;
        found.next().is_none().then_some(first)
    }
}

impl Module {
    #[must_use]
    pub fn route_feature(&self, route: &Route) -> Option<&FeatureDeclaration> {
        let features = &self.code.features;
        route
            .modifiers
            .iter()
            .find_map(|modifier| match modifier {
                Modifier::Feature(written) => features.declaration_written(written),
                _ => None,
            })
            .or_else(|| features.gate_on(&route.handler))
    }

    #[must_use]
    pub fn menu_feature(&self, entry: &MenuEntry) -> Option<&FeatureDeclaration> {
        self.code
            .features
            .declaration_written(entry.feature.as_deref()?)
    }
}

fn declaration_of(
    constant: &ItemConst,
    file: &Path,
    module_path: &[String],
) -> Option<FeatureDeclaration> {
    let syn::Type::Path(ty) = constant.ty.as_ref() else {
        return None;
    };
    let kind = ty.path.segments.last()?.ident.to_string();
    if kind != "Feature" && kind != "Variants" {
        return None;
    }
    let mut declaration = FeatureDeclaration {
        constant: [module_path, &[constant.ident.to_string()]].concat(),
        name: None,
        added: None,
        retire_after_days: DEFAULT_RETIRE_AFTER_DAYS,
        variants: kind == "Variants",
        file: file.to_path_buf(),
        line: line_of(&constant.ident),
    };
    read_builder(&constant.expr, &mut declaration);
    Some(declaration)
}

fn read_builder(expression: &Expr, declaration: &mut FeatureDeclaration) {
    match expression {
        Expr::MethodCall(call) => {
            if call.method == "retire_after_days"
                && let Some(Expr::Lit(literal)) = call.args.first()
                && let Lit::Int(days) = &literal.lit
            {
                declaration.retire_after_days =
                    days.base10_parse().unwrap_or(declaration.retire_after_days);
            }
            if call.method == "added" {
                declaration.added = call.args.first().and_then(string_literal);
            }
            read_builder(&call.receiver, declaration);
        }
        Expr::Call(call) => declaration.name = call.args.first().and_then(string_literal),
        _ => {}
    }
}

fn gate_of(function: &ItemFn, file: &Path, module_path: &[String]) -> Option<FeatureGate> {
    let attribute = function
        .attrs
        .iter()
        .find(|attribute| is_attribute(attribute, "feature"))?;
    let feature = attribute
        .parse_args::<syn::Path>()
        .map(|path| segments(&path))
        .unwrap_or_default();
    Some(FeatureGate {
        function: [module_path, &[function.sig.ident.to_string()]].concat(),
        feature,
        file: file.to_path_buf(),
        line: line_of(attribute),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collected(source: &str) -> FeatureCode {
        let file: syn::File = syn::parse_str(source).unwrap();
        let mut code = FeatureCode::default();
        for item in &file.items {
            code.collect(item, Path::new("module.rs"), &["module".to_owned()]);
        }
        code
    }

    #[test]
    fn reads_declared_flags_with_their_age() {
        let code = collected(
            r#"
            pub const SPLIT: Feature = Feature::new("order.split_payments").added("2026-09-01").retire_after_days(30);
            pub const LAYOUT: Variants<Layout> = sofie::flags::Variants::new("order.checkout_layout");
            pub const OTHER: u32 = 3;
            "#,
        );
        let names: Vec<_> = code
            .declarations
            .iter()
            .map(|d| {
                (
                    d.name.clone(),
                    d.added.clone(),
                    d.retire_after_days,
                    d.variants,
                )
            })
            .collect();
        assert_eq!(
            names,
            [
                (
                    Some("order.split_payments".to_owned()),
                    Some("2026-09-01".to_owned()),
                    30,
                    false
                ),
                (
                    Some("order.checkout_layout".to_owned()),
                    None,
                    DEFAULT_RETIRE_AFTER_DAYS,
                    true
                ),
            ]
        );
        assert_eq!(code.declarations[0].constant, ["module", "SPLIT"]);
    }

    #[test]
    fn reads_gated_functions_and_finds_their_flag() {
        let code = collected(
            r#"
            pub const SPLIT: Feature = Feature::new("order.split_payments");
            #[on]
            #[feature(crate::module::SPLIT)]
            pub async fn audit(event: OrderPlaced) -> Result<()> { Ok(()) }
            pub async fn plain() -> Result<()> { Ok(()) }
            "#,
        );
        assert_eq!(code.gates.len(), 1);
        let gate = &code.gates[0];
        assert_eq!(gate.function, ["module", "audit"]);
        assert_eq!(
            code.declaration_of(gate).unwrap().constant,
            ["module", "SPLIT"]
        );
    }
}
