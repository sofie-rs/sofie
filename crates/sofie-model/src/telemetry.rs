use std::path::{Path, PathBuf};

use syn::punctuated::Punctuated;
use syn::visit::{self, Visit};
use syn::{Expr, Item, ItemStruct, Macro, Token};

use crate::code::derives;
use crate::syntax::{has_attribute, line_of, path_ends_with, segments};

/// `product_event!(order.placed, order.total)`: its name and the values it records, as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProductEvent {
    pub name: String,
    pub values: Vec<String>,
    pub file: PathBuf,
    pub line: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TelemetryCode {
    pub product_events: Vec<ProductEvent>,
    /// Functions marked `#[span]`, each in a `step` span named after its path.
    pub steps: Vec<Vec<String>>,
    /// `#[derive(Value)]` types marked `#[sensitive]`, which log only their mask.
    pub sensitive_values: Vec<String>,
    /// Aggregate fields marked `#[sensitive]`, as (aggregate, field), which log only `***`.
    pub sensitive_fields: Vec<(String, String)>,
}

impl TelemetryCode {
    pub(crate) fn collect(&mut self, item: &Item, file: &Path, module_path: &[String]) {
        let mut calls = ProductEventCalls {
            file,
            found: Vec::new(),
        };
        calls.visit_item(item);
        self.product_events.extend(calls.found);
        match item {
            Item::Fn(function) if has_attribute(&function.attrs, "span") => {
                self.steps
                    .push([module_path, &[function.sig.ident.to_string()]].concat());
            }
            Item::Struct(structure) => self.collect_sensitive(structure),
            _ => {}
        }
    }

    fn collect_sensitive(&mut self, structure: &ItemStruct) {
        let owner = structure.ident.to_string();
        if derives(&structure.attrs, "Value") && has_attribute(&structure.attrs, "sensitive") {
            self.sensitive_values.push(owner);
            return;
        }
        if !derives(&structure.attrs, "Aggregate") {
            return;
        }
        let fields = structure
            .fields
            .iter()
            .filter(|field| has_attribute(&field.attrs, "sensitive"))
            .filter_map(|field| field.ident.as_ref());
        for field in fields {
            self.sensitive_fields
                .push((owner.clone(), field.to_string()));
        }
    }
}

struct ProductEventCalls<'file> {
    file: &'file Path,
    found: Vec<ProductEvent>,
}

impl Visit<'_> for ProductEventCalls<'_> {
    fn visit_macro(&mut self, call: &Macro) {
        if !path_ends_with(&call.path, &["product_event"]) {
            visit::visit_macro(self, call);
            return;
        }
        let arguments = call
            .parse_body_with(Punctuated::<Expr, Token![,]>::parse_terminated)
            .unwrap_or_default();
        let mut written = arguments.iter().filter_map(dotted);
        let Some(name) = written.next() else { return };
        self.found.push(ProductEvent {
            name,
            values: written.collect(),
            file: self.file.to_path_buf(),
            line: line_of(&call.path),
        });
    }
}

fn dotted(expression: &Expr) -> Option<String> {
    match expression {
        Expr::Path(path) => Some(segments(&path.path).join(".")),
        Expr::Field(field) => {
            let member = match &field.member {
                syn::Member::Named(ident) => ident.to_string(),
                syn::Member::Unnamed(index) => index.index.to_string(),
            };
            Some(format!("{}.{member}", dotted(&field.base)?))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::TelemetryCode;

    #[test]
    fn reads_product_events_steps_and_sensitive_values_and_fields() {
        let file: syn::File = syn::parse_str(
            r"
            #[derive(Value)]
            #[sensitive]
            pub struct TaxNumber(String);

            #[derive(Value)]
            pub struct Note(String);

            #[derive(Aggregate)]
            pub struct Payer {
                meta: Meta,
                name: String,
                #[sensitive]
                iban: String,
            }

            #[span]
            async fn reserve_stock() {}

            pub async fn place_order(order: Order) {
                sofie::telemetry::product_event!(order.placed, order.total, count);
            }
            ",
        )
        .unwrap();
        let mut code = TelemetryCode::default();
        for item in &file.items {
            code.collect(item, Path::new("actions.rs"), &["actions".to_owned()]);
        }

        let events: Vec<(&str, &[String], usize)> = code
            .product_events
            .iter()
            .map(|event| (event.name.as_str(), event.values.as_slice(), event.line))
            .collect();
        assert_eq!(
            events,
            [(
                "order.placed",
                &["order.total".to_owned(), "count".to_owned()][..],
                21
            )]
        );
        assert_eq!(code.steps, [["actions", "reserve_stock"]]);
        assert_eq!(code.sensitive_values, ["TaxNumber"]);
        assert_eq!(
            code.sensitive_fields,
            [("Payer".to_owned(), "iban".to_owned())]
        );
    }
}
