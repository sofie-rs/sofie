use std::collections::BTreeMap;

use crate::naming::{RouteSite, snake_case};
use crate::{Aggregate, App, AreaDefinition, KnownArea, Method, ModelError, Module, Route};

const PERMITTED_PRINCIPAL: &str = "Permitted";

const WHOLE_RECORD: &str = "*";
const RECORD_ACTIONS: [&str; 4] = ["read", "create", "update", "delete"];
const FIELD_ACTIONS: [&str; 2] = ["read", "update"];

impl AreaDefinition {
    #[must_use]
    pub fn is_permitted(&self) -> bool {
        self.principal
            .last()
            .is_some_and(|name| name == PERMITTED_PRINCIPAL)
    }
}

impl KnownArea<'_> {
    #[must_use]
    pub fn is_permitted(&self) -> bool {
        self.definition.is_some_and(AreaDefinition::is_permitted)
    }
}

impl RouteSite<'_> {
    #[must_use]
    pub fn permission(&self, app: &App) -> Option<String> {
        app.area(&self.area.area)
            .is_some_and(|area| area.is_permitted())
            .then(|| route_permission(self.module, self.route))
    }
}

#[must_use]
pub fn permission_constant(permission: &str) -> String {
    let axes: Vec<&str> = permission
        .split('.')
        .skip(1)
        .filter(|axis| *axis != WHOLE_RECORD)
        .collect();
    axes.join("_").to_ascii_uppercase()
}

/// The generated `permissions` module would not compile with one constant written twice.
pub fn refuse_colliding_constants(module: &Module) -> Result<(), ModelError> {
    let mut constants: BTreeMap<String, String> = BTreeMap::new();
    for permission in module_permissions(module) {
        let constant = permission_constant(&permission);
        if let Some(earlier) = constants.insert(constant.clone(), permission.clone()) {
            return Err(ModelError::in_file(
                &module.krate.path.join("src").join("lib.rs"),
                format!(
                    "permissions `{earlier}` and `{permission}` both become the constant \
                     `{constant}`; rename the record or field of one"
                ),
            ));
        }
    }
    Ok(())
}

#[must_use]
pub fn module_permissions(module: &Module) -> Vec<String> {
    let name = &module.krate.crate_name;
    let mut permissions: Vec<String> = module
        .code
        .aggregates
        .iter()
        .flat_map(|aggregate| aggregate_permissions(name, aggregate))
        .collect();
    let routed = module.areas.iter().flat_map(|area| &area.routes);
    permissions.extend(routed.map(|route| route_permission(module, route)));
    permissions.sort();
    permissions.dedup();
    permissions
}

fn aggregate_permissions(module: &str, aggregate: &Aggregate) -> Vec<String> {
    let model = snake_case(&aggregate.name);
    let record = RECORD_ACTIONS
        .iter()
        .map(|action| format!("{module}.{model}.{WHOLE_RECORD}.{action}"));
    let fields = aggregate.fields.iter().flat_map(|field| {
        FIELD_ACTIONS
            .iter()
            .map(|action| format!("{module}.{model}.{}.{action}", field.name))
    });
    record.chain(fields).collect()
}

/// `read` for a page, else the handler's first word, on the longest aggregate name its other words spell, or else its last word.
/// A write never needs `read`, which a read-only member holds: `read_notifications` marks them, so it needs `update`.
#[must_use]
pub fn route_permission(module: &Module, route: &Route) -> String {
    let handler = route.handler.last().map_or("", String::as_str);
    let words: Vec<&str> = handler.split('_').filter(|word| !word.is_empty()).collect();
    let (action, subject) = match (route.method, words.split_first()) {
        (Method::Get, _) => ("read", words.as_slice()),
        (_, None) => ("update", words.as_slice()),
        (_, Some((&"read", rest))) => ("update", rest),
        (_, Some((verb, rest))) => (*verb, rest),
    };
    let models: Vec<String> = module
        .code
        .aggregates
        .iter()
        .map(|aggregate| snake_case(&aggregate.name))
        .collect();
    let model = subject_model(&models, subject, handler);
    format!(
        "{}.{model}.{WHOLE_RECORD}.{action}",
        module.krate.crate_name
    )
}

/// A resource's list handler spells its record's plural, as `list_opening_hours`.
fn subject_model(models: &[String], words: &[&str], handler: &str) -> String {
    (1..=words.len())
        .rev()
        .find_map(|length| {
            words.windows(length).find_map(|run| {
                let joined = run.join("_");
                models
                    .iter()
                    .find(|model| {
                        **model == joined
                            || **model == singular_run(run)
                            || sofie_core::plural(model) == joined
                    })
                    .cloned()
            })
        })
        .unwrap_or_else(|| singular(words.last().copied().unwrap_or(handler)))
}

fn singular_run(run: &[&str]) -> String {
    let mut words: Vec<String> = run.iter().map(|word| (*word).to_owned()).collect();
    if let Some(last) = words.last_mut() {
        *last = singular(last);
    }
    words.join("_")
}

/// Naive, but enough for record names, which is all a handler's words are matched against.
fn singular(word: &str) -> String {
    if let Some(stem) = word.strip_suffix("ies") {
        return format!("{stem}y");
    }
    if ["ss", "us", "is"]
        .iter()
        .any(|ending| word.ends_with(ending))
    {
        return word.to_owned();
    }
    match word.strip_suffix('s') {
        Some(stem) if !stem.is_empty() => stem.to_owned(),
        _ => word.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{permission_constant, singular, subject_model};

    #[test]
    fn a_permission_s_constant_names_its_model_field_and_action() {
        assert_eq!(permission_constant("order.order.*.cancel"), "ORDER_CANCEL");
        assert_eq!(
            permission_constant("order.order_line.total.read"),
            "ORDER_LINE_TOTAL_READ"
        );
    }

    #[test]
    fn plural_record_names_become_singular() {
        assert_eq!(singular("orders"), "order");
        assert_eq!(singular("categories"), "category");
        assert_eq!(singular("order"), "order");
        assert_eq!(singular("address"), "address");
        assert_eq!(singular("status"), "status");
        assert_eq!(singular("campus"), "campus");
    }

    #[test]
    fn a_list_handler_names_its_record_by_the_record_s_plural() {
        let models = ["opening_hours", "category", "status"].map(str::to_owned);
        let model = |handler: &str| {
            let words: Vec<&str> = handler.split('_').collect();
            subject_model(&models, &words[1..], handler)
        };

        assert_eq!(model("list_opening_hours"), "opening_hours");
        assert_eq!(model("list_categories"), "category");
        assert_eq!(model("list_statuses"), "status");
        assert_eq!(model("list_hourses"), "hourse");
    }
}
