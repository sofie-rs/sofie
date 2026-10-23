//! Resources and webhooks are left out: the first needs the compiled app's description, the second is called by a provider.

#[cfg(feature = "openapi")]
mod json;
#[cfg(feature = "openapi")]
pub(crate) mod openapi;
pub(crate) mod typescript;
mod values;
mod withheld;

use std::collections::BTreeMap;

use sofie_model::naming::{RouteSite, path_parameters};
use sofie_model::{App, FieldDeclaration, Method, Module, TypeName};

use crate::routes_command::shown_path;

pub(crate) use values::{Scalar, VALUE_SHAPES, ValueShape};

pub(crate) struct Endpoint {
    pub(crate) name: String,
    pub(crate) area: String,
    pub(crate) method: Method,
    pub(crate) path: String,
    pub(crate) parameters: Vec<String>,
    pub(crate) query: Option<QueryParameters>,
    pub(crate) summary: Option<String>,
    pub(crate) input: Option<String>,
    pub(crate) output: Answer,
}

/// Read from the query string as `Query<Q>` or `Listing<T, Q>`: each field one parameter.
pub(crate) struct QueryParameters {
    pub(crate) name: String,
    pub(crate) properties: Vec<Property>,
}

pub(crate) enum Answer {
    Empty,
    Data(Shape),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Shape {
    Scalar(Scalar),
    Value(&'static str),
    Named(String),
    List(Box<Shape>),
    Optional(Box<Shape>),
    /// The value, or whether it holds anything.
    Tested(Box<Shape>),
    Committed,
    Unknown(String),
}

impl Shape {
    fn mentions_value(&self, value: &str) -> bool {
        match self {
            Self::Value(name) => *name == value,
            Self::List(inner) | Self::Optional(inner) | Self::Tested(inner) => {
                inner.mentions_value(value)
            }
            Self::Scalar(_) | Self::Named(_) | Self::Committed | Self::Unknown(_) => false,
        }
    }
}

pub(crate) enum Schema {
    Object(Vec<Property>),
    Enum(Vec<String>),
}

#[derive(Clone)]
pub(crate) struct Property {
    pub(crate) name: String,
    pub(crate) shape: Shape,
    pub(crate) required: bool,
}

pub(crate) fn value_properties(shape: ValueShape) -> Vec<Property> {
    shape
        .iter()
        .map(|(name, scalar, required)| Property {
            name: (*name).to_owned(),
            shape: Shape::Scalar(*scalar),
            required: *required,
        })
        .collect()
}

pub(crate) struct Api {
    pub(crate) title: String,
    pub(crate) endpoints: Vec<Endpoint>,
    pub(crate) schemas: BTreeMap<String, Schema>,
}

impl Api {
    pub(crate) fn of(app: &App) -> Self {
        let mut api = Self {
            title: app.app.package.clone(),
            endpoints: Vec::new(),
            schemas: BTreeMap::new(),
        };
        let sites: Vec<RouteSite> = app
            .route_sites()
            .into_iter()
            .filter(|site| !site.route.is_resource() && site.route.webhook.is_none())
            .collect();
        for site in &sites {
            let endpoint = api.endpoint(app, site);
            api.endpoints.push(endpoint);
        }
        let screens = sofie_check::Screens::read(&app.modules);
        for (index, site) in sites.iter().enumerate() {
            api.loosen_withheld(&screens, site, index);
        }
        api
    }

    pub(crate) fn mentions_value(&self, value: &str) -> bool {
        let answered = self
            .endpoints
            .iter()
            .filter_map(|endpoint| match &endpoint.output {
                Answer::Data(shape) => Some(shape),
                Answer::Empty => None,
            });
        let held = self.schemas.values().flat_map(|schema| match schema {
            Schema::Object(properties) => {
                properties.iter().map(|property| &property.shape).collect()
            }
            Schema::Enum(_) => Vec::new(),
        });
        answered
            .chain(held)
            .any(|shape| shape.mentions_value(value))
    }

    fn endpoint(&mut self, app: &App, site: &RouteSite) -> Endpoint {
        let route = site.route;
        let path = shown_path(app, site);
        let function = site.module.code.function(&route.handler);
        let input = function
            .and_then(|function| site.module.code.input_of(function))
            .map(|input| {
                let name = input.name.clone();
                let properties = self.properties(site.module, &input.fields, Carried::Sent);
                self.schemas
                    .insert(name.clone(), Schema::Object(properties));
                name
            });
        let query = function
            .and_then(|function| site.module.code.query_of(function))
            .map(|input| {
                let properties = self.properties(site.module, &input.fields, Carried::Sent);
                self.schemas
                    .insert(input.name.clone(), Schema::Object(properties.clone()));
                QueryParameters {
                    name: input.name.clone(),
                    properties,
                }
            });
        let output = match function.and_then(|function| function.output.as_ref()) {
            None => Answer::Empty,
            Some(output) => self.answer(site.module, output.without_result()),
        };
        Endpoint {
            name: site.name(),
            area: site.area.area.clone(),
            method: route.method,
            parameters: path_parameters(&path)
                .into_iter()
                .map(str::to_owned)
                .collect(),
            path: path.replace("{*", "{"),
            query,
            summary: function.and_then(|function| function.doc.clone()),
            input,
            output,
        }
    }

    fn answer(&mut self, module: &Module, output: &TypeName) -> Answer {
        let aggregate = |ty: &TypeName| {
            module
                .code
                .aggregates
                .iter()
                .any(|aggregate| aggregate.name == ty.name)
        };
        match (output.name.as_str(), output.arguments.as_slice()) {
            ("()", []) => Answer::Empty,
            ("Screen", [data]) => Answer::Data(self.shape(module, data)),
            ("Vec", [item]) if aggregate(item) => {
                Answer::Data(Shape::List(Box::new(Shape::Committed)))
            }
            _ if aggregate(output) => Answer::Data(Shape::Committed),
            _ => Answer::Data(self.shape(module, output)),
        }
    }

    fn shape(&mut self, module: &Module, ty: &TypeName) -> Shape {
        let code = &module.code;
        match (ty.name.as_str(), ty.arguments.as_slice()) {
            ("Option", [inner]) => Shape::Optional(Box::new(self.shape(module, inner))),
            ("Vec", [item]) => Shape::List(Box::new(self.shape(module, item))),
            ("Id", [_]) => Shape::Scalar(Scalar::Uuid),
            (name, []) => {
                if let Some(scalar) = Scalar::of(name) {
                    return Shape::Scalar(scalar);
                }
                if let Some((value, _)) = VALUE_SHAPES.iter().find(|(value, _)| *value == name) {
                    return Shape::Value(value);
                }
                if let Some(declared) = code.enums.iter().find(|declared| declared.name == name) {
                    self.schemas.insert(
                        declared.name.clone(),
                        Schema::Enum(declared.stored_names.clone()),
                    );
                    return Shape::Named(declared.name.clone());
                }
                if let Some(model) = code.read_model(name) {
                    let properties = self.properties(module, &model.fields, Carried::Answered);
                    self.schemas
                        .insert(model.name.clone(), Schema::Object(properties));
                    return Shape::Named(model.name.clone());
                }
                if ty.record().is_some() {
                    return Shape::Scalar(Scalar::Uuid);
                }
                Shape::Unknown(ty.to_string())
            }
            _ => Shape::Unknown(ty.to_string()),
        }
    }

    fn properties(
        &mut self,
        module: &Module,
        fields: &[FieldDeclaration],
        carried: Carried,
    ) -> Vec<Property> {
        let mut properties = Vec::new();
        for field in fields {
            let shape = self.shape(module, &field.ty);
            let moves = module
                .code
                .enums
                .iter()
                .find(|declared| declared.name == field.ty.name && field.ty.arguments.is_empty())
                .filter(|declared| declared.has_transitions());
            properties.push(Property {
                name: field.name.clone(),
                required: carried == Carried::Answered || !matches!(shape, Shape::Optional(_)),
                shape: shape.clone(),
            });
            if moves.is_some() {
                properties.push(Property {
                    name: format!("{}_to", field.name),
                    shape: Shape::List(Box::new(shape)),
                    required: false,
                });
            }
        }
        properties
    }
}

/// An answer writes every field, `None` as `null`; a sender may leave an optional field out.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Carried {
    Sent,
    Answered,
}

#[cfg(test)]
mod tests {
    use sofie_model::testing::{app, area, enumeration, field, function, module, route, type_name};
    use sofie_model::{InputDeclaration, Method, ReadModel};

    pub(super) fn shop() -> sofie_model::App {
        let mut order = module("order", "shop/modules/order");
        let mut list = route(Method::Get, "/orders", "queries::list_orders", 3);
        list.screen = Some("orders".to_owned());
        order.areas.push(area(
            &order,
            "web",
            vec![
                list,
                route(
                    Method::Post,
                    "/orders/{order}/notes",
                    "actions::add_note",
                    4,
                ),
            ],
        ));
        let mut list_orders = function(
            "queries::list_orders",
            "shop/modules/order/src/queries.rs",
            9,
        );
        list_orders.arguments = vec![type_name("Listing<OrderRow, OrderSearch>")];
        list_orders.output = Some(type_name("Result<Vec<OrderRow>>"));
        list_orders.doc = Some("The latest orders.".to_owned());
        let mut add_note = function("actions::add_note", "shop/modules/order/src/actions.rs", 9);
        add_note.arguments = vec![type_name("Order"), type_name("AddNote")];
        add_note.output = Some(type_name("Result<Order>"));
        order.code.functions = vec![list_orders, add_note];
        order.code.aggregates = vec![sofie_model::testing::aggregate(
            "Order",
            Vec::new(),
            "shop/modules/order/src/order.rs",
        )];
        order.code.enums = vec![enumeration(
            "OrderStatus",
            &[("Pending", &["Paid"]), ("Paid", &[])],
        )];
        order.code.read_models = vec![ReadModel {
            name: "OrderRow".to_owned(),
            from: Some("Order".to_owned()),
            fields: vec![
                field("id", "Id<Order>", 1),
                field("status", "OrderStatus", 2),
                field("total", "Money", 3),
                field("note", "Option<String>", 4),
            ],
            file: "shop/modules/order/src/queries.rs".into(),
            line: 1,
        }];
        order.code.inputs = vec![
            InputDeclaration {
                name: "AddNote".to_owned(),
                path: vec!["actions".to_owned(), "AddNote".to_owned()],
                from: None,
                fields: vec![field("text", "String", 2), field("pinned", "bool", 3)],
                unconstrained: Vec::new(),
                file: "shop/modules/order/src/actions.rs".into(),
                line: 1,
            },
            InputDeclaration {
                name: "OrderSearch".to_owned(),
                path: vec!["queries".to_owned(), "OrderSearch".to_owned()],
                from: None,
                fields: vec![field("text", "String", 2), field("days", "Option<u32>", 3)],
                unconstrained: Vec::new(),
                file: "shop/modules/order/src/queries.rs".into(),
                line: 1,
            },
        ];
        app("shop", vec![order])
    }
}
