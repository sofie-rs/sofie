use sofie_core::DisplayZone;
use sofie_model::naming::RouteSite;
use sofie_model::{
    App, Cadence, EventDeclaration, Listener, Modifier, Module, Schedule, Tier, interval_seconds,
};

use super::{Finding, Rule};

pub(super) fn check_messages(app: &App) -> Vec<Finding> {
    let modules = &app.modules;
    // The contract's `<Aggregate>Pruned` may not be generated yet.
    let pruned: Vec<String> = modules
        .iter()
        .flat_map(|module| &module.code.aggregates)
        .map(|aggregate| format!("{}Pruned", aggregate.name))
        .collect();
    let events: Vec<&str> = modules
        .iter()
        .flat_map(|module| &module.events)
        .chain(&pruned)
        .map(String::as_str)
        .chain(app.package_events.iter().map(|event| event.name.as_str()))
        .collect();
    let mut findings = Vec::new();
    for module in modules {
        findings.extend(
            module
                .listeners
                .iter()
                .filter_map(|listener| check_listener(listener, &events)),
        );
        findings.extend(module.schedules.iter().filter_map(check_schedule));
        findings.extend(
            module
                .schedules
                .iter()
                .filter_map(|schedule| check_each(module, schedule)),
        );
        findings.extend(
            module
                .module_events
                .iter()
                .map(|event| outside_contract(module, event)),
        );
        if module.manifest.as_ref().map(|manifest| manifest.tier) == Some(Tier::Platform) {
            findings.extend(
                module
                    .listeners
                    .iter()
                    .filter_map(|listener| check_platform_listener(listener, modules)),
            );
        }
    }
    findings
}

/// A queued route's job runs on the worker for no principal, so it must be a job and its
/// area one whose principals own nothing.
pub(super) fn check_queued_routes(app: &App) -> Vec<Finding> {
    app.route_sites()
        .into_iter()
        .filter(|site| site.route.modifiers.contains(&Modifier::Queued))
        .filter_map(|site| unqueueable(app, &site))
        .collect()
}

fn unqueueable(app: &App, site: &RouteSite) -> Option<Finding> {
    let handler = site.route.handler.join("::");
    let finding = |message: String, fix: &str| Finding {
        rule: Rule::QueuedRoute,
        file: site.area.routes_file.clone(),
        line: site.route.line,
        message,
        fix: fix.to_owned(),
    };
    if !site
        .module
        .jobs
        .iter()
        .any(|job| job.function == site.route.handler)
    {
        return Some(finding(
            format!(
                "{} queues `{handler}`, which is no `#[job]`, so the worker could not run it",
                site.route.path
            ),
            "mark the action `#[sofie::job]` and run `sofie gen`",
        ));
    }
    let owned = app
        .area(&site.area.area)
        .is_some_and(|area| !area.is_public() && !area.is_permitted());
    owned.then(|| {
        finding(
            format!(
                "{} queues `{handler}` from area `{}`, whose principals own their records, but a \
                 job reaches every record",
                site.route.path, site.area.area
            ),
            "route an action that checks the record and queues the job through `Jobs`, or \
             queue it from a public or permitted area",
        )
    })
}

/// D15: Platform reacts to the modules above it only by being called.
fn check_platform_listener(listener: &Listener, modules: &[Module]) -> Option<Finding> {
    let (Some(source), Some(event)) = (listener.event.first(), listener.event.last()) else {
        return None;
    };
    // The model qualifies the event's path, so it starts with the crate defining it.
    let above = modules.iter().find(|module| {
        let defines_source = module.krate.crate_name == *source
            || module
                .contract
                .as_ref()
                .is_some_and(|contract| contract.crate_name == *source);
        defines_source
            && module.events.iter().any(|defined| defined == event)
            && module
                .manifest
                .as_ref()
                .is_some_and(|manifest| manifest.tier > Tier::Platform)
    })?;
    Some(Finding {
        rule: Rule::PlatformListener,
        file: listener.file.clone(),
        line: listener.line,
        message: format!(
            "a Platform module listens to `{event}` of `{}`, which would make it depend on a tier above it",
            above.krate.package
        ),
        fix: format!(
            "let `{}` call this module's contract, or listen to a Platform event",
            above.krate.package
        ),
    })
}

fn check_listener(listener: &Listener, events: &[&str]) -> Option<Finding> {
    let event = listener.event.last().map_or("", String::as_str);
    if events.contains(&event) {
        return None;
    }
    Some(Finding {
        rule: Rule::ListenerEvent,
        file: listener.file.clone(),
        line: listener.line,
        message: format!(
            "`{event}` does not derive `Event`, so nothing can deliver it to this listener"
        ),
        fix: format!("add `#[derive(Event)]` to `{event}`, or listen to an event"),
    })
}

fn check_schedule(schedule: &Schedule) -> Option<Finding> {
    let interval = match &schedule.cadence {
        Cadence::Interval(interval) => interval,
        Cadence::WallClock(wall_clock) => return check_zone(schedule, &wall_clock.zone),
    };
    if interval_seconds(interval).is_some() {
        return None;
    }
    Some(Finding {
        rule: Rule::ScheduleInterval,
        file: schedule.file.clone(),
        line: schedule.line,
        message: format!("`{interval}` is not an interval"),
        fix: "write a positive whole number and a unit: `#[every(\"30s\")]`, `\"15m\"`, `\"1h\"` or `\"1d\"`"
            .to_owned(),
    })
}

/// The attribute macro checks the time and weekday as the app compiles; a zone only the
/// zone database knows. The worker reads the zone untrimmed, unlike `DisplayZone`.
fn check_zone(schedule: &Schedule, zone: &str) -> Option<Finding> {
    if zone.trim() == zone && DisplayZone::named(zone).is_ok() {
        return None;
    }
    Some(Finding {
        rule: Rule::ScheduleZone,
        file: schedule.file.clone(),
        line: schedule.line,
        message: format!("`{zone}` is not an IANA time zone, so the schedule cannot run"),
        fix: "name the zone whose clock it reads: `zone = \"Europe/Istanbul\"`".to_owned(),
    })
}

fn outside_contract(module: &Module, event: &EventDeclaration) -> Finding {
    Finding {
        rule: Rule::EventInContract,
        file: event.file.clone(),
        line: event.line,
        message: format!(
            "event `{}` is declared in module `{}` itself, which no other module may import",
            event.name, module.krate.package
        ),
        fix: format!(
            "move `{}` to the module's `contract/src/events.rs` and export it from the contract",
            event.name
        ),
    }
}

fn check_each(module: &Module, schedule: &Schedule) -> Option<Finding> {
    let each = schedule.each.as_ref()?;
    let declared = module
        .code
        .functions
        .iter()
        .any(|function| &function.path == each);
    if declared {
        return None;
    }
    Some(Finding {
        rule: Rule::ScheduleEach,
        file: schedule.file.clone(),
        line: schedule.line,
        message: format!(
            "`each = {}` names no function of module `{}`",
            each.join("::"),
            module.krate.package
        ),
        fix: "name a function of the module listing the records to run on: \
              `async fn stale(read: Read) -> Result<Vec<Id<Order>>>`"
            .to_owned(),
    })
}
