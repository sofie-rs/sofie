use sofie_model::Module;

use super::manifest::DependencySite;
use super::{Finding, Rule};

pub(super) fn check_direction(site: &DependencySite<'_>, target: &Module) -> Option<Finding> {
    let own = site.module.manifest.as_ref()?.tier;
    let other = target.manifest.as_ref()?.tier;
    if own.may_depend_on(other) {
        return None;
    }
    Some(Finding {
        rule: Rule::TierDirection,
        file: site.manifest.clone(),
        line: site.line,
        message: format!(
            "module `{}` ({}) depends on `{}` of module `{}` ({}); dependencies flow \
             Experience → Core → Platform, so a lower tier never knows a higher one",
            site.module.krate.package,
            own.name(),
            site.dependency,
            target.krate.package,
            other.name()
        ),
        fix: format!(
            "let `{}` call `{}` instead, or have `{}` publish an event the other listens to",
            target.krate.package, site.module.krate.package, site.module.krate.package
        ),
    })
}

pub(super) fn app_dependency_finding(site: &DependencySite<'_>) -> Finding {
    Finding {
        rule: Rule::TierDirection,
        file: site.manifest.clone(),
        line: site.line,
        message: format!(
            "`{}` depends on the app `{}`; nothing depends on the app, which only wires modules",
            site.krate.package, site.dependency
        ),
        fix: "move what it needs into a module and depend on that module's contract".to_owned(),
    }
}
