//! Joined only along edges the model knows exactly; an edge it cannot resolve is listed, never guessed.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::PathBuf;

use sofie_model::naming::RouteSite;
use sofie_model::{App, Effects, Module, Schedule, TypeName};

use crate::routes_command::shown_path;
use crate::sorted;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Step {
    Function {
        module: String,
        path: Vec<String>,
    },
    Method {
        module: String,
        owner: String,
        name: String,
    },
    Event(String),
    Contract(String),
    Resource(String),
}

impl Step {
    pub(crate) fn label(&self) -> String {
        match self {
            Self::Function { module, path } => format!("{module}::{}", path.join("::")),
            Self::Method {
                module,
                owner,
                name,
            } => format!("{module}::{owner}::{name}"),
            Self::Event(name) => format!("event {name}"),
            Self::Contract(name) => format!("contract {name}"),
            Self::Resource(handler) => format!("resource {handler}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EdgeKind {
    Calls,
    Raises,
    Delivers,
    Queues,
    Uses,
}

impl EdgeKind {
    pub(crate) fn verb(self) -> &'static str {
        match self {
            Self::Calls => "calls",
            Self::Raises => "raises",
            Self::Delivers => "delivers",
            Self::Queues => "queues",
            Self::Uses => "uses",
        }
    }

    #[cfg_attr(
        all(not(feature = "docs"), not(test)),
        expect(dead_code, reason = "only `sofie docs` draws every flow")
    )]
    pub(crate) fn is_deferred(self) -> bool {
        matches!(self, Self::Delivers | Self::Queues)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Edge {
    pub(crate) from: Step,
    pub(crate) to: Step,
    pub(crate) kind: EdgeKind,
    pub(crate) repeated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Unresolved {
    pub(crate) file: PathBuf,
    pub(crate) line: usize,
    pub(crate) reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Flow {
    pub(crate) name: String,
    pub(crate) trigger: String,
    pub(crate) entry: Step,
    pub(crate) edges: Vec<Edge>,
    pub(crate) unresolved: Vec<Unresolved>,
}

#[cfg_attr(
    all(not(feature = "docs"), not(test)),
    expect(dead_code, reason = "only `sofie docs` draws every flow")
)]
pub(crate) fn flows(app: &App) -> Vec<Flow> {
    let mut flows: Vec<Flow> = app
        .route_sites()
        .iter()
        .map(|site| endpoint_flow(app, site))
        .collect();
    for module in sorted::modules(app) {
        flows.extend(
            module
                .schedules
                .iter()
                .map(|schedule| schedule_flow(app, module, schedule)),
        );
    }
    flows
}

pub(crate) fn endpoint_flow(app: &App, site: &RouteSite) -> Flow {
    let route = site.route;
    let entry = if route.is_resource() {
        Step::Resource(route.handler.last().cloned().unwrap_or_default())
    } else {
        function_step(site.module, &route.handler)
    };
    let path = shown_path(app, site);
    follow_from(
        app,
        site.name(),
        format!("{} {path}", route.method.name()),
        entry,
    )
}

#[cfg_attr(
    all(not(feature = "docs"), not(test)),
    expect(dead_code, reason = "only `sofie docs` draws every flow")
)]
fn schedule_flow(app: &App, module: &Module, schedule: &Schedule) -> Flow {
    let mut trigger = schedule.cadence.to_string();
    if let Some(each) = &schedule.each {
        let _ = write!(
            trigger,
            ", once per record of {}::{}",
            module.krate.crate_name,
            each.join("::")
        );
    }
    let entry = function_step(module, &schedule.function);
    follow_from(app, entry.label(), trigger, entry)
}

fn follow_from(app: &App, name: String, trigger: String, entry: Step) -> Flow {
    let mut walk = Walk {
        app,
        edges: Vec::new(),
        unresolved: Vec::new(),
        seen: BTreeSet::from([entry.clone()]),
    };
    walk.follow(&entry);
    Flow {
        name,
        trigger,
        entry,
        edges: walk.edges,
        unresolved: walk.unresolved,
    }
}

fn function_step(module: &Module, path: &[String]) -> Step {
    Step::Function {
        module: module.krate.crate_name.clone(),
        path: path.to_vec(),
    }
}

struct Walk<'app> {
    app: &'app App,
    edges: Vec<Edge>,
    unresolved: Vec<Unresolved>,
    seen: BTreeSet<Step>,
}

impl<'app> Walk<'app> {
    fn module(&self, name: &str) -> Option<&'app Module> {
        self.app
            .modules
            .iter()
            .find(|module| module.krate.crate_name == name)
    }

    fn follow(&mut self, step: &Step) {
        match step {
            Step::Function { module, path } => self.follow_function(module, path),
            Step::Method {
                module,
                owner,
                name,
            } => self.follow_method(module, owner, name),
            Step::Event(event) => self.follow_event(step, event),
            Step::Contract(_) | Step::Resource(_) => {}
        }
    }

    fn follow_function(&mut self, module_name: &str, path: &[String]) {
        let Some(module) = self.module(module_name) else {
            return;
        };
        let from = function_step(module, path);
        let Some(function) = module.code.function(path) else {
            self.unresolved.push(Unresolved {
                file: module.krate.path.clone(),
                line: 1,
                reason: format!(
                    "`{}` is not a function the model read in module `{module_name}`",
                    path.join("::")
                ),
            });
            return;
        };
        for contract in function.arguments.iter().filter_map(contract_of) {
            self.edge(&from, &Step::Contract(contract), EdgeKind::Uses);
        }
        let site = Site {
            module,
            file: &function.file,
        };
        self.follow_effects(&from, &site, &function.effects);
    }

    fn follow_method(&mut self, module_name: &str, owner: &str, name: &str) {
        let Some(module) = self.module(module_name) else {
            return;
        };
        let from = Step::Method {
            module: module_name.to_owned(),
            owner: owner.to_owned(),
            name: name.to_owned(),
        };
        let methods = module
            .code
            .methods
            .iter()
            .filter(|method| method.owner == owner && method.name == name);
        for method in methods {
            let site = Site {
                module,
                file: &method.file,
            };
            self.follow_effects(&from, &site, &method.effects);
        }
    }

    fn follow_event(&mut self, from: &Step, event: &str) {
        for module in sorted::modules(self.app) {
            for listener in &module.listeners {
                if listener.event.last().is_some_and(|heard| heard == event) {
                    let to = function_step(module, &listener.function);
                    self.edge(from, &to, EdgeKind::Delivers);
                }
            }
        }
    }

    fn follow_effects(&mut self, from: &Step, site: &Site, effects: &Effects) {
        for call in &effects.calls {
            if let Some(to) = called_step(site.module, &call.path) {
                self.edge(from, &to, EdgeKind::Calls);
            }
        }
        for call in &effects.method_calls {
            match method_step(site.module, call.receiver.as_deref(), &call.method) {
                Some(Ok(to)) => self.edge(from, &to, EdgeKind::Calls),
                Some(Err(reason)) => self.unresolved.push(site.unresolved(call.line, reason)),
                None => {}
            }
        }
        for raised in &effects.raised {
            match &raised.event {
                Some(event) => self.edge(from, &Step::Event(event.clone()), EdgeKind::Raises),
                None => self.unresolved.push(site.unresolved(
                    raised.line,
                    "an event raised from an expression, not a struct literal or path".to_owned(),
                )),
            }
        }
        for queued in &effects.queued {
            match queued_step(site.module, queued.job.as_deref()) {
                Ok(to) => self.edge(from, &to, EdgeKind::Queues),
                Err(reason) => self.unresolved.push(site.unresolved(queued.line, reason)),
            }
        }
    }

    fn edge(&mut self, from: &Step, to: &Step, kind: EdgeKind) {
        let repeated = !self.seen.insert(to.clone());
        self.edges.push(Edge {
            from: from.clone(),
            to: to.clone(),
            kind,
            repeated,
        });
        if !repeated {
            self.follow(to);
        }
    }
}

struct Site<'a> {
    module: &'a Module,
    file: &'a PathBuf,
}

impl Site<'_> {
    fn unresolved(&self, line: usize, reason: String) -> Unresolved {
        Unresolved {
            file: self.file.clone(),
            line,
            reason,
        }
    }
}

fn contract_of(argument: &TypeName) -> Option<String> {
    if !matches!(argument.name.as_str(), "Dep" | "Extensions") {
        return None;
    }
    let object = argument.arguments.first()?;
    (object.name == "dyn")
        .then(|| object.arguments.first().map(|named| named.name.clone()))
        .flatten()
}

fn called_step(module: &Module, path: &[String]) -> Option<Step> {
    if module.code.function(path).is_some() {
        return Some(function_step(module, path));
    }
    let [.., owner, name] = path else {
        return None;
    };
    method_step(module, Some(owner), name).and_then(Result::ok)
}

fn method_step(
    module: &Module,
    receiver: Option<&str>,
    name: &str,
) -> Option<Result<Step, String>> {
    let defined = |owner: &str| {
        module
            .code
            .methods
            .iter()
            .any(|method| method.owner == owner && method.name == name)
    };
    match receiver {
        Some(owner) if defined(owner) => Some(Ok(Step::Method {
            module: module.krate.crate_name.clone(),
            owner: owner.to_owned(),
            name: name.to_owned(),
        })),
        Some(_) => None,
        // Only a method that calls, raises or queues anything would add a step to the flow.
        None => module
            .code
            .methods
            .iter()
            .any(|method| {
                method.name == name && method.implements.is_none() && has_effects(&method.effects)
            })
            .then(|| {
                Err(format!(
                    "`.{name}()` on a value whose type the signature does not name; take it as \
                     an argument of its type, or call `Type::{name}`"
                ))
            }),
    }
}

fn has_effects(effects: &Effects) -> bool {
    !(effects.calls.is_empty()
        && effects.method_calls.is_empty()
        && effects.raised.is_empty()
        && effects.queued.is_empty())
}

fn queued_step(module: &Module, job: Option<&[String]>) -> Result<Step, String> {
    let Some(job) = job else {
        return Err("a job queued from an expression, not a path".to_owned());
    };
    if module.jobs.iter().any(|declared| declared.function == job) {
        return Ok(function_step(module, job));
    }
    Err(format!(
        "`{}` is queued but is not a `#[job]` of module `{}`",
        job.join("::"),
        module.krate.crate_name
    ))
}

#[cfg(test)]
mod tests {
    use sofie_model::testing::{app, area, function, method, module, route, type_name};
    use sofie_model::{Call, Listener, Method, MethodCall, Raised};

    use super::{EdgeKind, Step, flows};

    #[test]
    fn a_call_of_a_method_that_does_nothing_to_follow_is_not_unresolved() {
        let mut order = module("order", "shop/modules/order");
        order.areas.push(area(
            &order,
            "web",
            vec![route(Method::Get, "/orders", "queries::list", 3)],
        ));
        let mut list = function("queries::list", "shop/modules/order/src/queries.rs", 1);
        list.effects.method_calls = vec![MethodCall {
            receiver: None,
            method: "total".to_owned(),
            line: 2,
        }];
        order.code.functions = vec![list];
        order.code.methods = vec![method(
            "Order",
            "total",
            "shop/modules/order/src/order.rs",
            5,
        )];

        let flows = flows(&app("shop", vec![order]));

        assert_eq!(flows[0].unresolved.len(), 0, "{:?}", flows[0].unresolved);
    }

    #[test]
    fn trait_methods_are_no_receiver_guess_and_every_impl_of_a_method_is_followed() {
        let mut order = module("order", "shop/modules/order");
        order.areas.push(area(
            &order,
            "web",
            vec![route(Method::Post, "/orders", "actions::place", 3)],
        ));
        let mut place = function("actions::place", "shop/modules/order/src/actions.rs", 1);
        place.arguments = vec![type_name("Order")];
        place.effects.method_calls = vec![
            MethodCall {
                receiver: None,
                method: "apply".to_owned(),
                line: 2,
            },
            MethodCall {
                receiver: Some("Order".to_owned()),
                method: "apply".to_owned(),
                line: 3,
            },
        ];
        let raising = |event: &str| {
            let mut apply = method("Order", "apply", "shop/modules/order/src/order.rs", 5);
            apply.implements = Some(format!("Apply{event}"));
            apply.effects.raised = vec![Raised {
                event: Some(event.to_owned()),
                line: 6,
            }];
            apply
        };
        order.code.functions = vec![place];
        order.code.methods = vec![raising("OrderPlaced"), raising("OrderPriced")];

        let flows = flows(&app("shop", vec![order]));

        assert_eq!(flows[0].unresolved.len(), 0, "{:?}", flows[0].unresolved);
        let raised: Vec<String> = flows[0]
            .edges
            .iter()
            .filter(|edge| edge.kind.verb() == "raises")
            .map(|edge| edge.to.label())
            .collect();
        assert_eq!(raised, ["event OrderPlaced", "event OrderPriced"]);
    }

    #[test]
    fn follows_an_endpoint_through_a_method_its_event_and_a_listener() {
        let mut order = module("order", "shop/modules/order");
        order.areas.push(area(
            &order,
            "web",
            vec![route(
                Method::Post,
                "/orders/{order}/cancel",
                "actions::cancel",
                3,
            )],
        ));
        let mut cancel = function("actions::cancel", "shop/modules/order/src/actions.rs", 1);
        cancel.arguments = vec![type_name("Order"), type_name("Dep<dyn Payments>")];
        cancel.effects.method_calls = vec![
            MethodCall {
                receiver: Some("Order".to_owned()),
                method: "cancel".to_owned(),
                line: 2,
            },
            MethodCall {
                receiver: None,
                method: "cancel".to_owned(),
                line: 3,
            },
        ];
        let mut cancelled = method("Order", "cancel", "shop/modules/order/src/order.rs", 5);
        cancelled.effects.raised = vec![Raised {
            event: Some("OrderCancelled".to_owned()),
            line: 6,
        }];
        let mut audit = function("listeners::audit", "shop/modules/order/src/listeners.rs", 1);
        audit.effects.calls = vec![Call {
            path: vec!["actions".to_owned(), "cancel".to_owned()],
            line: 2,
        }];
        order.code.functions = vec![cancel, audit];
        order.code.methods = vec![cancelled];
        order.listeners = vec![Listener {
            file: "shop/modules/order/src/listeners.rs".into(),
            line: 1,
            function: vec!["listeners".to_owned(), "audit".to_owned()],
            event: vec!["OrderCancelled".to_owned()],
        }];

        let flows = flows(&app("shop", vec![order]));

        let edges: Vec<(String, &str, String, bool)> = flows[0]
            .edges
            .iter()
            .map(|edge| {
                (
                    edge.from.label(),
                    edge.kind.verb(),
                    edge.to.label(),
                    edge.repeated,
                )
            })
            .collect();
        let expected = [
            ("order::actions::cancel", "uses", "contract Payments", false),
            (
                "order::actions::cancel",
                "calls",
                "order::Order::cancel",
                false,
            ),
            (
                "order::Order::cancel",
                "raises",
                "event OrderCancelled",
                false,
            ),
            (
                "event OrderCancelled",
                "delivers",
                "order::listeners::audit",
                false,
            ),
            (
                "order::listeners::audit",
                "calls",
                "order::actions::cancel",
                true,
            ),
        ]
        .map(|(from, verb, to, repeated)| (from.to_owned(), verb, to.to_owned(), repeated));
        assert_eq!(edges, expected);
        assert_eq!(flows[0].trigger, "POST /orders/{order}/cancel");
        assert_eq!(flows[0].unresolved.len(), 1);
        assert_eq!(flows[0].unresolved[0].line, 3);
        assert!(EdgeKind::Delivers.is_deferred());
        assert_eq!(
            flows[0].entry,
            Step::Function {
                module: "order".to_owned(),
                path: vec!["actions".to_owned(), "cancel".to_owned()]
            }
        );
    }
}
