//! The wiring of a split (`[package.metadata.sofie]`): modules served to other
//! apps, and modules reached in another app through their contracts.

use proc_macro2::TokenStream;
use quote::quote;
use sofie_model::{App, Module, RemoteModule};

use super::{crossing_traits, exposed_aggregates, path};

pub(super) fn connections(app: &App) -> Vec<TokenStream> {
    app.split
        .remote
        .iter()
        .map(|module| {
            let name = &module.name;
            quote!(sofie::remote::connect(#name)?;)
        })
        .collect()
}

/// In place of the module's own implementations, which live in its app now.
pub(super) fn provisions(app: &App) -> Vec<TokenStream> {
    app.split
        .remote
        .iter()
        .filter_map(|module| {
            let name = &module.name;
            let contract = &module.contract.crate_name;
            let traits = crossing_traits(&module.traits).map(|contract_trait| {
                let contract_trait = path(&format!("{contract}::{}", contract_trait.path.join("::")));
                quote! {
                    sofie::db::provide::<dyn #contract_trait>(
                        db,
                        std::sync::Arc::new(sofie::remote::Remote::<dyn #contract_trait>::new(app.clone())),
                    );
                }
            });
            let lookups = remote_lookups(module).map(|dto| {
                let dto = path(&format!("{contract}::{dto}"));
                quote! {
                    sofie::db::provide::<dyn sofie::db::Lookup<#dto>>(
                        db,
                        std::sync::Arc::new(sofie::remote::RemoteLookup::<#dto>::new(app.clone())),
                    );
                }
            });
            let provided: Vec<TokenStream> = traits.chain(lookups).collect();
            (!provided.is_empty()).then(|| {
                quote! {
                    if let Some(app) = sofie::remote::app(#name) {
                        #(#provided)*
                    }
                }
            })
        })
        .collect()
}

/// This app's served modules' events published, and every remote module's feed pulled.
pub(super) fn feeds(app: &App, modules: &[&Module]) -> Option<TokenStream> {
    let published = modules
        .iter()
        .filter(|module| serves(app, module))
        .flat_map(|module| {
            let name = &module.krate.crate_name;
            module.events.iter().map(move |event| {
                let event = format!("{name}.{event}");
                quote!(.publish(#event))
            })
        });
    let pulled = app.split.remote.iter().map(|module| {
        let name = &module.name;
        quote!(.pull(sofie::remote::feed(#name)))
    });
    let parts: Vec<TokenStream> = published.chain(pulled).collect();
    (!parts.is_empty()).then(|| quote!(sofie::worker::Feeds::default() #(#parts)*))
}

pub(super) fn serving(app: &App, modules: &[&Module]) -> Option<TokenStream> {
    let served: Vec<&&Module> = modules
        .iter()
        .filter(|module| serves(app, module))
        .collect();
    if served.is_empty() {
        return None;
    }
    let parts = served.iter().flat_map(|module| {
        let contract = module
            .contract
            .as_ref()
            .map_or_else(String::new, |krate| krate.crate_name.clone());
        let traits = crossing_traits(&module.contract_traits).map(|contract_trait| {
            let contract_trait = path(&format!("{contract}::{}", contract_trait.path.join("::")));
            quote!(.contract::<dyn #contract_trait>())
        });
        let lookups = exposed_aggregates(module).map(|aggregate| {
            let dto = path(&format!("{contract}::{}", aggregate.name));
            quote!(.lookup::<#dto>())
        });
        traits.chain(lookups).collect::<Vec<_>>()
    });
    Some(quote! {
        sofie::remote::Served::default() #(#parts)* .feed().serve(&db);
    })
}

fn serves(app: &App, module: &Module) -> bool {
    app.split.serves.contains(&module.krate.crate_name)
}

/// A DTO is exported beside its `<Name>Changed` event, as `sofie gen` writes both.
fn remote_lookups(module: &RemoteModule) -> impl Iterator<Item = &String> {
    module.exports.iter().filter(|name| {
        let changed = format!("{name}Changed");
        module.exports.contains(&changed)
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use sofie_model::testing::{app, krate, module};
    use sofie_model::{ContractMethod, ContractTrait, RemoteModule, Split};

    use crate::wiring::tests::{formatted as wiring, has};

    fn orders_trait(stays_local: Option<&'static str>) -> ContractTrait {
        ContractTrait {
            name: "Orders".to_owned(),
            path: vec!["Orders".to_owned()],
            methods: vec![ContractMethod {
                name: "total".to_owned(),
                stays_local,
            }],
            file: PathBuf::from("contract/src/lib.rs"),
            line: 2,
        }
    }

    #[test]
    fn a_remote_module_is_connected_provided_and_pulled() {
        let mut shop = app("shop", Vec::new());
        shop.split = Split {
            serves: Vec::new(),
            remote: vec![RemoteModule {
                name: "order".to_owned(),
                contract: krate("order-contract", "shop/order/modules/order/contract"),
                defined_areas: Vec::new(),
                exports: vec!["Order".to_owned(), "OrderChanged".to_owned()],
                events: vec!["OrderChanged".to_owned()],
                traits: vec![orders_trait(None), {
                    let mut local = orders_trait(Some("it is not `async`"));
                    local.path = vec!["Ledger".to_owned()];
                    local
                }],
            }],
        };

        let wiring = wiring(&shop);

        assert!(
            has(&wiring, "sofie::remote::connect(\"order\")?; provide(&db);"),
            "{wiring}"
        );
        assert!(
            has(
                &wiring,
                "sofie::db::provide::<dyn order_contract::Orders>(db, std::sync::Arc::new(\
                 sofie::remote::Remote::<dyn order_contract::Orders>::new("
            ),
            "{wiring}"
        );
        assert!(
            has(
                &wiring,
                "sofie::remote::RemoteLookup::<order_contract::Order>::new"
            ),
            "{wiring}"
        );
        assert!(!wiring.contains("Ledger"), "{wiring}");
        assert!(
            has(&wiring, ".pull(sofie::remote::feed(\"order\"))"),
            "{wiring}"
        );
    }

    #[test]
    fn a_served_module_serves_its_contract_and_publishes_its_events() {
        let mut order = module("order", "shop/modules/order");
        order.contract = Some(krate("order-contract", "shop/modules/order/contract"));
        order.contract_traits = vec![orders_trait(None)];
        order.events = vec!["OrderPlaced".to_owned()];
        let mut split = app("order-app", vec![order]);
        split.split.serves = vec!["order".to_owned()];

        let wiring = wiring(&split);

        assert!(
            has(
                &wiring,
                "sofie::remote::Served::default()\
                 .contract::<dyn order_contract::Orders>().feed().serve(&db);"
            ),
            "{wiring}"
        );
        assert!(has(&wiring, ".publish(\"order.OrderPlaced\")"), "{wiring}");
    }
}
