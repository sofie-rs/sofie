//! A screen sends `null` for what a viewer's gates withhold, or whether a value holds anything
//! where only a condition the viewer sees tests it, so its answer's types say so. A read model's
//! schema is shared by every endpoint answering it, so one screen's gates loosen it for all.

use std::collections::BTreeMap;

use sofie_check::Screens;
use sofie_core::listing::ITEMS;
use sofie_engine::Step;
use sofie_engine::rendering::{Withholdable, withholdable};
use sofie_model::naming::RouteSite;

use super::{Answer, Api, Schema, Shape, VALUE_SHAPES, value_properties};

impl Api {
    /// After every endpoint is built: building one inserts its read models' schemas afresh.
    pub(super) fn loosen_withheld(&mut self, screens: &Screens, site: &RouteSite, index: usize) {
        let Answer::Data(mut shape) =
            std::mem::replace(&mut self.endpoints[index].output, Answer::Empty)
        else {
            return;
        };
        let mut loosening = Loosening {
            schemas: &mut self.schemas,
        };
        for template in rendered_templates(screens, site) {
            for withheld in withholdable(template) {
                loosening.path(&mut shape, &withheld.steps, &withheld);
            }
        }
        self.endpoints[index].output = Answer::Data(shape);
    }
}

/// The schemas an endpoint's answer refers to, loosened where a screen withholds.
struct Loosening<'a> {
    schemas: &'a mut BTreeMap<String, Schema>,
}

impl Loosening<'_> {
    fn path(&mut self, shape: &mut Shape, steps: &[Step], withheld: &Withholdable) {
        let Some((step, rest)) = steps.split_first() else {
            self.unnamed(shape, &withheld.named_fields);
            if withheld.nullable {
                loosen(shape, withheld.tested);
            }
            return;
        };
        match (step, shape) {
            (_, Shape::Optional(inner) | Shape::Tested(inner)) => {
                self.path(inner, steps, withheld);
            }
            (Step::Item, Shape::List(item)) => self.path(item, rest, withheld),
            (Step::Item, Shape::Named(name)) => {
                let name = name.clone();
                self.with_property(&name, ITEMS, |api, items| {
                    api.path(items, steps, withheld);
                });
            }
            (Step::Field(field), Shape::Named(name)) => {
                let name = name.clone();
                self.with_property(&name, field, |api, property| {
                    api.path(property, rest, withheld);
                });
            }
            _ => {}
        }
    }

    /// With no part named, a value object is sent whole or replaced whole.
    fn unnamed(&mut self, shape: &mut Shape, named: &[String]) {
        match shape {
            Shape::Optional(inner) | Shape::Tested(inner) => self.unnamed(inner, named),
            Shape::Named(name) => self.with_properties(&name.clone(), |_, properties| {
                loosen_properties(properties, named);
            }),
            Shape::Value(value) if !named.is_empty() => {
                *shape = self.withheld_value(value, named);
            }
            _ => {}
        }
    }

    /// A value object's schema is shared by inputs too, so its loosened parts go in a copy.
    fn withheld_value(&mut self, value: &'static str, named: &[String]) -> Shape {
        let name = format!("Withheld{value}");
        if !self.schemas.contains_key(&name) {
            let (_, parts) = VALUE_SHAPES
                .iter()
                .find(|(known, _)| *known == value)
                .expect("a value shape is named from VALUE_SHAPES");
            self.schemas
                .insert(name.clone(), Schema::Object(value_properties(parts)));
        }
        self.with_properties(&name, |_, properties| {
            loosen_properties(properties, named);
        });
        Shape::Named(name)
    }

    fn with_property(
        &mut self,
        name: &str,
        property_name: &str,
        change: impl FnOnce(&mut Self, &mut Shape),
    ) {
        self.with_properties(name, |api, properties| {
            if let Some(property) = properties
                .iter_mut()
                .find(|property| property.name == property_name)
            {
                change(api, &mut property.shape);
            }
        });
    }

    /// Taken out while `change` runs, so it may loosen other schemas.
    fn with_properties(
        &mut self,
        name: &str,
        change: impl FnOnce(&mut Self, &mut Vec<super::Property>),
    ) {
        let mut properties = match self.schemas.get_mut(name) {
            Some(Schema::Object(properties)) => std::mem::take(properties),
            _ => return,
        };
        change(self, &mut properties);
        if let Some(Schema::Object(held)) = self.schemas.get_mut(name) {
            *held = properties;
        }
    }
}

/// The route's screen and those its handler renders by name. `sofie check` reports a screen
/// that is missing or cannot be read or parsed.
fn rendered_templates<'s>(
    screens: &'s Screens,
    site: &RouteSite,
) -> impl Iterator<Item = &'s sofie_engine::Template> {
    let rendered = site
        .module
        .code
        .function(&site.route.handler)
        .into_iter()
        .flat_map(|function| &function.renders)
        .filter_map(|render| render.screen.as_ref());
    let files: Vec<_> = site
        .route
        .screen
        .iter()
        .chain(rendered)
        .map(|name| site.area.screen_file(name))
        .filter(|file| site.area.screens.contains(file))
        .collect();
    files.into_iter().filter_map(|file| {
        let screen = screens.screen(&file).as_ref().ok()?;
        screen.template.as_ref().ok()
    })
}

fn loosen_properties(properties: &mut [super::Property], named: &[String]) {
    for property in properties {
        if !named.contains(&property.name) {
            loosen(&mut property.shape, false);
        }
    }
}

fn loosen(shape: &mut Shape, tested: bool) {
    let inner = match std::mem::replace(shape, Shape::Committed) {
        Shape::Optional(inner) => *inner,
        other => other,
    };
    let inner = match inner {
        Shape::Tested(_) => inner,
        other if tested => Shape::Tested(Box::new(other)),
        other => other,
    };
    *shape = Shape::Optional(Box::new(inner));
}

#[cfg(test)]
mod tests {
    use std::fs;

    use sofie_model::testing::{app, area, field, function, module, route, type_name};
    use sofie_model::{FieldDeclaration, Method, ReadModel, Route, ScreenRender};

    use crate::api::typescript::typescript;

    const SCREEN: &str = "<h1>{data.title}</h1>{#if data.discount}<p>off</p>{/if}\
        {#each data.orders as order}<p>{order.id}</p>\
        {#if can(\"shop.order.cost.read\")}<p>{order.cost} {data.discount}</p>{/if}{/each}";

    fn read_model(name: &str, fields: Vec<FieldDeclaration>) -> ReadModel {
        ReadModel {
            name: name.to_owned(),
            from: None,
            fields,
            file: "shop/modules/order/src/queries.rs".into(),
            line: 1,
        }
    }

    /// Each route answers an `OrderPage` from a handler of its own, which renders the screens
    /// named beside it.
    fn typed(screen: &str, page: Vec<FieldDeclaration>, routes: Vec<(Route, &[&str])>) -> String {
        let directory = crate::local_app::private_directory("sofie-api-withheld").unwrap();
        let mut order = module("order", "shop/modules/order");
        let mut functions = Vec::new();
        for (route, renders) in &routes {
            let mut handler = function(
                &route.handler.join("::"),
                "shop/modules/order/src/queries.rs",
                9,
            );
            handler.output = Some(type_name("Result<OrderPage>"));
            handler.renders = renders
                .iter()
                .map(|name| ScreenRender {
                    screen: Some((*name).to_owned()),
                    line: 10,
                })
                .collect();
            functions.push(handler);
        }
        let routes = routes.into_iter().map(|(route, _)| route).collect();
        let mut web = area(&order, "web", routes);
        web.routes_file = directory.join("routes.rs");
        let file = web.screen_file("orders");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, screen).unwrap();
        web.screens = vec![file];
        order.areas.push(web);
        order.code.functions = functions;
        order.code.read_models = vec![
            read_model("OrderPage", page),
            read_model(
                "OrderRow",
                vec![field("id", "i64", 1), field("cost", "i64", 2)],
            ),
        ];
        let text = typescript(&app("shop", vec![order]));
        fs::remove_dir_all(&directory).unwrap();
        text
    }

    fn screen_route(path: &str, handler: &str) -> Route {
        let mut shown = route(Method::Get, path, handler, 3);
        shown.screen = Some("orders".to_owned());
        shown
    }

    fn order_page() -> Vec<FieldDeclaration> {
        vec![
            field("title", "String", 1),
            field("discount", "Option<i64>", 2),
            field("orders", "Vec<OrderRow>", 3),
            field("note", "String", 4),
        ]
    }

    fn assert_typed(text: &str, typed: &[&str]) {
        for typed in typed {
            assert!(text.contains(typed), "{typed} in {text}");
        }
    }

    #[test]
    fn a_path_a_screen_gates_is_typed_nullable_and_a_tested_one_boolean() {
        let route = screen_route("/orders", "queries::list_orders");

        let text = typed(SCREEN, order_page(), vec![(route, &[])]);

        assert_typed(
            &text,
            &[
                "  title: string;\n",
                "  discount: number | boolean | null;\n",
                "  note: string | null;\n",
                "  id: number;\n",
                "  cost: number | null;\n",
            ],
        );
    }

    #[test]
    fn a_later_endpoint_answering_the_same_read_model_keeps_it_loosened() {
        let shown = screen_route("/orders", "queries::list_orders");
        let plain = route(Method::Get, "/orders/plain", "queries::plain_orders", 4);

        let text = typed(SCREEN, order_page(), vec![(shown, &[]), (plain, &[])]);

        assert_typed(&text, &["  cost: number | null;\n"]);
    }

    #[test]
    fn a_screen_a_handler_renders_by_name_loosens_its_answer() {
        let chosen = route(Method::Get, "/orders", "queries::chosen_orders", 3);

        let text = typed(SCREEN, order_page(), vec![(chosen, &["orders"])]);

        assert_typed(&text, &["  cost: number | null;\n"]);
    }

    #[test]
    fn a_gated_field_of_a_page_models_items_is_typed_nullable() {
        let screen = "<h1>{data.title}</h1>{#each data.items as order}<p>{order.id}</p>\
            {#if can(\"shop.order.cost.read\")}<p>{order.cost}</p>{/if}{/each}";
        let page = vec![
            field("title", "String", 1),
            field("items", "Vec<OrderRow>", 2),
        ];
        let route = screen_route("/orders", "queries::list_orders");

        let text = typed(screen, page, vec![(route, &[])]);

        assert_typed(
            &text,
            &[
                "  title: string;\n",
                "  items: OrderRow[];\n",
                "  id: number;\n",
                "  cost: number | null;\n",
            ],
        );
    }

    #[test]
    fn a_value_objects_unnamed_parts_under_a_hidden_whole_read_are_typed_nullable() {
        let screen = "<h1>{data.total.currency}</h1>\
            {#if can(\"shop.order.cost.read\")}<p>{data.total}</p>{/if}";
        let page = vec![field("title", "String", 1), field("total", "Money", 2)];
        let route = screen_route("/orders", "queries::list_orders");

        let text = typed(screen, page, vec![(route, &[])]);

        assert_typed(
            &text,
            &[
                "  total: WithheldMoney;\n",
                "export interface WithheldMoney {\n  minor?: number | null;\n  \
                 amount?: string | null;\n  currency: string;\n}",
            ],
        );
    }
}
