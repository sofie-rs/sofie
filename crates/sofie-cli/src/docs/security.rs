use sofie_model::naming::RouteSite;
use sofie_model::{App, ConfigKey, KnownArea, Method, Modifier, Module};

use super::markdown::{code, codes, location, mermaid, mermaid_text, table};
use crate::env_example::{framework_keys, module_keys, remote_prefix, vendor_prefix};
use crate::routes_command::{feature, owner_scope, shown_path};
use crate::sorted;

/// As sofie-http reserves them; a test holds the two together.
const FRAMEWORK_ENDPOINTS: [(&str, &str); 4] = [
    ("/live", "answers once the process serves"),
    (
        "/ready",
        "names a dependency that is down, never why; telemetry outages answer 200 degraded",
    ),
    (
        "/metrics",
        "feature `metrics`: only with `Authorization: Bearer` and `SOFIE_METRICS_TOKEN`; absent without the token",
    ),
    (
        "/_sofie/telemetry",
        "feature `browser-telemetry`: 60 posts a minute per address, a kind, a listed name and a number only",
    ),
];

pub(super) fn security(app: &App) -> String {
    let mut text = String::from(
        "# Security surface\n\nWho reaches what, read from the routes, areas, gateways and \
         config. Fields logged only as their mask are listed in \
         [telemetry](telemetry.md#masked-fields).\n\n",
    );
    text.push_str(&mermaid("flowchart LR", &reach_lines(app)));
    text.push_str("\n## Routes\n\n");
    text.push_str(&routes(app));
    text.push_str("\n## Public endpoints\n\n");
    text.push_str(&public_endpoints(app));
    text.push_str("\n## Outgoing requests\n\n");
    text.push_str(&outgoing(app));
    text.push_str("\n## Secrets\n\nNever logged or printed; set them only in the environment.\n\n");
    text.push_str(&secrets(app));
    text.push_str("\n## Not checked here\n\n");
    text.push_str(&unchecked(app));
    text
}

fn access(area: Option<&KnownArea>) -> String {
    match area {
        None => "unknown area".to_owned(),
        Some(area) if area.is_public() => "anyone".to_owned(),
        Some(area) if area.is_permitted() => "a grant of the permission".to_owned(),
        Some(area) => {
            let principal = area
                .definition
                .and_then(|definition| definition.principal.last())
                .map_or("?", String::as_str);
            format!("signed in as {}", code(principal))
        }
    }
}

fn reach_lines(app: &App) -> Vec<String> {
    let sites = app.route_sites();
    let mut lines = Vec::new();
    for (index, area) in app.areas().iter().enumerate() {
        let routes = sites
            .iter()
            .filter(|site| site.area.area == area.name)
            .count();
        let identity = app.identity.is_some() && area.definition.is_none();
        if routes == 0 && !identity {
            continue;
        }
        let mut label = format!("{}: {routes} routes", area.name);
        if identity {
            label.push_str(" and sofie-identity's screens");
        }
        let who = access(Some(area)).replace('`', "");
        lines.push(format!(
            "who{index}([\"{}\"]) --> area{index}[\"{}\"]",
            mermaid_text(&who),
            mermaid_text(&label)
        ));
        lines.push(format!("area{index} --> app[[{}]]", app.app.crate_name));
    }
    for (index, vendor) in vendor_hosts(app).iter().enumerate() {
        lines.push(format!(
            "app --> vendor{index}([\"{}\"])",
            mermaid_text(&vendor.vendor)
        ));
    }
    lines
}

fn routes(app: &App) -> String {
    let rows: Vec<[String; 8]> = app
        .route_sites()
        .iter()
        .map(|site| {
            let route = site.route;
            [
                route.method.name().to_owned(),
                code(&shown_path(app, site)),
                code(&site.area.area),
                access(app.area(&site.area.area).as_ref()),
                site.permission(app)
                    .map_or("—".to_owned(), |permission| code(&permission)),
                owner_scope(app, &site.area.area).to_owned(),
                throttle(site).map_or("—".to_owned(), code),
                codes_or_dash(&checks(site)),
            ]
        })
        .collect();
    if rows.is_empty() {
        return "None.\n".to_owned();
    }
    table(
        [
            "Method",
            "Path",
            "Area",
            "Access",
            "Permission",
            "Owner scope",
            "Rate limit",
            "Checks",
        ],
        &rows,
    )
}

fn throttle<'site>(site: &RouteSite<'site>) -> Option<&'site str> {
    site.route
        .modifiers
        .iter()
        .find_map(|modifier| match modifier {
            Modifier::Throttle(rate) => Some(rate.as_str()),
            _ => None,
        })
}

fn checks(site: &RouteSite) -> Vec<String> {
    let route = site.route;
    let mut checks = Vec::new();
    match &route.webhook {
        Some(verifier) => checks.push(format!("signature by {}, no session", code(verifier))),
        None if route.method != Method::Get => checks.push("same origin (CSRF)".to_owned()),
        None => {}
    }
    for modifier in &route.modifiers {
        match modifier {
            Modifier::Signed => checks.push("signed link".to_owned()),
            Modifier::Idempotent => checks.push("idempotent".to_owned()),
            Modifier::BodyLimit(bytes) => checks.push(format!("body at most {}", code(bytes))),
            Modifier::FileLimit(bytes) => checks.push(format!("each file at most {}", code(bytes))),
            Modifier::Entitled(entitlement) => {
                checks.push(format!("entitlement {}", code(entitlement)));
            }
            Modifier::Throttle(_)
            | Modifier::Feature(_)
            | Modifier::Live(_)
            | Modifier::Cached(_)
            | Modifier::Document(_)
            | Modifier::Queued => {}
        }
    }
    if takes_upload(site) {
        checks.push("upload".to_owned());
    }
    if let Some(flag) = feature(site) {
        checks.push(format!("flag {}", code(&flag)));
    }
    checks
}

fn codes_or_dash(checks: &[String]) -> String {
    if checks.is_empty() {
        return "—".to_owned();
    }
    checks.join(", ")
}

fn takes_upload(site: &RouteSite) -> bool {
    let code = &site.module.code;
    code.function(&site.route.handler)
        .and_then(|function| code.input_of(function))
        .is_some_and(|input| {
            input.fields.iter().any(|field| {
                field.ty.name == "Upload"
                    || field
                        .ty
                        .arguments
                        .iter()
                        .any(|argument| argument.name == "Upload")
            })
        })
}

fn public_endpoints(app: &App) -> String {
    let mut lines: Vec<String> = app
        .route_sites()
        .iter()
        .filter(|site| {
            app.area(&site.area.area)
                .is_some_and(|area| area.is_public())
        })
        .map(|site| {
            format!(
                "- {} {}",
                site.route.method.name(),
                code(&shown_path(app, site))
            )
        })
        .collect();
    if let Some(identity) = &app.identity {
        lines.push(format!(
            "- sofie-identity's screens: {}",
            codes(&identity.screens)
        ));
    }
    lines.extend(
        FRAMEWORK_ENDPOINTS
            .iter()
            .map(|(path, guarded)| format!("- {}: {guarded}", code(path))),
    );
    format!("{}\n", lines.join("\n"))
}

struct VendorHosts {
    vendor: String,
    called_by: String,
    keys: Vec<String>,
}

fn vendor_hosts(app: &App) -> Vec<VendorHosts> {
    let mut found = Vec::new();
    for module in sorted::modules(app) {
        for gateway in &module.gateways {
            found.extend(gateway.adapters.iter().filter_map(|adapter| {
                let vendor = adapter.vendor.as_deref()?;
                Some(VendorHosts {
                    vendor: vendor.to_owned(),
                    called_by: format!(
                        "{}'s {}",
                        code(&module.krate.crate_name),
                        code(&gateway.path.join("::"))
                    ),
                    keys: host_keys(&vendor_prefix(module, vendor)),
                })
            }));
        }
    }
    found.extend(app.split.remote.iter().map(|remote| VendorHosts {
        vendor: format!("{} (split app)", remote.name),
        called_by: format!("contract {}", code(&remote.contract.crate_name)),
        keys: host_keys(&remote_prefix(&remote.name)),
    }));
    found
}

fn host_keys(prefix: &str) -> Vec<String> {
    ["BASE_URL", "HOSTS", "PRIVATE"]
        .iter()
        .map(|key| format!("{prefix}_{key}"))
        .collect()
}

fn outgoing(app: &App) -> String {
    let rows: Vec<[String; 3]> = vendor_hosts(app)
        .into_iter()
        .map(|vendor| [code(&vendor.vendor), vendor.called_by, codes(&vendor.keys)])
        .collect();
    if rows.is_empty() {
        return "None: no gateway or split module sends requests.\n".to_owned();
    }
    format!(
        "Each vendor reaches only the hosts its keys allow; private and metadata addresses \
         are refused after resolution unless `_PRIVATE` is `true`.\n\n{}",
        table(["Vendor", "Called by", "Hosts allowed by"], &rows)
    )
}

fn secrets(app: &App) -> String {
    let mut rows: Vec<[String; 3]> = framework_keys(app)
        .iter()
        .filter(|key| key.secret)
        .map(|key| secret_row("the framework", key))
        .collect();
    for module in sorted::modules(app) {
        let owner = code(&module.krate.crate_name);
        rows.extend(
            module_keys(module)
                .iter()
                .filter(|key| key.secret)
                .map(|key| secret_row(&owner, key)),
        );
    }
    table(["Key", "Read by", "Description"], &rows)
}

fn secret_row(owner: &str, key: &ConfigKey) -> [String; 3] {
    [code(&key.name), owner.to_owned(), key.doc.join(" ")]
}

fn unchecked(app: &App) -> String {
    let mut lines = vec![
        "- Area-wide rate limits (`app.throttle::<A>(…)`), CORS policies and content security \
         sources, set in code the model does not read."
            .to_owned(),
    ];
    for module in sorted::modules(app) {
        lines.extend(raw_sql(app, module));
        lines.extend(plain_gateways(app, module));
    }
    lines.extend(webhooks(app));
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

fn raw_sql(app: &App, module: &Module) -> Vec<String> {
    module
        .code
        .raw_sql
        .iter()
        .map(|sql| {
            format!(
                "- Raw SQL at {}: held to the module's schema by its database role, not by types.",
                location(&app.root, &sql.file, sql.line)
            )
        })
        .collect()
}

fn webhooks(app: &App) -> Vec<String> {
    app.route_sites()
        .iter()
        .filter_map(|site| {
            let verifier = site.route.webhook.as_ref()?;
            Some(format!(
                "- Webhook {} trusts its verifier {} to check the signature.",
                code(&shown_path(app, site)),
                code(verifier)
            ))
        })
        .collect()
}

fn plain_gateways(app: &App, module: &Module) -> Vec<String> {
    module
        .gateways
        .iter()
        .flat_map(|gateway| {
            gateway
                .adapters
                .iter()
                .filter(|adapter| !adapter.is_adapter)
                .map(move |adapter| {
                    format!(
                        "- {} implements {} without `Adapter` at {}, so its requests are not held to an allow-list.",
                        code(&adapter.ty.join("::")),
                        code(&gateway.path.join("::")),
                        location(&app.root, &adapter.file, adapter.line)
                    )
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::FRAMEWORK_ENDPOINTS;

    #[test]
    fn every_path_sofie_http_reserves_is_listed() {
        let source = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../sofie-http/src/health.rs"),
        )
        .unwrap();
        let reserved = source
            .lines()
            .find(|line| line.starts_with("const RESERVED"))
            .unwrap();

        assert_eq!(
            reserved.matches(", ").count() + 1,
            FRAMEWORK_ENDPOINTS.len(),
            "{reserved}"
        );
        for (path, _) in FRAMEWORK_ENDPOINTS {
            assert!(source.contains(&format!("\"{path}\"")), "{path}");
        }
    }
}
