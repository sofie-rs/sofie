use std::fmt::Write as _;
use std::path::PathBuf;
use std::ptr;

use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use sofie_engine::TranslationError;
use sofie_model::naming::{
    PathParameter, RouteSite, SOURCE_LANGUAGE, file_stem, key_constant, route_parameters,
    snake_case,
};

use crate::translations::AppLanguages;
use sofie_model::permissions::{module_permissions, permission_constant};
use sofie_model::{App, AreaDefinition, Module};

use crate::wiring::{HEADER, exposed_aggregates, path};

pub fn render_contract_gens(
    app: &App,
    languages: Option<&AppLanguages>,
) -> Result<Vec<(PathBuf, Option<String>)>, TranslationError> {
    let mut gens = Vec::new();
    for module in &app.modules {
        let Some(contract) = &module.contract else {
            continue;
        };
        let keys = languages
            .map(|languages| languages.module_keys(&module.krate.crate_name))
            .transpose()?;
        gens.push((contract.gen_file(), render_contract_gen(app, module, keys)));
    }
    Ok(gens)
}

fn render_contract_gen(app: &App, module: &Module, keys: Option<Vec<String>>) -> Option<String> {
    let mut items: Vec<String> = module
        .defined_areas
        .iter()
        .filter(|area| area.needs_generated_member_area())
        .map(|area| member_area(app, area).to_string())
        .collect();
    items.extend(exposed_dtos(module));
    items.extend(pruned_events(module));
    items.extend(route_builders(app, module).map(|builders| builders.to_string()));
    items.extend(permissions(app, module).map(|permissions| permissions.to_string()));
    items.extend(
        keys.filter(|_| has_source_language(module))
            .map(|keys| key_constants(&keys).to_string()),
    );
    if items.is_empty() {
        return None;
    }
    Some(format!("{HEADER}\n{}\n", items.join("\n\n")))
}

/// Members land on the area's first screen; signing up stays closed unless
/// the area implements `sofie_identity::SignUp`.
fn member_area(app: &App, area: &AreaDefinition) -> TokenStream {
    let home = app
        .member_home(area)
        .expect("the member-home rule refuses a member area without a home");
    let within_contract = path(&format!("crate::{}", area.type_path[1..].join("::")));
    let sign_up = area.opens_sign_up.then(|| {
        quote! {
            const SIGN_UP: Option<&'static str> =
                Some(<#within_contract as sofie_identity::SignUp>::PATH);
        }
    });
    quote! {
        impl sofie_identity::MemberArea for #within_contract {
            const HOME: &'static str = #home;
            #sign_up
        }
    }
}

/// Text rather than `quote!`, whose spacing inside an attribute rustfmt leaves alone.
fn exposed_dtos(module: &Module) -> impl Iterator<Item = String> {
    let code = &module.code;
    exposed_aggregates(module).map(|aggregate| {
        let mut fields = String::new();
        for (name, ty) in aggregate
            .contract_fields(code)
            .expect("the exposed-field rule refuses a type a contract cannot name")
        {
            let _ = write!(fields, " pub {name}: {ty},");
        }
        // A soft-deleting record tells its mirrors when it was deleted.
        let deleted = if aggregate.soft_deletes() {
            " pub deleted_at: Option<sofie::core::Instant>,"
        } else {
            ""
        };
        format!(
            "#[derive(sofie::serde::Deserialize, sofie::serde::Serialize)]\n\
                 #[serde(crate = \"sofie::serde\")]\n\
                 pub struct {name} {{ pub id: sofie::db::Uuid,{fields} }}\n\n\
                 #[derive(sofie::Event)]\n\
                 pub struct {name}Changed {{ pub id: sofie::db::Uuid, pub version: i64,{fields}{deleted} }}",
            name = aggregate.name
        )
    })
}

/// In a module of their own, so a new aggregate needs no new export in the contract.
fn pruned_events(module: &Module) -> Option<String> {
    if module.code.aggregates.is_empty() {
        return None;
    }
    let events: Vec<String> = module
        .code
        .aggregates
        .iter()
        .map(|aggregate| {
            format!(
                "#[derive(sofie::Event)]\n\
                 pub struct {}Pruned {{ pub id: sofie::db::Uuid, pub version: i64 }}",
                aggregate.name
            )
        })
        .collect();
    Some(format!("pub mod pruned {{\n{}\n}}", events.join("\n\n")))
}

fn route_builders(app: &App, module: &Module) -> Option<TokenStream> {
    let sites: Vec<RouteSite> = app
        .route_sites()
        .into_iter()
        .filter(|site| ptr::eq(site.module, module))
        .collect();
    if sites.is_empty() {
        return None;
    }
    let mut areas: Vec<&str> = sites.iter().map(|site| site.area.area.as_str()).collect();
    areas.dedup();
    let modules = areas.into_iter().map(|area| {
        let builders = sites
            .iter()
            .filter(|site| site.area.area == area)
            .map(|site| route_builder(app, site));
        let area = format_ident!("{area}");
        quote! {
            pub mod #area {
                #(#builders)*
            }
        }
    });
    Some(quote! {
        pub mod routes {
            #(#modules)*
        }
    })
}

fn route_builder(app: &App, site: &RouteSite) -> TokenStream {
    let path = site
        .full_path(app)
        .expect("the area-prefix rule refuses a route whose prefix is unknown");
    let handler = format_ident!(
        "{}",
        site.route
            .handler
            .last()
            .expect("a route names its handler")
    );
    let method = format_ident!("{}", site.route.method.name());
    let parameters: Vec<RouteParameter> = route_parameters(app, site.module, &path)
        .into_iter()
        .map(|(name, parameter)| RouteParameter {
            name: parameter_ident(name),
            module: &site.module.krate.crate_name,
            parameter: parameter
                .expect("the route-name rule refuses a parameter no handler can read"),
        })
        .collect();
    let generics = parameters.iter().filter_map(RouteParameter::generic);
    let arguments = parameters.iter().map(RouteParameter::argument);
    let checks = parameters.iter().filter_map(RouteParameter::check);
    let segments = parameters.iter().map(RouteParameter::segment);
    let path = if parameters.is_empty() {
        quote!(#path.to_owned())
    } else {
        let template = format_template(&path);
        quote!(format!(#template, #(#segments),*))
    };
    quote! {
        #[must_use]
        pub fn #handler<#(#generics),*>(#(#arguments),*) -> sofie::http::RoutePath {
            #(#checks)*
            sofie::http::RoutePath::new(sofie::http::Method::#method, #path)
        }
    }
}

/// `{match}` for an aggregate `Match`: a keyword is written raw, and one that cannot be is suffixed.
fn parameter_ident(name: &str) -> Ident {
    if syn::parse_str::<Ident>(name).is_ok() {
        return format_ident!("{name}");
    }
    match name {
        "self" | "Self" | "super" | "crate" => format_ident!("{name}_"),
        _ => Ident::new_raw(name, proc_macro2::Span::call_site()),
    }
}

/// The contract cannot name a module's aggregate, so its id is a generic the build checks.
struct RouteParameter<'app> {
    name: Ident,
    module: &'app str,
    parameter: PathParameter<'app>,
}

impl RouteParameter<'_> {
    fn aggregate(&self) -> Option<(&str, &str)> {
        match &self.parameter {
            PathParameter::Aggregate(aggregate) => Some((&aggregate.name, self.module)),
            PathParameter::Reference { module, aggregate } => {
                Some((&aggregate.name, &module.krate.crate_name))
            }
            PathParameter::Value(_) | PathParameter::Rest => None,
        }
    }

    fn generic(&self) -> Option<TokenStream> {
        let (aggregate, _) = self.aggregate()?;
        let aggregate = format_ident!("{aggregate}");
        Some(quote!(#aggregate: sofie::db::Aggregate))
    }

    fn argument(&self) -> TokenStream {
        let name = &self.name;
        let ty = match &self.parameter {
            PathParameter::Aggregate(aggregate) | PathParameter::Reference { aggregate, .. } => {
                let aggregate = format_ident!("{}", aggregate.name);
                quote!(sofie::db::Id<#aggregate>)
            }
            PathParameter::Value(ty) if ty == "String" => quote!(&str),
            PathParameter::Value(ty) => {
                let ty: TokenStream = ty.parse().expect("a contract type is Rust");
                quote!(#ty)
            }
            PathParameter::Rest => quote!(&str),
        };
        quote!(#name: #ty)
    }

    fn check(&self) -> Option<TokenStream> {
        let (aggregate, module) = self.aggregate()?;
        let name = snake_case(aggregate);
        let aggregate = format_ident!("{aggregate}");
        Some(quote!(const { sofie::http::require_aggregate::<#aggregate>(#module, #name) };))
    }

    fn segment(&self) -> TokenStream {
        let name = &self.name;
        if let PathParameter::Rest = self.parameter {
            return quote!(sofie::http::path_rest(#name));
        }
        quote!(sofie::http::path_segment(&#name))
    }
}

fn permissions(app: &App, module: &Module) -> Option<TokenStream> {
    let permissions = module_permissions(module);
    if !app.has_permitted_areas() || permissions.is_empty() {
        return None;
    }
    let constants = permissions.iter().map(|permission| {
        let constant = format_ident!("{}", permission_constant(permission));
        quote! {
            pub const #constant: sofie::http::Permission = sofie::http::Permission::new(#permission);
        }
    });
    Some(quote! {
        pub mod permissions {
            #(#constants)*
        }
    })
}

fn has_source_language(module: &Module) -> bool {
    module
        .languages
        .iter()
        .any(|file| file_stem(file) == SOURCE_LANGUAGE)
}

/// A key removed from the source language file fails the build wherever Rust names it.
fn key_constants(keys: &[String]) -> TokenStream {
    let constants = keys.iter().map(|key| {
        let constant = format_ident!("{}", key_constant(key));
        quote!(pub const #constant: &str = #key;)
    });
    quote! {
        pub mod keys {
            #(#constants)*
        }
    }
}

fn format_template(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            if segment.starts_with('{') {
                "{}"
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use sofie_model::Method;
    use sofie_model::testing::{
        aggregate, app, area, define_area, enumeration, field, module, route,
    };

    use super::render_contract_gens;
    use crate::rustfmt::format_rust;

    fn formatted(files: &[(PathBuf, Option<String>)]) -> String {
        format_rust(files[0].1.as_deref().unwrap(), Path::new("generated.rs")).unwrap()
    }

    fn generated(app: &sofie_model::App) -> String {
        formatted(&render_contract_gens(app, None).unwrap())
    }

    #[test]
    fn writes_route_builders_and_member_areas_the_contract_leaves_out() {
        let mut order = module("order", "shop/modules/order");
        let mut orders = route(Method::Get, "/orders", "queries::list_orders", 4);
        orders.screen = Some("orders".to_owned());
        let detail = route(Method::Get, "/orders/{order}", "queries::show_order", 5);
        order
            .areas
            .push(area(&order, "member", vec![detail, orders]));
        order.code.aggregates.push(aggregate(
            "Order",
            Vec::new(),
            "shop/modules/order/src/order/mod.rs",
        ));
        define_area(&mut order, "member");
        let mut generated = app("shop", vec![order]);
        generated.modules[0].defined_areas[0].implements_member_area = false;
        let mut written = generated.clone();
        written.modules[0].defined_areas[0].implements_member_area = true;

        let files = render_contract_gens(&generated, None).unwrap();
        let text = formatted(&files);

        assert_eq!(
            files[0].0.to_str(),
            Some("shop/modules/order/contract/src/gen/mod.rs")
        );
        assert_eq!(
            text,
            "// @generated by `sofie gen`; do not edit.\n\n\
             impl sofie_identity::MemberArea for crate::area::Member {\n    \
             const HOME: &'static str = \"/account/orders\";\n}\n\n\
             pub mod pruned {\n    \
             #[derive(sofie::Event)]\n    \
             pub struct OrderPruned {\n        \
             pub id: sofie::db::Uuid,\n        \
             pub version: i64,\n    }\n}\n\n\
             pub mod routes {\n    pub mod member {\n        \
             #[must_use]\n        pub fn show_order<Order: sofie::db::Aggregate>(\n            \
             order: sofie::db::Id<Order>,\n        ) -> sofie::http::RoutePath {\n            \
             const { sofie::http::require_aggregate::<Order>(\"order\", \"order\") };\n            \
             sofie::http::RoutePath::new(\n                sofie::http::Method::GET,\n                \
             format!(\"/account/orders/{}\", sofie::http::path_segment(&order)),\n            )\n        }\n        \
             #[must_use]\n        pub fn list_orders() -> sofie::http::RoutePath {\n            \
             sofie::http::RoutePath::new(sofie::http::Method::GET, \"/account/orders\".to_owned())\n        }\n    }\n}\n"
        );
        written.modules[0].areas.clear();
        written.modules[0].code.aggregates.clear();
        assert_eq!(render_contract_gens(&written, None).unwrap()[0].1, None);
    }

    /// `{match}` names the aggregate `Match`; as a bare parameter it was the keyword and rustfmt refused the contract.
    #[test]
    fn a_path_parameter_named_by_a_keyword_is_a_raw_identifier() {
        let mut league = module("league", "shop/modules/league");
        let cancel = route(
            Method::Post,
            "/matches/{match}/cancel",
            "actions::cancel_match",
            4,
        );
        league.areas.push(area(&league, "member", vec![cancel]));
        league.code.aggregates.push(aggregate(
            "Match",
            Vec::new(),
            "shop/modules/league/src/r#match/mod.rs",
        ));
        define_area(&mut league, "member");

        let text = generated(&app("shop", vec![league]));

        assert!(text.contains("r#match: sofie::db::Id<Match>"), "{text}");
        assert!(
            text.contains("sofie::http::path_segment(&r#match)"),
            "{text}"
        );
    }

    #[test]
    fn a_builder_takes_a_parent_id_a_field_value_and_the_rest_of_a_path_typed() {
        let mut order = module("order", "shop/modules/order");
        let lines = route(
            Method::Get,
            "/accounts/{account}/codes/{code}/{*rest}",
            "queries::lines",
            4,
        );
        order.areas.push(area(&order, "member", vec![lines]));
        order.code.aggregates.push(aggregate(
            "OrderLine",
            vec![
                field("account", "Id<Account>", 2),
                field("code", "String", 3),
            ],
            "shop/modules/order/src/order_line/mod.rs",
        ));
        define_area(&mut order, "member");
        let mut customer = module("customer", "shop/modules/customer");
        customer.code.aggregates.push(aggregate(
            "Account",
            Vec::new(),
            "shop/modules/customer/src/account/mod.rs",
        ));
        let mut typed = app("shop", vec![order, customer]);
        typed.modules[0].defined_areas[0].implements_member_area = true;

        let text = generated(&typed);

        assert!(
            text.contains(
                "pub fn lines<Account: sofie::db::Aggregate>(\n            \
                 account: sofie::db::Id<Account>,\n            code: &str,\n            \
                 rest: &str,\n        ) -> sofie::http::RoutePath {\n            \
                 const { sofie::http::require_aggregate::<Account>(\"customer\", \"account\") };"
            ),
            "{text}"
        );
        assert!(
            text.contains(
                "sofie::http::path_segment(&account),\n                    \
                 sofie::http::path_segment(&code),\n                    \
                 sofie::http::path_rest(rest)"
            ),
            "{text}"
        );
    }

    #[test]
    fn a_generated_member_area_opens_sign_up_the_area_declares() {
        let mut order = module("order", "shop/modules/order");
        let mut orders = route(Method::Get, "/orders", "queries::list_orders", 4);
        orders.screen = Some("orders".to_owned());
        order.areas.push(area(&order, "member", vec![orders]));
        define_area(&mut order, "member");
        let mut opened = app("shop", vec![order]);
        let definition = &mut opened.modules[0].defined_areas[0];
        definition.implements_member_area = false;
        definition.opens_sign_up = true;

        let text = generated(&opened);

        assert!(
            text.contains(
                "impl sofie_identity::MemberArea for crate::area::Member {\n    \
                 const HOME: &'static str = \"/account/orders\";\n    \
                 const SIGN_UP: Option<&'static str> =\n        \
                 Some(<crate::area::Member as sofie_identity::SignUp>::PATH);\n}"
            ),
            "{text}"
        );
    }

    #[test]
    fn writes_every_permission_once_an_area_admits_by_permission() {
        let mut order = module("order", "shop/modules/order");
        let cancel = route(
            Method::Post,
            "/orders/{order}/cancel",
            "actions::cancel_order",
            4,
        );
        order.areas.push(area(&order, "console", vec![cancel]));
        order.code.aggregates.push(aggregate(
            "Order",
            Vec::new(),
            "shop/modules/order/src/order/mod.rs",
        ));
        define_area(&mut order, "console");
        let mut permitted_app = app("shop", vec![order]);
        let owned = generated(&permitted_app);
        permitted_app.modules[0].defined_areas[0].principal = vec!["Permitted".to_owned()];

        let text = generated(&permitted_app);

        let expected = "pub mod permissions {\n    pub const ORDER_CANCEL: sofie::http::Permission =\n        \
                        sofie::http::Permission::new(\"order.order.*.cancel\");\n";
        assert!(text.contains(expected), "{text}");
        assert!(!owned.contains("permissions"), "{owned}");
    }

    #[test]
    fn writes_a_dto_of_the_fields_an_aggregate_exposes() {
        let mut order = module("order", "shop/modules/order");
        order.contract = define_contract(&order);
        order
            .code
            .enums
            .push(enumeration("OrderStatus", &[("Pending", &[])]));
        let mut placed = aggregate(
            "Order",
            vec![
                field("account", "Id<Account>", 3),
                field("status", "OrderStatus", 3),
                field("total", "Money", 3),
                field("note", "Option<String>", 3),
                field("secret_code", "String", 3),
            ],
            "shop/modules/order/src/order/mod.rs",
        );
        placed.exposed = ["account", "status", "total", "note"]
            .map(str::to_owned)
            .to_vec();
        order.code.aggregates.push(placed);

        let files = render_contract_gens(&app("shop", vec![order]), None).unwrap();
        let text = formatted(&files);

        assert_eq!(
            text,
            "// @generated by `sofie gen`; do not edit.\n\n\
             #[derive(sofie::serde::Deserialize, sofie::serde::Serialize)]\n\
             #[serde(crate = \"sofie::serde\")]\n\
             pub struct Order {\n    \
             pub id: sofie::db::Uuid,\n    \
             pub account: sofie::db::Uuid,\n    \
             pub status: String,\n    \
             pub total: sofie::values::Money,\n    \
             pub note: Option<String>,\n}\n\n\
             #[derive(sofie::Event)]\n\
             pub struct OrderChanged {\n    \
             pub id: sofie::db::Uuid,\n    \
             pub version: i64,\n    \
             pub account: sofie::db::Uuid,\n    \
             pub status: String,\n    \
             pub total: sofie::values::Money,\n    \
             pub note: Option<String>,\n}\n\n\
             pub mod pruned {\n    \
             #[derive(sofie::Event)]\n    \
             pub struct OrderPruned {\n        \
             pub id: sofie::db::Uuid,\n        \
             pub version: i64,\n    }\n}\n"
        );
    }

    #[test]
    fn writes_the_pruned_event_of_an_aggregate_that_exposes_nothing() {
        let mut order = module("order", "shop/modules/order");
        order.contract = define_contract(&order);
        order.code.aggregates.push(aggregate(
            "Audit",
            vec![field("note", "String", 3)],
            "shop/modules/order/src/audit/mod.rs",
        ));

        let files = render_contract_gens(&app("shop", vec![order]), None).unwrap();
        let text = formatted(&files);

        assert!(
            text.contains(
                "pub mod pruned {\n    #[derive(sofie::Event)]\n    pub struct AuditPruned {\n        \
                 pub id: sofie::db::Uuid,\n        pub version: i64,\n    }\n}"
            ),
            "{text}"
        );
        assert!(!text.contains("AuditChanged"), "{text}");
    }

    #[test]
    fn writes_the_module_s_own_translation_keys_as_constants() {
        let directory = sofie_model::testing::TemporaryDirectory::new("contract-keys");
        let english = directory.write(
            "lang/en.toml",
            "[orders]\ncount = \"{count} orders\"\n\n[label]\ntotal = \"Sum\"\n",
        );
        let mut order = module("order", "shop/modules/order");
        order.contract = define_contract(&order);
        order.languages = vec![english];
        let shop = app("shop", vec![order]);
        let languages = crate::translations::AppLanguages::read(&shop).unwrap();

        let files = render_contract_gens(&shop, languages.as_ref()).unwrap();
        let text = formatted(&files);

        assert!(
            text.ends_with(
                "pub mod keys {\n    pub const LABEL_TOTAL: &str = \"label.total\";\n    \
                 pub const ORDERS_COUNT: &str = \"orders.count\";\n}\n"
            ),
            "{text}"
        );
    }

    fn define_contract(order: &sofie_model::Module) -> Option<sofie_model::Crate> {
        let mut defined = order.clone();
        define_area(&mut defined, "member");
        defined.contract
    }
}
