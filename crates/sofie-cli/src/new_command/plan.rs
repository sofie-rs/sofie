use sofie_model::{App, Module};

use super::Edits;
use crate::gen_command::GenError;
use crate::source_edits::read;

const CATALOG: &str = "pub const PLANS: Plans = Plans::new(";

/// The first plan is the default, which every owner without a subscription holds.
pub(super) fn new_plan(
    edits: &mut Edits,
    app: &App,
    module: &Module,
    name: &str,
) -> Result<(), GenError> {
    if let Some(other) = app
        .modules
        .iter()
        .find(|other| other.plans.is_some() && other.krate.path != module.krate.path)
    {
        return Err(GenError::Usage(format!(
            "module `{}` declares the app's plans; add `{name}` there",
            other.krate.crate_name
        )));
    }
    let constant = name.to_ascii_uppercase();
    if constant == "PLANS" {
        return Err(GenError::Usage(
            "`PLANS` names the catalog of plans; choose another name".to_owned(),
        ));
    }
    let declaration = format!("pub const {constant}: Plan = Plan::new(\"{name}\");\n");
    edits.enable_framework_feature(&module.krate, "billing")?;
    let file = module.krate.path.join("src").join("plans.rs");
    if !file.is_file() {
        edits.create_rust(
            &file,
            &format!(
                "use sofie::billing::{{Plan, Plans}};\n\n{declaration}\n{CATALOG}{constant}, &[]);\n"
            ),
        )?;
        return edits.declare(&module.krate, &["plans"]);
    }
    if read(&file)?.contains(&format!("const {constant}:")) {
        return Err(GenError::Usage(format!(
            "{} declares plan `{name}` already; choose another name",
            file.display()
        )));
    }
    edits.edit_rust(&file, |text| {
        let listed = listed(text, &constant).ok_or_else(|| {
            GenError::Usage(format!(
                "{} has no `{CATALOG}DEFAULT, &[…]);` to list `{constant}` in; add it there",
                file.display()
            ))
        })?;
        let start = text.find(CATALOG).unwrap_or(text.len());
        Ok(format!(
            "{}{declaration}\n{}",
            &listed[..start],
            &listed[start..]
        ))
    })
}

/// The catalog with `constant` added to the end of its other plans.
fn listed(text: &str, constant: &str) -> Option<String> {
    let start = text.find(CATALOG)?;
    let list = start + text[start..].find("&[")? + 2;
    let end = list + text[list..].find(']')?;
    let others = text[list..end].trim().trim_end_matches(',');
    let separator = if others.is_empty() { "" } else { ", " };
    Some(format!(
        "{}{others}{separator}{constant}{}",
        &text[..list],
        &text[end..]
    ))
}

#[cfg(test)]
mod tests {
    use super::listed;

    #[test]
    fn a_new_plan_joins_the_end_of_the_catalog() {
        assert_eq!(
            listed("pub const PLANS: Plans = Plans::new(FREE, &[]);\n", "PRO").unwrap(),
            "pub const PLANS: Plans = Plans::new(FREE, &[PRO]);\n"
        );
        assert_eq!(
            listed(
                "pub const PLANS: Plans = Plans::new(FREE, &[PRO,]).products(&[]);",
                "TEAM"
            )
            .unwrap(),
            "pub const PLANS: Plans = Plans::new(FREE, &[PRO, TEAM]).products(&[]);"
        );
        assert!(listed("const X: u8 = 1;", "PRO").is_none());
    }
}
