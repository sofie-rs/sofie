use std::fmt::Write as _;

use sofie_model::naming::{RouteSite, bound_aggregate, path_parameters, screen_key, snake_case};
use sofie_model::{Aggregate, KnownArea, Method, ReadModel};

use super::facts::{Fact, ModuleFacts};
use super::link_facts;
use super::requests::{Requests, caller, granted_principal, principal};

pub(super) fn route(requests: &Requests, site: &RouteSite, facts: &mut ModuleFacts) {
    let name = site.name();
    let Some(area) = requests.app.area(&site.area.area) else {
        return;
    };
    let Some(request) = requests.request(site, &[]) else {
        facts.uncover(
            &name,
            format!("{name}: the contract exports no route builders; run `sofie gen`"),
        );
        return;
    };
    let handler = handler(site);
    if area.is_permitted() {
        permission(requests, site, &area, &request, facts);
        return;
    }
    if !area.is_public() {
        facts.facts.push(Fact {
            subject: site.name(),
            text: format!("route {name} refuses a caller without a principal"),
            test: format!("{handler}_refuses_a_caller_without_a_principal"),
            body: format!("app.request(&{request}).accept_json().await.assert_status(401);"),
            on_app: true,
        });
        another_owner(requests, site, &area, facts);
    }
    valid_call(requests, site, &area, facts);
    flagged_off(requests, site, &area, &request, facts);
    if let Some(screen) = &site.route.screen {
        screen_renders(requests, site, &area, screen, facts);
        link_facts::row_links(requests, site, &area, screen, facts);
    }
}

fn permission(
    requests: &Requests,
    site: &RouteSite,
    area: &KnownArea,
    request: &str,
    facts: &mut ModuleFacts,
) {
    let Some(permission) = site.permission(requests.app) else {
        return;
    };
    let handler = handler(site);
    facts.facts.push(Fact {
        subject: site.name(),
        text: format!("route {} needs {permission}", site.name()),
        test: format!("{handler}_refuses_a_principal_without_its_permission"),
        body: format!(
            "let principal = sofie_testing::TestPrincipal::<{}>::fresh(&app);\napp.as_(&principal).request(&{request}).accept_json().await.assert_status(403);",
            area.type_path
        ),
        on_app: true,
    });
    if granted_principal(requests.app, area, "member", &[]).is_some() {
        valid_call(requests, site, area, facts);
        return;
    }
    let name = site.name();
    facts.uncover(
        &name,
        format!("{name}: a call holding {permission} succeeds, which needs a grant made by hand"),
    );
}

fn another_owner(requests: &Requests, site: &RouteSite, area: &KnownArea, facts: &mut ModuleFacts) {
    let parameters = path_parameters(&site.route.path);
    let [parameter] = parameters.as_slice() else {
        return;
    };
    let owned = bound_aggregate(requests.module, parameter).is_some_and(Aggregate::is_owned);
    let (Some(member), Some(other)) = (
        principal(requests.app, area, "member"),
        principal(requests.app, area, "other"),
    ) else {
        return;
    };
    let made = requests.make_parameter(site, parameter, "other", "record");
    let (true, Some(made), Some(request)) = (owned, made, requests.request(site, &["record"]))
    else {
        return;
    };
    let handler = handler(site);
    facts.facts.push(Fact {
        subject: site.name(),
        text: format!("route {} hides another owner's {parameter}", site.name()),
        test: format!("{handler}_hides_another_owner_s_{parameter}"),
        body: format!(
            "{member}{other}{made}app.as_(&member).request(&{request}).accept_json().await.assert_status(404);"
        ),
        on_app: true,
    });
}

fn valid_call(requests: &Requests, site: &RouteSite, area: &KnownArea, facts: &mut ModuleFacts) {
    let name = site.name();
    let Some((statements, request)) = requests.prepared(site, area) else {
        facts.uncover(
            &name,
            format!("{name}: no record to call it on, as nothing makes one"),
        );
        return;
    };
    let handler = handler(site);
    let called = format!("{request}{}.await", requests.body_of(site));
    let shown = answered_model(requests, site)
        .map(|model| shown_fields(requests, site, model))
        .unwrap_or_default();
    let checked = if shown.is_empty() {
        format!("{called}.assert_ok();")
    } else {
        format!("let shown = {called};\nshown.assert_ok();\nlet shown = shown.json();\n{shown}")
    };
    facts.facts.push(Fact {
        subject: site.name(),
        text: format!("route {name} succeeds"),
        test: format!("{handler}_succeeds"),
        body: format!("{statements}{checked}"),
        on_app: true,
    });
}

fn answered_model<'module>(
    requests: &Requests<'module>,
    site: &RouteSite,
) -> Option<&'module ReadModel> {
    if site.route.method != Method::Get {
        return None;
    }
    let code = &requests.module.code;
    let output = code.function(&site.route.handler)?.output.as_ref()?;
    code.read_model(&output.without_result().name)
}

/// The fields the read model answers, and its `id` the record the path loaded.
fn shown_fields(requests: &Requests, site: &RouteSite, model: &ReadModel) -> String {
    let mut checks = String::new();
    let loaded = path_parameters(&site.route.path)
        .into_iter()
        .position(|parameter| {
            bound_aggregate(requests.module, parameter).is_some_and(|aggregate| {
                model
                    .from
                    .as_deref()
                    .and_then(|from| from.rsplit("::").next())
                    == Some(aggregate.name.as_str())
            })
        });
    if let Some(index) = loaded.filter(|_| model.fields.iter().any(|field| field.name == "id")) {
        let _ = writeln!(
            checks,
            "assert_eq!(shown[\"id\"], record_{index}.to_string());"
        );
    }
    let names: Vec<String> = model
        .fields
        .iter()
        .filter(|field| field.name != "id")
        .map(|field| format!("{:?}", field.name))
        .collect();
    if !names.is_empty() {
        let _ = write!(
            checks,
            "for field in [{}] {{\n    assert!(shown.get(field).is_some(), \"{{field}} missing from {{shown}}\");\n}}",
            names.join(", ")
        );
    }
    checks
}

fn flagged_off(
    requests: &Requests,
    site: &RouteSite,
    area: &KnownArea,
    request: &str,
    facts: &mut ModuleFacts,
) {
    let Some(flag) = requests
        .module
        .route_feature(site.route)
        .and_then(|declaration| declaration.name.as_ref())
    else {
        return;
    };
    let member = principal(requests.app, area, "member").unwrap_or_default();
    let caller = caller(area);
    let handler = handler(site);
    facts.facts.push(Fact {
        subject: site.name(),
        text: format!("route {} is not found while {flag} is off", site.name()),
        test: format!("{handler}_is_not_found_while_its_flag_is_off"),
        body: format!(
            "app.flags().set({flag:?}, sofie::flags::Rule::off()).await.unwrap();\n{member}{caller}.request(&{request}).accept_json().await.assert_status(404);"
        ),
        on_app: true,
    });
}

fn screen_renders(
    requests: &Requests,
    site: &RouteSite,
    area: &KnownArea,
    screen: &str,
    facts: &mut ModuleFacts,
) {
    let Some((mut statements, request)) = requests.prepared(site, area) else {
        return;
    };
    // A screen loading its record from the path shows that one; a list is shown one of each.
    let lists = path_parameters(&site.route.path).is_empty() && !area.is_public();
    let aggregates = &requests.module.code.aggregates;
    for (index, aggregate) in aggregates.iter().enumerate().filter(|_| lists) {
        if let Some(creating) = requests.creating_route(&aggregate.name, Some(&site.area.area)) {
            let variable = format!("_shown_{index}");
            statements.push_str(
                &requests
                    .create_through(&creating, "member", &variable)
                    .unwrap_or_default(),
            );
        }
    }
    let key = screen_key(requests.crate_name(), &site.area.area, screen);
    let fields = text_fields_written(requests, site, screen);
    let mut body = statements;
    if !fields.is_empty() {
        let _ = writeln!(body, "let shown = {request}.accept_json().await.json();");
    }
    let _ = write!(
        body,
        "let page = {request}.header(\"accept\", \"text/html\").await;\npage.assert_ok();\nassert!(page.body.starts_with(\"<!doctype html>\"), \"{{}}\", page.body);"
    );
    if !fields.is_empty() {
        let listed: Vec<String> = fields.iter().map(|field| format!("{field:?}")).collect();
        let _ = write!(
            body,
            "\nfor field in [{}] {{\n    let value = shown[field].as_str().unwrap_or_else(|| panic!(\"{{field}} is not text in {{shown}}\"));\n    let mut written = String::new();\n    sofie::engine::push_escaped(&mut written, value);\n    assert!(page.body.contains(&written), \"{{field}} `{{written}}` missing from {{}}\", page.body);\n}}",
            listed.join(", ")
        );
    }
    facts.facts.push(Fact {
        subject: site.name(),
        text: format!("screen {key} renders"),
        test: format!("{}_screen_renders_with_fixtures", snake_case(screen)),
        body,
        on_app: true,
    });
}

/// Types a screen writes as their JSON text; money, dates and other formatted values are not.
const TEXT_TYPES: [&str; 3] = ["String", "Email", "Id"];

/// The answered read model's fields the screen writes bare, as `{data.<field>}` outside an
/// attribute expression, whose value is text the page shows unchanged but escaped.
fn text_fields_written(requests: &Requests, site: &RouteSite, screen: &str) -> Vec<String> {
    let Some(model) = answered_model(requests, site) else {
        return Vec::new();
    };
    let file = site.area.screen_file(screen);
    if !site.area.screens.contains(&file) {
        return Vec::new();
    }
    let Ok(read) = requests.screens.screen(&file) else {
        return Vec::new();
    };
    let enums = &requests.module.code.enums;
    model
        .fields
        .iter()
        .filter(|field| {
            TEXT_TYPES.contains(&field.ty.name.as_str())
                || enums.iter().any(|declared| declared.name == field.ty.name)
        })
        .filter(|field| writes_bare(&read.source, &field.name))
        .map(|field| field.name.clone())
        .collect()
}

fn writes_bare(source: &str, field: &str) -> bool {
    let written = format!("{{data.{field}}}");
    source
        .match_indices(&written)
        .any(|(at, _)| !source[..at].ends_with('='))
}

pub(super) fn handler<'site>(site: &'site RouteSite) -> &'site str {
    site.route.handler.last().map_or("", String::as_str)
}
