use sofie::db::Uuid;
use sofie::flags::{Flags, Rule};
use sofie::http::{Area, FlagRules, NeedsPermission, Permission, Routes, Shipped};
use sofie::prelude::*;
use sofie::serde_json::{Value, json};

use super::Listed;

const READ: Permission = Permission::new("sofie_identity.flag.*.read");
const UPDATE: Permission = Permission::new("sofie_identity.flag.*.update");

pub(super) fn routes<A: Area>(routes: &mut Routes<A>) {
    let describe: sofie::http::Describe =
        |screens, key| screens.describe_form::<Targeting>(key, "switch");
    routes
        .shipped(
            "/flags",
            "flags",
            (include_str!("flags.html"), Some(describe)),
            list,
        )
        .name("list_flags")
        .permission(READ);
    routes
        .post("/flags", target)
        .name("target_flag")
        .permission(UPDATE)
        .then_screen("flags", "flags");
}

async fn list(flags: FlagRules) -> Result<Listed> {
    let rows = flags
        .rules()
        .into_iter()
        .map(|(name, rule)| row(&name, &rule))
        .collect();
    Ok(Listed(Value::Array(rows)))
}

fn row(name: &str, rule: &Rule) -> Value {
    let principals: Vec<String> = rule.principals.iter().map(ToString::to_string).collect();
    let variants: Vec<String> = rule
        .variants
        .iter()
        .map(|(variant, weight)| format!("{variant}:{weight}"))
        .collect();
    json!({
        "name": name,
        "on": rule.on,
        "environments": rule.environments.join(", "),
        "areas": rule.areas.join(", "),
        "principals": principals.join(", "),
        "percentage": rule.percentage,
        "variants": variants.join(", "),
    })
}

/// Lists are comma-separated, as a form posts them; none listed means everyone.
#[derive(Input)]
struct Targeting {
    #[required]
    flag: String,
    on: bool,
    environments: Option<String>,
    areas: Option<String>,
    principals: Option<String>,
    percentage: Option<u16>,
    variants: Option<String>,
}

async fn target(flags: Flags, targeting: Targeting) -> Result<()> {
    let rule = Rule {
        on: targeting.on,
        environments: entries(targeting.environments.as_deref()),
        areas: entries(targeting.areas.as_deref()),
        principals: principals(targeting.principals.as_deref())?,
        percentage: percentage(targeting.percentage)?,
        variants: variants(targeting.variants.as_deref())?,
    };
    flags.set(&targeting.flag, rule).await
}

fn entries(list: Option<&str>) -> Vec<String> {
    list.unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_owned)
        .collect()
}

fn principals(list: Option<&str>) -> Result<Vec<Uuid>> {
    entries(list)
        .iter()
        .map(|entry| {
            entry.parse().map_err(|_| {
                Error::invalid("principals", format!("`{entry}` is not a principal's id"))
            })
        })
        .collect()
}

fn percentage(posted: Option<u16>) -> Result<u8> {
    posted.map_or(Ok(100), |percentage| {
        u8::try_from(percentage)
            .ok()
            .filter(|percentage| *percentage <= 100)
            .ok_or_else(|| Error::invalid("percentage", "is between 0 and 100"))
    })
}

fn variants(list: Option<&str>) -> Result<Vec<(String, u32)>> {
    entries(list)
        .iter()
        .map(|entry| {
            let weighted = entry.split_once(':').and_then(|(variant, weight)| {
                Some((variant.trim().to_owned(), weight.trim().parse().ok()?))
            });
            weighted.ok_or_else(|| {
                Error::invalid(
                    "variants",
                    format!("`{entry}` is not a variant and its weight, such as `COMPACT:3`"),
                )
            })
        })
        .collect()
}
