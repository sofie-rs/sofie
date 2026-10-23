use sofie_model::{App, AreaDefinition, Module};

use super::{Finding, Rule};

pub(super) fn check_member_areas(app: &App) -> Vec<Finding> {
    let mut findings = Vec::new();
    for module in &app.modules {
        for area in module
            .defined_areas
            .iter()
            .filter(|area| area.needs_generated_member_area())
        {
            if app.member_home(area).is_none() {
                findings.push(no_home(area));
            }
            if !module.contract_declares_gen {
                findings.push(undeclared_gen(module, area));
            }
        }
    }
    findings
}

fn no_home(area: &AreaDefinition) -> Finding {
    Finding {
        rule: Rule::MemberHome,
        file: area.file.clone(),
        line: area.line,
        message: format!(
            "member area `{}` {}, so members have nowhere to land once signed in",
            area.name,
            if area.prefix.is_some() {
                "routes no screen without path parameters"
            } else {
                "has no literal `PREFIX`"
            }
        ),
        fix:
            "route a screen in the area, or write `impl sofie_identity::MemberArea` with its `HOME`"
                .to_owned(),
    }
}

fn undeclared_gen(module: &Module, area: &AreaDefinition) -> Finding {
    let root = module.contract.as_ref().map_or_else(
        || area.file.clone(),
        |contract| contract.path.join("src/lib.rs"),
    );
    Finding {
        rule: Rule::UndeclaredGen,
        file: root,
        line: 1,
        message: format!(
            "`sofie gen` writes `impl MemberArea` for area `{}` into src/gen/mod.rs, which no `mod` \
             declaration reaches",
            area.name
        ),
        fix: "add `mod r#gen;` to the contract's src/lib.rs".to_owned(),
    }
}
