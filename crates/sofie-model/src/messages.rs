use std::path::Path;

use syn::parse::ParseStream;
use syn::{Attribute, FnArg, Ident, Item, ItemFn, LitStr, Meta, Token, Type};

use crate::areas::{area_definition, implementing};
use crate::code::{Code, derives};
use crate::contract_traits::{ContractTrait, contract_trait};
use crate::gateways::{GatewayTrait, gateway_trait};
use crate::handlers::HandlerScope;
use crate::provisions::{ImplSite, impl_site, unit_struct};
use crate::syntax::{is_attribute, line_of, segments};
use crate::{
    AreaDefinition, Cadence, EventDeclaration, Job, Listener, ModelError, PrivateItem, Schedule,
    Task, WallClock,
};

const TASK_MODULE: &str = "tasks";

#[derive(Default)]
pub(crate) struct Found {
    pub listeners: Vec<Listener>,
    pub schedules: Vec<Schedule>,
    pub events: Vec<EventDeclaration>,
    pub areas: Vec<AreaDefinition>,
    pub member_areas: Vec<String>,
    pub sign_up_areas: Vec<String>,
    pub jobs: Vec<Job>,
    pub tasks: Vec<Task>,
    pub impls: Vec<ImplSite>,
    pub units: Vec<Vec<String>>,
    pub gateways: Vec<GatewayTrait>,
    pub contract_traits: Vec<ContractTrait>,
    pub private_items: Vec<PrivateItem>,
    pub code: Code,
}

impl Found {
    pub(crate) fn wired_count(&self) -> usize {
        self.listeners.len()
            + self.schedules.len()
            + self.events.len()
            + self.areas.len()
            + self.jobs.len()
            + self.tasks.len()
    }
}

pub(crate) fn collect_item(
    item: &Item,
    file: &Path,
    scope: &HandlerScope,
    found: &mut Found,
) -> Result<(), ModelError> {
    let module_path = scope.module_path();
    found.code.collect(item, file, scope);
    found.units.extend(unit_struct(item, module_path));
    found
        .gateways
        .extend(gateway_trait(item, file, module_path));
    found
        .contract_traits
        .extend(contract_trait(item, file, module_path));
    match item {
        Item::Fn(function) => collect_function(function, file, scope, found),
        // `sofie gen` writes a pruned event per aggregate; listing each would only repeat them.
        Item::Struct(_)
            if is_contract_gen(file) && module_path.last().is_some_and(|name| name == "pruned") =>
        {
            Ok(())
        }
        Item::Struct(structure) if derives(&structure.attrs, "Event") => {
            found.events.push(EventDeclaration {
                name: structure.ident.to_string(),
                file: file.to_path_buf(),
                line: line_of(structure),
            });
            Ok(())
        }
        Item::Impl(implementation) => {
            found
                .impls
                .extend(impl_site(implementation, file, module_path));
            found
                .areas
                .extend(area_definition(implementation, file, module_path)?);
            if !is_contract_gen(file) {
                found
                    .member_areas
                    .extend(implementing(implementation, "MemberArea"));
            }
            found
                .sign_up_areas
                .extend(implementing(implementation, "SignUp"));
            Ok(())
        }
        _ => Ok(()),
    }
}

/// `sofie gen` writes a `MemberArea` there itself; read as hand-written, the next run would
/// drop it and the one after write it again.
fn is_contract_gen(file: &Path) -> bool {
    file.ends_with(
        Path::new("src")
            .join(crate::naming::GEN_MODULE)
            .join("mod.rs"),
    )
}

fn collect_function(
    function: &ItemFn,
    file: &Path,
    scope: &HandlerScope,
    found: &mut Found,
) -> Result<(), ModelError> {
    let module_path = scope.module_path();
    let mut path = module_path.to_vec();
    path.push(function.sig.ident.to_string());
    for attribute in &function.attrs {
        let line = line_of(attribute);
        if is_attribute(attribute, "on") {
            found.listeners.push(Listener {
                file: file.to_path_buf(),
                line,
                function: path.clone(),
                event: listened_event(attribute, function)
                    .and_then(|written| scope.qualified(&written))
                    .map_err(|message| ModelError::new(file, line, message))?,
            });
        } else if is_attribute(attribute, "job") {
            found.jobs.push(Job {
                file: file.to_path_buf(),
                line,
                function: path.clone(),
            });
        } else if is_attribute(attribute, "task") {
            let task = task(&found.tasks, file, line, path.clone())
                .map_err(|message| ModelError::new(file, line, message))?;
            found.tasks.push(task);
        } else if is_attribute(attribute, "every") {
            let (cadence, each) = schedule_arguments(attribute);
            found.schedules.push(Schedule {
                file: file.to_path_buf(),
                line,
                function: path.clone(),
                cadence,
                each: each.map(|each| scope.callee(&each)),
            });
        }
    }
    Ok(())
}

fn task(
    tasks: &[Task],
    file: &Path,
    line: usize,
    function: Vec<String>,
) -> Result<Task, &'static str> {
    if function.first().map(String::as_str) != Some(TASK_MODULE) {
        return Err("a task lives in its module's `src/tasks.rs` or `src/tasks/`");
    }
    if tasks
        .iter()
        .any(|task| task.function.last() == function.last())
    {
        return Err(
            "another task of this module has this function's name; a task is named \
             `<module>.<function>`",
        );
    }
    Ok(Task {
        file: file.to_path_buf(),
        line,
        function,
    })
}

/// The attribute macro refuses a malformed one; read as written, the schedule rules report it.
fn schedule_arguments(attribute: &Attribute) -> (Cadence, Option<Vec<String>>) {
    attribute
        .parse_args_with(read_schedule)
        .unwrap_or_else(|_| {
            let written = attribute
                .meta
                .require_list()
                .map(|list| list.tokens.to_string());
            (Cadence::Interval(written.unwrap_or_default()), None)
        })
}

fn read_schedule(input: ParseStream) -> syn::Result<(Cadence, Option<Vec<String>>)> {
    let interval = input
        .peek(LitStr)
        .then(|| input.parse::<LitStr>())
        .transpose()?;
    let (mut each, mut daily_at, mut weekly_on, mut zone) = (None, None, None, None);
    let mut follows_an_argument = interval.is_some();
    while !input.is_empty() {
        if follows_an_argument {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                break;
            }
        }
        follows_an_argument = true;
        let key: Ident = input.parse()?;
        input.parse::<Token![=]>()?;
        if key == "each" {
            each = Some(segments(&input.parse::<syn::Path>()?));
            continue;
        }
        let value = input.parse::<LitStr>()?.value();
        match key.to_string().as_str() {
            "daily_at" => daily_at = Some(value),
            "weekly_on" => weekly_on = Some(value),
            "zone" => zone = Some(value),
            _ => return Err(syn::Error::new(key.span(), "unknown schedule argument")),
        }
    }
    let cadence = match (interval, daily_at) {
        (Some(interval), None) => Cadence::Interval(interval.value()),
        (None, Some(daily_at)) => Cadence::WallClock(WallClock {
            daily_at,
            weekly_on,
            zone: zone.unwrap_or_default(),
        }),
        _ => return Err(input.error("an interval or `daily_at`")),
    };
    Ok((cadence, each))
}

fn listened_event(attribute: &Attribute, function: &ItemFn) -> Result<Vec<String>, String> {
    if !matches!(attribute.meta, Meta::Path(_)) {
        return Err(
            "write a bare `#[on]`: the listener's first argument's type is the event".to_owned(),
        );
    }
    if let Some(FnArg::Typed(argument)) = function.sig.inputs.first()
        && let Type::Path(path) = argument.ty.as_ref()
    {
        return Ok(segments(&path.path));
    }
    Err(
        "a listener takes the event it hears as its first argument: `event: OrderPlaced`"
            .to_owned(),
    )
}

#[must_use]
pub fn interval_seconds(text: &str) -> Option<u64> {
    let unit_at = text.len().checked_sub(1)?;
    let (count, unit) = text.split_at_checked(unit_at)?;
    let scale = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 60 * 60,
        "d" => 24 * 60 * 60,
        _ => return None,
    };
    if !count.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    count
        .parse::<u64>()
        .ok()
        .filter(|count| *count > 0)
        .and_then(|count| count.checked_mul(scale))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_contract_s_generated_file_is_not_hand_written() {
        assert!(super::is_contract_gen(std::path::Path::new(
            "shop/modules/order/contract/src/gen/mod.rs"
        )));
        assert!(!super::is_contract_gen(std::path::Path::new(
            "shop/modules/order/contract/src/area.rs"
        )));
    }

    use super::interval_seconds;

    #[test]
    fn reads_whole_numbers_of_one_unit() {
        assert_eq!(interval_seconds("30s"), Some(30));
        assert_eq!(interval_seconds("15m"), Some(900));
        assert_eq!(interval_seconds("1h"), Some(3600));
        assert_eq!(interval_seconds("2d"), Some(172_800));
    }

    #[test]
    fn refuses_anything_else() {
        for text in [
            "",
            "h",
            "0h",
            "-1h",
            "+1h",
            "1.5h",
            "1w",
            "1 h",
            "1H",
            "99999999999999999999d",
        ] {
            assert_eq!(interval_seconds(text), None, "{text}");
        }
    }
}
