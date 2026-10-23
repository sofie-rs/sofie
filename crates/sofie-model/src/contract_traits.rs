use std::path::{Path, PathBuf};

use syn::visit::Visit;
use syn::{FnArg, GenericParam, Item, ReceiverKind, ReturnType, Signature, TraitItem, Type};

use crate::syntax::{first_type_argument, has_attribute, line_of};

/// `sofie split` serves it from the app its module moves to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContractTrait {
    pub name: String,
    /// Within its contract crate.
    pub path: Vec<String>,
    pub methods: Vec<ContractMethod>,
    pub file: PathBuf,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContractMethod {
    pub name: String,
    /// Why a call of it cannot travel between apps.
    pub stays_local: Option<&'static str>,
}

impl ContractTrait {
    #[must_use]
    pub fn crosses_apps(&self) -> bool {
        self.methods
            .iter()
            .all(|method| method.stays_local.is_none())
    }
}

pub(crate) fn contract_trait(
    item: &Item,
    file: &Path,
    module_path: &[String],
) -> Option<ContractTrait> {
    let Item::Trait(contract) = item else {
        return None;
    };
    if !has_attribute(&contract.attrs, "contract") {
        return None;
    }
    let methods = contract
        .items
        .iter()
        .filter_map(|item| match item {
            TraitItem::Fn(function) => Some(ContractMethod {
                name: function.sig.ident.to_string(),
                stays_local: stays_local(&function.sig),
            }),
            _ => None,
        })
        .collect();
    Some(ContractTrait {
        name: contract.ident.to_string(),
        path: [module_path, &[contract.ident.to_string()]].concat(),
        methods,
        file: file.to_path_buf(),
        line: line_of(item),
    })
}

/// sofie-macros' `remote::method` applies the same rule when it writes the client and server.
fn stays_local(signature: &Signature) -> Option<&'static str> {
    if signature.asyncness.is_none() {
        return Some("it is not `async`");
    }
    if has_type_parameters(signature) {
        return Some("it is generic");
    }
    if !signature.inputs.iter().all(input_crosses) {
        return Some("it takes `&mut`, `self` by value or a type holding a reference");
    }
    if !answers_owned_result(&signature.output) {
        return Some("it does not answer `Result<…>` of an owned type");
    }
    None
}

fn has_type_parameters(signature: &Signature) -> bool {
    signature
        .generics
        .params
        .iter()
        .any(|parameter| !matches!(parameter, GenericParam::Lifetime(_)))
}

/// `&self`, an owned value or a shared reference to one, which the server reads back owned.
fn input_crosses(input: &FnArg) -> bool {
    match input {
        FnArg::Receiver(receiver) => matches!(receiver.kind, ReceiverKind::Reference(_, _, None)),
        FnArg::Typed(typed) => match typed.ty.as_ref() {
            Type::Reference(reference) => reference.mutability.is_none() && owned(&reference.elem),
            other => owned(other),
        },
    }
}

fn answers_owned_result(output: &ReturnType) -> bool {
    let ReturnType::Type(_, output) = output else {
        return false;
    };
    let Type::Path(path) = output.as_ref() else {
        return false;
    };
    path.path
        .segments
        .last()
        .is_some_and(|last| last.ident == "Result" && first_type_argument(last).is_some_and(owned))
}

fn owned(ty: &Type) -> bool {
    let mut borrows = Borrows(false);
    borrows.visit_type(ty);
    !borrows.0
}

/// A lifetime, a reference or `Self` anywhere in a type.
struct Borrows(bool);

impl Visit<'_> for Borrows {
    fn visit_lifetime(&mut self, _lifetime: &syn::Lifetime) {
        self.0 = true;
    }

    fn visit_type_reference(&mut self, _reference: &syn::TypeReference) {
        self.0 = true;
    }

    fn visit_path_segment(&mut self, segment: &syn::PathSegment) {
        self.0 |= segment.ident == "Self";
        syn::visit::visit_path_segment(self, segment);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(source: &str) -> ContractTrait {
        let item: Item = syn::parse_str(source).unwrap();
        contract_trait(
            &item,
            Path::new("contract/src/lib.rs"),
            &["orders".to_owned()],
        )
        .unwrap()
    }

    #[test]
    fn a_contract_crosses_apps_only_when_every_method_can() {
        let remote = read(
            "#[sofie::contract] pub trait Orders: Send + Sync {
                async fn total(&self, order: Uuid, names: &[String], note: &str) -> Result<Money>;
            }",
        );
        let local = read(
            "#[contract] pub trait Ledger {
                async fn rewrite(&self, entries: &mut Vec<u8>) -> Result<()>;
                fn now(&self) -> Result<i64>;
                async fn borrowed(&self) -> Result<&'static str>;
            }",
        );

        assert_eq!(remote.path, ["orders", "Orders"]);
        assert!(remote.crosses_apps());
        let reasons: Vec<Option<&str>> = local
            .methods
            .iter()
            .map(|method| method.stays_local)
            .collect();
        assert_eq!(
            reasons,
            [
                Some("it takes `&mut`, `self` by value or a type holding a reference"),
                Some("it is not `async`"),
                Some("it does not answer `Result<…>` of an owned type"),
            ]
        );
    }

    #[test]
    fn an_unmarked_trait_is_no_contract() {
        let item: Item = syn::parse_str("pub trait Plain {}").unwrap();

        assert!(contract_trait(&item, Path::new("lib.rs"), &[]).is_none());
    }
}
