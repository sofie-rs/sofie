use sofie_model::naming::RouteSite;
use sofie_model::{App, ModelError, Modifier, Route, ServedRoute, Then};

const HEADERS: [&str; 9] = [
    "AREA",
    "METHOD",
    "PATH",
    "NAME",
    "HANDLER",
    "PERMISSION",
    "FEATURE",
    "OWNER SCOPE",
    "MODIFIERS",
];

pub(crate) const GENERATOR_PATHS: &str = "Route paths in `routes` and `explain` are full, with \
     the area's prefix; generators such as `sofie new route` take a path relative to the area's \
     prefix: `/orders/{order}` for `/account/orders/{order}` in an area under `/account`.";

/// The framework packages' routes are part of the table, so failing to read them fails it.
pub fn route_table(app: &App) -> Result<String, ModelError> {
    let mut sites = app.route_sites();
    sites.sort_by(|left, right| {
        (&left.area.area, &left.route.path, left.route.method).cmp(&(
            &right.area.area,
            &right.route.path,
            right.route.method,
        ))
    });
    let mut rows: Vec<[String; 9]> = sites.iter().map(|site| row(app, site)).collect();
    let package_routes = app.package_routes();
    let packages = package_routes
        .identity()?
        .into_iter()
        .chain(package_routes.other_packages());
    rows.extend(packages.map(|served| package_row(app, &served)));
    Ok(format!(
        "{}\n{GENERATOR_PATHS}\n",
        text_table(HEADERS, &rows)
    ))
}

fn package_row(app: &App, served: &ServedRoute) -> [String; 9] {
    [
        served.area.clone(),
        served.route.method.clone(),
        served.path.clone(),
        served.name.clone(),
        format!("{}::{}", served.package(), served.route.mount),
        served.route.permission.clone().unwrap_or_default(),
        String::new(),
        owner_scope(app, &served.area).to_owned(),
        String::new(),
    ]
}

fn row(app: &App, site: &RouteSite) -> [String; 9] {
    let route = site.route;
    [
        site.area.area.clone(),
        route.method.name().to_owned(),
        shown_path(app, site),
        site.name(),
        handler_path(site),
        site.permission(app).unwrap_or_default(),
        feature(site).unwrap_or_default(),
        owner_scope(app, &site.area.area).to_owned(),
        modifiers(route).join(" "),
    ]
}

pub(crate) fn feature(site: &RouteSite) -> Option<String> {
    let declaration = site.module.route_feature(site.route)?;
    Some(
        declaration
            .name
            .clone()
            .unwrap_or_else(|| declaration.constant.join("::")),
    )
}

pub(crate) fn handler_path(site: &RouteSite) -> String {
    format!(
        "{}::{}",
        site.module.krate.crate_name,
        site.route.handler.join("::")
    )
}

pub(crate) fn owner_scope(app: &App, area: &str) -> &'static str {
    match app.area(area) {
        Some(known) if known.is_public() || known.is_permitted() => "any",
        Some(_) => "principal's own",
        None => "unknown area",
    }
}

pub(crate) fn modifiers(route: &Route) -> Vec<String> {
    let mut written = Vec::new();
    written.extend(
        route
            .screen
            .as_ref()
            .map(|screen| format!("screen({screen})")),
    );
    written.extend(
        route
            .webhook
            .as_ref()
            .map(|verifier| format!("webhook verify({verifier})")),
    );
    written.extend(
        route
            .modifiers
            .iter()
            .filter_map(|modifier| match modifier {
                Modifier::Throttle(rate) => Some(format!("throttle({rate})")),
                Modifier::Signed => Some("signed".to_owned()),
                Modifier::Idempotent => Some("idempotent".to_owned()),
                Modifier::BodyLimit(bytes) => Some(format!("body_limit({bytes})")),
                Modifier::Live(fragment) => Some(format!("live({fragment})")),
                Modifier::Cached(time) => Some(format!("cached({time})")),
                Modifier::Queued => Some("queued".to_owned()),
                Modifier::Entitled(entitlement) => Some(format!("entitled({entitlement})")),
                Modifier::Document(media) => Some(format!("document({media})")),
                Modifier::FileLimit(bytes) => Some(format!("file_limit({bytes})")),
                Modifier::Feature(_) => None,
            }),
    );
    written.extend(route.then.as_ref().map(|then| match then {
        Then::Screen { screen, fragment } => format!("then_screen({screen}#{fragment})"),
        Then::Redirect(path) => format!("then_redirect({path})"),
        Then::Follow => "then_follow".to_owned(),
        Then::RedirectOrFollow(path) => format!("then_redirect_or_follow({path})"),
    }));
    written
}

/// Its own path when its area's prefix is not a literal.
pub(crate) fn shown_path(app: &App, site: &RouteSite) -> String {
    site.full_path(app)
        .unwrap_or_else(|| site.route.path.clone())
}

pub(crate) fn text_table<const COLUMNS: usize>(
    headers: [&str; COLUMNS],
    rows: &[[String; COLUMNS]],
) -> String {
    let mut widths = headers.map(str::len);
    for row in rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let mut table = line(&headers.map(str::to_owned), &widths);
    for row in rows {
        table.push_str(&line(row, &widths));
    }
    table
}

fn line(cells: &[String], widths: &[usize]) -> String {
    let padded: Vec<String> = cells
        .iter()
        .zip(widths)
        .map(|(cell, width)| format!("{cell:<width$}"))
        .collect();
    let mut text = padded.join("  ").trim_end().to_owned();
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use sofie_model::testing::{app, area, define_area, module, route};
    use sofie_model::{
        FeatureDeclaration, Identity, ListedRoute, Method, Modifier, RouteMount, RoutedPackage,
    };

    use super::{GENERATOR_PATHS, route_table};

    #[test]
    fn shows_the_permission_a_permitted_area_needs_and_the_flag_a_route_hides_behind() {
        let mut order = module("order", "shop/modules/order");
        let mut cancel = route(
            Method::Post,
            "/orders/{order}/cancel",
            "actions::cancel_order",
            4,
        );
        cancel
            .modifiers
            .push(Modifier::Feature("crate::module::SPLIT".to_owned()));
        order.areas.push(area(&order, "console", vec![cancel]));
        define_area(&mut order, "console");
        order.defined_areas[0].principal = vec!["Permitted".to_owned()];
        order.code.features.declarations = vec![FeatureDeclaration {
            constant: vec!["module".to_owned(), "SPLIT".to_owned()],
            name: Some("order.split".to_owned()),
            added: Some("2026-09-01".to_owned()),
            retire_after_days: 30,
            variants: false,
            file: PathBuf::from("shop/modules/order/src/module.rs"),
            line: 5,
        }];

        let table = route_table(&app("shop", vec![order])).unwrap();

        let row: Vec<&str> = table.lines().nth(1).unwrap().split_whitespace().collect();
        assert_eq!(
            row,
            [
                "console",
                "POST",
                "/account/orders/{order}/cancel",
                "order.console.cancel_order",
                "order::actions::cancel_order",
                "order.order.*.cancel",
                "order.split",
                "any"
            ],
            "{table}"
        );
        assert!(
            table.ends_with(&format!("\n\n{GENERATOR_PATHS}\n")),
            "{table}"
        );
    }

    #[test]
    fn lists_the_identity_routes_the_app_mounts_in_its_areas() {
        let mut order = module("order", "shop/modules/order");
        define_area(&mut order, "console");
        order.defined_areas[0].type_path = vec!["order_contract".to_owned(), "Console".to_owned()];
        let mut shop = app("shop", vec![order]);
        shop.identity = Some(Identity {
            routes: vec![ListedRoute {
                mount: "permitted_routes".to_owned(),
                method: "GET".to_owned(),
                path: "/roles".to_owned(),
                handler: "list_roles".to_owned(),
                permission: Some("sofie_identity.role.*.read".to_owned()),
            }],
            mounts: vec![RouteMount {
                mount: "permitted_routes".to_owned(),
                area_type: "Console".to_owned(),
            }],
            ..Identity::default()
        });

        let table = route_table(&shop).unwrap();

        let row: Vec<&str> = table.lines().nth(1).unwrap().split_whitespace().collect();
        assert_eq!(
            row,
            [
                "console",
                "GET",
                "/account/roles",
                "sofie_identity.console.list_roles",
                "sofie_identity::permitted_routes",
                "sofie_identity.role.*.read",
                "principal's",
                "own"
            ],
            "{table}"
        );
    }

    #[test]
    fn lists_the_routes_of_the_packages_the_app_mounts() {
        let mut order = module("order", "shop/modules/order");
        define_area(&mut order, "staff");
        order.defined_areas[0].type_path = vec!["order_contract".to_owned(), "Staff".to_owned()];
        let mut shop = app("shop", vec![order]);
        let listed = |mount: &str, path: &str, handler: &str| ListedRoute {
            mount: mount.to_owned(),
            method: "GET".to_owned(),
            path: path.to_owned(),
            handler: handler.to_owned(),
            permission: None,
        };
        let mounted = |mount: &str, area_type: &str| RouteMount {
            mount: mount.to_owned(),
            area_type: area_type.to_owned(),
        };
        shop.packages = vec![
            RoutedPackage {
                crate_name: "sofie_content".to_owned(),
                routes: vec![
                    listed("routes", "/sitemap.xml", "sitemap"),
                    listed("admin_routes", "/pages", "list_pages"),
                ],
                mounts: vec![mounted("routes", "Web")],
            },
            RoutedPackage {
                crate_name: "sofie_tenancy".to_owned(),
                routes: vec![listed("organization_routes", "/members", "list_members")],
                mounts: vec![mounted("organization_routes", "Staff")],
            },
        ];

        let table = route_table(&shop).unwrap();

        let rows: Vec<Vec<&str>> = table
            .lines()
            .skip(1)
            .take(2)
            .map(|line| line.split_whitespace().collect())
            .collect();
        assert_eq!(
            rows,
            [
                vec![
                    "web",
                    "GET",
                    "/sitemap.xml",
                    "sofie_content.web.sitemap",
                    "sofie_content::routes",
                    "any"
                ],
                vec![
                    "staff",
                    "GET",
                    "/account/members",
                    "sofie_tenancy.staff.list_members",
                    "sofie_tenancy::organization_routes",
                    "principal's",
                    "own"
                ],
            ],
            "{table}"
        );
        assert!(!table.contains("list_pages"), "{table}");
    }

    #[test]
    fn identity_routes_it_cannot_read_fail_the_table() {
        let mut order = module("order", "shop/modules/order");
        define_area(&mut order, "console");
        let mut shop = app("shop", vec![order]);
        shop.identity = Some(Identity {
            routes: vec![ListedRoute {
                mount: "routes".to_owned(),
                method: "GET".to_owned(),
                path: "/profile".to_owned(),
                handler: "profile".to_owned(),
                permission: None,
            }],
            ..Identity::default()
        });

        let error = route_table(&shop).unwrap_err();

        assert!(error.message.contains("cannot read the file"), "{error}");
    }
}
