use std::collections::BTreeSet;

use sofie_model::naming::{RouteSite, job_name, screen_key, task_name};
use sofie_model::{App, Module, TypeName};

use super::markdown::{code, codes, location, mermaid, table};
use crate::routes_command::{handler_path, shown_path};
use crate::sorted;

struct Metric {
    name: &'static str,
    kind: &'static str,
    labels: &'static [&'static str],
    values: fn(&App) -> Vec<String>,
}

/// As sofie-http, sofie-db, sofie-telemetry and sofie-client declare them; a test holds the two together.
const METRICS: [Metric; 14] = [
    Metric {
        name: "http.server.request.duration",
        kind: "duration",
        labels: &[
            "http.route",
            "http.request.method",
            "http.response.status_code",
            "area",
        ],
        values: route_names_and_areas,
    },
    Metric {
        name: "sofie.handler.duration",
        kind: "duration",
        labels: &["code.function.name", "module"],
        values: handlers,
    },
    Metric {
        name: "db.client.operation.duration",
        kind: "duration",
        labels: &["db.operation.name"],
        values: nothing,
    },
    Metric {
        name: "db.client.connection.count",
        kind: "gauge",
        labels: &[],
        values: nothing,
    },
    Metric {
        name: "messaging.outbox.depth",
        kind: "gauge",
        labels: &[],
        values: nothing,
    },
    Metric {
        name: "messaging.process.lag",
        kind: "duration",
        labels: &["messaging.work"],
        values: nothing,
    },
    Metric {
        name: "messaging.process.outcomes",
        kind: "counter",
        labels: &["messaging.work", "messaging.name", "outcome"],
        values: message_names,
    },
    Metric {
        name: "product.events",
        kind: "counter",
        labels: &["product.event"],
        values: product_event_names,
    },
    Metric {
        name: "sofie.browser.events",
        kind: "counter",
        labels: &["browser.kind", "browser.name", "http.route"],
        values: screen_route_names,
    },
    Metric {
        name: "sofie.shield.decisions",
        kind: "counter",
        labels: &["shield.rule", "shield.grade"],
        values: nothing,
    },
    Metric {
        name: "sofie.shield.signals.dropped",
        kind: "counter",
        labels: &[],
        values: nothing,
    },
    Metric {
        name: "sofie.metrics.dropped",
        kind: "counter",
        labels: &[],
        values: nothing,
    },
    Metric {
        name: "sofie.gateway.duration",
        kind: "duration",
        labels: &["gateway.vendor", "gateway.operation", "outcome"],
        values: vendors,
    },
    Metric {
        name: "sofie.outgoing.duration",
        kind: "duration",
        labels: &["gateway.vendor", "gateway.operation", "outcome"],
        values: vendors,
    },
];

/// The value types sofie-values logs as their mask; a test holds the list to `log_fields.rs`.
const MASKED_TYPES: [&str; 4] = ["Email", "PersonName", "PhoneNumber", "Secret"];

pub(super) fn telemetry(app: &App) -> String {
    let mut text = String::from(
        "# Telemetry\n\nThe framework opens spans and records metrics at every boundary; this \
         page names them for this app. Every log line carries `trace_id`, `span_id`, \
         `request_id`, `module` and `area`.\n\n## Spans\n\n",
    );
    text.push_str(&mermaid("flowchart LR", &span_nesting(app)));
    text.push('\n');
    text.push_str(&table(["Span", "Kind", "Opened for"], &spans(app)));
    text.push_str(
        "\n## Metrics\n\nBehind the `metrics` feature. A label takes only names fixed at \
         compile time, so each is bounded by the values listed.\n\n",
    );
    text.push_str(&metrics(app));
    text.push_str("\n## Product events\n\n");
    text.push_str(&product_events(app));
    text.push_str(
        "\n## Masked fields\n\nThese fields log only their mask, by their value type or a \
         `#[sensitive]` mark.\n\n",
    );
    text.push_str(&masked_fields(app));
    text
}

fn span_nesting(app: &App) -> Vec<String> {
    let modules = sorted::modules(app);
    let any = |present: fn(&Module) -> bool| modules.iter().any(|module| present(module));
    let mut lines = vec![
        "request --> handler".to_owned(),
        "handler --> statement".to_owned(),
        "handler --> commit".to_owned(),
    ];
    if !screen_route_names(app).is_empty() {
        lines.push("request --> render".to_owned());
    }
    if any(|module| !module.listeners.is_empty() || !module.jobs.is_empty()) {
        lines.push("commit -. outbox .-> message[listener or job]".to_owned());
        lines.push("message --> statement".to_owned());
    }
    if any(|module| !module.schedules.is_empty()) {
        lines.push("schedule([schedule]) -.-> message[listener or job]".to_owned());
    }
    if any(|module| !module.code.telemetry.steps.is_empty()) {
        lines.push("handler --> step".to_owned());
    }
    if any(|module| !module.contract_traits.is_empty()) {
        lines.push("handler --> contract".to_owned());
    }
    if any(|module| !module.gateways.is_empty()) {
        lines.push("handler --> gateway --> http.client".to_owned());
    }
    if any(|module| !module.tasks.is_empty()) {
        lines.push("task --> statement".to_owned());
    }
    lines
}

fn spans(app: &App) -> Vec<[String; 3]> {
    let mut rows = route_spans(app);
    for module in sorted::modules(app) {
        rows.extend(message_spans(app, module));
        rows.extend(module_spans(module));
    }
    rows.extend([
        span_row(
            "statement",
            "database",
            "each statement, without its bindings",
        ),
        span_row("commit", "database", "each unit of work's commit"),
    ]);
    if app.modules.iter().any(|module| !module.gateways.is_empty()) {
        rows.push(span_row(
            "http.client",
            "outgoing",
            "each HTTP request an adapter sends, named by its method",
        ));
    }
    rows
}

fn span_row(name: &str, kind: &str, opened_for: &str) -> [String; 3] {
    [code(name), kind.to_owned(), opened_for.to_owned()]
}

fn route_spans(app: &App) -> Vec<[String; 3]> {
    let mut rows = Vec::new();
    let mut handlers = BTreeSet::new();
    let mut screens = BTreeSet::new();
    for site in app.route_sites() {
        let name = site.name();
        let request = format!("{} {name}", site.route.method.name());
        rows.push(span_row(
            &request,
            "request",
            &code(&shown_path(app, &site)),
        ));
        let handler = handler_path(&site);
        if handlers.insert(handler.clone()) {
            rows.push(span_row(&handler, "handler", &code(&name)));
        }
        if let Some(screen) = &site.route.screen {
            let key = screen_key(&site.module.krate.crate_name, &site.area.area, screen);
            if screens.insert(key.clone()) {
                rows.push(span_row(&key, "render", &code(&name)));
            }
        }
    }
    rows
}

fn message_spans(app: &App, module: &Module) -> Vec<[String; 3]> {
    let name = &module.krate.crate_name;
    let job = |function: &[String]| format!("job {}", job_name(module, function));
    let mut rows = Vec::new();
    for event in &module.events {
        let listeners = listener_count(app, event);
        if listeners > 1 {
            rows.push(span_row(
                &format!("event {name}.{event}"),
                "message",
                &format!(
                    "{} raised, handing it to {listeners} listeners",
                    code(event)
                ),
            ));
        }
    }
    for listener in &module.listeners {
        let heard = listener.event.last().map_or("", String::as_str);
        rows.push(span_row(
            &format!("listener {}", job_name(module, &listener.function)),
            "message",
            &format!("{} heard", code(heard)),
        ));
    }
    for queued in &module.jobs {
        rows.push(span_row(
            &job(&queued.function),
            "message",
            "queued by a handler",
        ));
    }
    for schedule in &module.schedules {
        let every = schedule.cadence.to_string();
        let Some(each) = &schedule.each else {
            rows.push(span_row(&job(&schedule.function), "message", &every));
            continue;
        };
        rows.push(span_row(&job(each), "message", &every));
        rows.push(span_row(
            &job(&schedule.function),
            "message",
            &format!("once per record {} lists", code(&each.join("::"))),
        ));
    }
    rows
}

fn listener_count(app: &App, event: &str) -> usize {
    app.modules
        .iter()
        .flat_map(|module| &module.listeners)
        .filter(|listener| listener.event.last().is_some_and(|heard| heard == event))
        .count()
}

fn module_spans(module: &Module) -> Vec<[String; 3]> {
    let name = &module.krate.crate_name;
    let mut rows: Vec<[String; 3]> = module
        .tasks
        .iter()
        .map(|task| {
            span_row(
                &format!("task {}", task_name(module, &task.function)),
                "task",
                "`sofie task` running it",
            )
        })
        .collect();
    rows.extend(module.code.telemetry.steps.iter().map(|step| {
        span_row(
            &format!("{name}::{}", step.join("::")),
            "step",
            "`#[span]` on the function",
        )
    }));
    for contract in &module.contract_traits {
        rows.extend(contract.methods.iter().map(|method| {
            span_row(
                &format!("{}::{}", contract.name, method.name),
                "contract",
                &format!("a call of {}'s contract", code(name)),
            )
        }));
    }
    for gateway in &module.gateways {
        let trait_name = gateway.path.join("::");
        rows.extend(gateway.adapters.iter().filter_map(|adapter| {
            let vendor = adapter.vendor.as_deref()?;
            Some(span_row(
                &format!("{vendor}.<operation>"),
                "gateway",
                &format!(
                    "a call of {}'s {} or an `Outgoing::operation` of {}",
                    code(name),
                    code(&trait_name),
                    code(vendor)
                ),
            ))
        }));
    }
    rows
}

fn metrics(app: &App) -> String {
    let rows: Vec<[String; 4]> = METRICS
        .iter()
        .map(|metric| {
            [
                code(metric.name),
                metric.kind.to_owned(),
                codes(metric.labels),
                codes(&(metric.values)(app)),
            ]
        })
        .collect();
    table(["Metric", "Kind", "Labels", "Names in this app"], &rows)
}

fn nothing(_: &App) -> Vec<String> {
    Vec::new()
}

fn route_names_and_areas(app: &App) -> Vec<String> {
    let sites = app.route_sites();
    let mut values: Vec<String> = sites.iter().map(RouteSite::name).collect();
    let areas: BTreeSet<&str> = sites.iter().map(|site| site.area.area.as_str()).collect();
    values.extend(areas.into_iter().map(str::to_owned));
    values
}

fn handlers(app: &App) -> Vec<String> {
    let sites = app.route_sites();
    let mut seen = BTreeSet::new();
    let mut values: Vec<String> = sites
        .iter()
        .map(handler_path)
        .filter(|handler| seen.insert(handler.clone()))
        .collect();
    let modules: BTreeSet<&str> = sites
        .iter()
        .map(|site| site.module.krate.crate_name.as_str())
        .collect();
    values.extend(modules.into_iter().map(str::to_owned));
    values
}

fn message_names(app: &App) -> Vec<String> {
    let mut names = Vec::new();
    for module in sorted::modules(app) {
        let name = &module.krate.crate_name;
        names.extend(
            module
                .events
                .iter()
                .filter(|event| listener_count(app, event) > 1)
                .map(|event| format!("{name}.{event}")),
        );
        names.extend(
            module
                .listeners
                .iter()
                .map(|listener| job_name(module, &listener.function)),
        );
        names.extend(
            module
                .jobs
                .iter()
                .map(|job| job_name(module, &job.function)),
        );
        for schedule in &module.schedules {
            names.extend(schedule.each.iter().map(|each| job_name(module, each)));
            names.push(job_name(module, &schedule.function));
        }
    }
    names
}

fn product_event_names(app: &App) -> Vec<String> {
    let names: BTreeSet<&str> = app
        .modules
        .iter()
        .flat_map(|module| &module.code.telemetry.product_events)
        .map(|event| event.name.as_str())
        .collect();
    names.into_iter().map(str::to_owned).collect()
}

fn screen_route_names(app: &App) -> Vec<String> {
    app.route_sites()
        .iter()
        .filter(|site| site.route.screen.is_some())
        .map(RouteSite::name)
        .collect()
}

fn vendors(app: &App) -> Vec<String> {
    let vendors: BTreeSet<&str> = app
        .modules
        .iter()
        .flat_map(|module| &module.gateways)
        .flat_map(|gateway| &gateway.adapters)
        .filter_map(|adapter| adapter.vendor.as_deref())
        .collect();
    vendors.into_iter().map(str::to_owned).collect()
}

fn product_events(app: &App) -> String {
    let mut rows = Vec::new();
    for module in sorted::modules(app) {
        rows.extend(module.code.telemetry.product_events.iter().map(|event| {
            let logged: Vec<&str> = event
                .values
                .iter()
                .map(|value| value.rsplit('.').next().unwrap_or(value))
                .collect();
            [
                code(&event.name),
                codes(&logged),
                location(&app.root, &event.file, event.line),
            ]
        }));
    }
    if rows.is_empty() {
        return "None: no module calls `product_event!`.\n".to_owned();
    }
    format!(
        "Counted in full under the target `product`, never sampled.\n\n{}",
        table(["Event", "Values", "Recorded at"], &rows)
    )
}

fn masked_fields(app: &App) -> String {
    let sensitive: BTreeSet<&str> = app
        .modules
        .iter()
        .flat_map(|module| &module.code.telemetry.sensitive_values)
        .map(String::as_str)
        .chain(MASKED_TYPES)
        .collect();
    let mut rows = Vec::new();
    for module in sorted::modules(app) {
        let code_of = &module.code;
        let marked = &code_of.telemetry.sensitive_fields;
        let owners = code_of
            .aggregates
            .iter()
            .map(|aggregate| (&aggregate.name, &aggregate.fields))
            .chain(
                code_of
                    .inputs
                    .iter()
                    .map(|input| (&input.name, &input.fields)),
            )
            .chain(
                code_of
                    .read_models
                    .iter()
                    .map(|model| (&model.name, &model.fields)),
            );
        for (owner, fields) in owners {
            rows.extend(
                fields
                    .iter()
                    .filter(|field| {
                        is_masked(&field.ty, &sensitive)
                            || marked.iter().any(|(marked_owner, marked_field)| {
                                marked_owner == owner && *marked_field == field.name
                            })
                    })
                    .map(|field| {
                        [
                            code(&format!("{}::{owner}", module.krate.crate_name)),
                            code(&field.name),
                            code(&field.ty.to_string()),
                        ]
                    }),
            );
        }
    }
    if rows.is_empty() {
        return "None.\n".to_owned();
    }
    table(["Type", "Field", "Value type"], &rows)
}

fn is_masked(ty: &TypeName, sensitive: &BTreeSet<&str>) -> bool {
    sensitive.contains(ty.name.as_str())
        || ty
            .arguments
            .iter()
            .any(|argument| is_masked(argument, sensitive))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::{Path, PathBuf};

    use sofie_model::Method;
    use sofie_model::testing::{app, area, module, route};

    use super::{MASKED_TYPES, METRICS, telemetry};

    #[test]
    fn a_handler_two_routes_share_is_named_once() {
        let mut order = module("order", "shop/modules/order");
        let routes = vec![
            route(Method::Get, "/orders", "queries::orders", 3),
            route(Method::Get, "/orders/{order}", "queries::order", 4),
            route(Method::Get, "/history", "queries::orders", 5),
        ];
        order.areas.push(area(&order, "web", routes));

        let page = telemetry(&app("shop", vec![order]));

        assert!(
            page.contains(
                "| `sofie.handler.duration` | duration | `code.function.name`, `module` | \
                 `order::queries::orders`, `order::queries::order`, `order` |"
            ),
            "{page}"
        );
    }

    fn sources(directory: &Path, found: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                sources(&path, found);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                found.push(path);
            }
        }
    }

    fn quoted(text: &str) -> Vec<String> {
        text.split('"')
            .skip(1)
            .step_by(2)
            .map(str::to_owned)
            .collect()
    }

    fn declared_instruments() -> BTreeSet<(String, Vec<String>)> {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut files = Vec::new();
        for krate in ["sofie-http", "sofie-db", "sofie-telemetry", "sofie-client"] {
            sources(&crates.join(krate).join("src"), &mut files);
        }
        let mut instruments = BTreeSet::new();
        for file in files {
            let source = fs::read_to_string(&file).unwrap();
            for (start, _) in source.match_indices("Instrument {\n") {
                let before = source[..start]
                    .trim_end_matches(|c: char| c.is_alphanumeric() || c == '_' || c == ':');
                if !before.trim_end().ends_with('=') {
                    continue;
                }
                let body = &source[start..start + source[start..].find("};").unwrap()];
                let name = quoted(body.lines().find(|line| line.contains("name:")).unwrap());
                let labels = &body[body.find("labels:").unwrap()..];
                let labels = &labels[..=labels.find(']').unwrap()];
                if !name[0].starts_with("test.") {
                    instruments.insert((name[0].clone(), quoted(labels)));
                }
            }
        }
        instruments
    }

    #[test]
    fn every_metric_the_framework_records_is_listed_with_its_labels() {
        let listed: BTreeSet<(String, Vec<String>)> = METRICS
            .iter()
            .map(|metric| {
                (
                    metric.name.to_owned(),
                    metric
                        .labels
                        .iter()
                        .map(|label| (*label).to_owned())
                        .collect(),
                )
            })
            .collect();

        assert_eq!(listed, declared_instruments());
    }

    #[test]
    fn every_value_type_logged_as_its_mask_is_listed() {
        let source = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../sofie-values/src/log_fields.rs"),
        )
        .unwrap();
        let mut masked: Vec<&str> = source
            .lines()
            .filter(|line| line.starts_with("log_whole!(") && line.contains("masked("))
            .filter_map(|line| line["log_whole!(".len()..].split(" =>").next())
            .map(|ty| ty.rsplit("::").next().unwrap_or(ty))
            .collect();
        if source.contains("impl<T> LogFields for Secret<T>") {
            masked.push("Secret");
        }
        masked.sort_unstable();

        assert_eq!(masked, MASKED_TYPES);
    }
}
