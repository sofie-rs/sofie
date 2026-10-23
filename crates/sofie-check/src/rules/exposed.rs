use sofie_model::{Aggregate, Module};

use super::{Finding, Rule};

pub(super) fn check_exposed(modules: &[Module]) -> Vec<Finding> {
    let mut findings = Vec::new();
    for module in modules {
        for aggregate in module
            .code
            .aggregates
            .iter()
            .filter(|aggregate| !aggregate.exposed.is_empty())
        {
            if module.contract.is_none() {
                findings.push(Finding {
                    rule: Rule::ExposedField,
                    file: aggregate.file.clone(),
                    line: aggregate.line,
                    message: format!(
                        "`{}` exposes fields, but module `{}` has no contract to publish them in",
                        aggregate.name, module.krate.package
                    ),
                    fix: "add the module's `contract/` crate, or drop `#[expose]`".to_owned(),
                });
            }
            if let Err(field) = aggregate.contract_fields(&module.code) {
                findings.push(Finding {
                    rule: Rule::ExposedField,
                    file: aggregate.file.clone(),
                    line: field.line,
                    message: format!(
                        "exposed field `{}` is a `{}`, which a contract cannot name",
                        field.name, field.ty
                    ),
                    fix: "expose a field of a scalar, `String`, an sofie value, `Id<X>`, a module \
                          enum or an `Option` of one, or drop `#[expose]`"
                        .to_owned(),
                });
            }
        }
    }
    findings
}

pub(super) fn check_mirrors(modules: &[Module]) -> Vec<Finding> {
    let mut findings = Vec::new();
    for module in modules {
        for mirror in &module.code.aggregates {
            let Some(source) = &mirror.mirror else {
                continue;
            };
            findings.extend(check_mirror(modules, mirror, source));
        }
    }
    findings
}

fn check_mirror(modules: &[Module], mirror: &Aggregate, source: &[String]) -> Vec<Finding> {
    let [contract, name] = source else {
        return vec![mirror_finding(
            mirror,
            mirror.line,
            format!(
                "`#[mirror({})]` does not name a contract's aggregate",
                source.join("::")
            ),
        )];
    };
    let exposing = modules
        .iter()
        .filter(|module| {
            module
                .contract
                .as_ref()
                .is_some_and(|krate| &krate.crate_name == contract)
        })
        .flat_map(|module| &module.code.aggregates)
        .find(|aggregate| &aggregate.name == name);
    let Some(exposing) = exposing else {
        return vec![mirror_finding(
            mirror,
            mirror.line,
            format!("`{contract}::{name}` is no aggregate whose module has that contract"),
        )];
    };
    let hides_nothing = (exposing.soft_deletes() && !mirror.soft_deletes()).then(|| {
        mirror_finding(
            mirror,
            mirror.line,
            format!(
                "`{name}` deletes softly, but its mirror `{}` keeps no `deleted_at`, so it would \
                 go on showing deleted records",
                mirror.name
            ),
        )
    });
    mirror
        .fields
        .iter()
        .filter(|field| !exposing.exposed.contains(&field.name))
        .map(|field| {
            mirror_finding(
                mirror,
                field.line,
                format!(
                    "`{}` mirrors `{}`, which `{name}` does not expose",
                    mirror.name, field.name
                ),
            )
        })
        .chain(hides_nothing)
        .collect()
}

fn mirror_finding(mirror: &Aggregate, line: usize, message: String) -> Finding {
    Finding {
        rule: Rule::MirrorField,
        file: mirror.file.clone(),
        line,
        message,
        fix: "mirror only fields the source marks `#[expose]`, by the same name, and delete \
              softly (`#[aggregate(behaviors(SoftDelete))]`) when the source does"
            .to_owned(),
    }
}
