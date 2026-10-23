use std::fmt::Write;

use sofie_engine::{Children, ComponentSpec, SPECS};

/// With the app's sofie features, each feature a component needs is marked on or off.
#[must_use]
pub fn component_list(enabled: Option<&[String]>) -> String {
    let mut text = String::from(
        "Screen components, written in templates as `<Name prop=\"…\" />`. Templates are \
         HTML besides: plain elements, `{data.path}` text, `{#if …}`/`{:else}`/`{/if}`, \
         `{#each list as item, index}`/`{/each}`; conditions may ask `can(\"…\")`, \
         `enabled(\"…\")` and `entitled(\"…\")`, and text may be `t(\"key\", name = …)`, \
         `number(…)` or `money(…)` (the `i18n` feature) or `date(…)`. `explain <Name>` gives \
         one component's props.\n",
    );
    if let Some(enabled) = enabled {
        let mut features: Vec<&str> = SPECS.iter().filter_map(|spec| spec.feature).collect();
        features.dedup();
        let states: Vec<String> = features
            .iter()
            .map(|feature| format!("`{feature}` {}", state(feature, enabled)))
            .collect();
        let _ = writeln!(text, "In this app: {}.", states.join(", "));
    }
    for spec in SPECS {
        let _ = write!(text, "\n{}", component_section(spec, enabled));
    }
    text
}

fn state(feature: &str, enabled: &[String]) -> &'static str {
    if enabled.iter().any(|on| on == feature) {
        "on"
    } else {
        "off"
    }
}

/// `enabled` is asked only for a component behind a feature, as reading it runs Cargo.
pub fn component(name: &str, enabled: impl FnOnce() -> Option<Vec<String>>) -> Option<String> {
    let spec = SPECS.iter().find(|spec| spec.name == name)?;
    let enabled = spec.feature.and_then(|_| enabled());
    Some(component_section(spec, enabled.as_deref()))
}

fn component_section(spec: &ComponentSpec, enabled: Option<&[String]>) -> String {
    let mut text = format!("component {}: {}\n", spec.name, spec.summary);
    if let Some(feature) = spec.feature {
        let in_app = match enabled.map(|enabled| state(feature, enabled)) {
            Some("on") => ", on in this app".to_owned(),
            Some(_) => format!(
                ", off in this app: `sofie feature add {feature}`, or the MCP tool `feature`, \
                 turns it on"
            ),
            None => String::new(),
        };
        let _ = writeln!(
            text,
            "  feature: `{feature}` (sofie's feature of the same name{in_app})"
        );
    }
    let _ = writeln!(text, "  children: {}", children(spec.children));
    for prop in spec.props {
        let need = if prop.required {
            "required"
        } else {
            "optional"
        };
        let _ = writeln!(
            text,
            "  {} ({need}, {}): {}",
            prop.name,
            prop.kind.describe(),
            prop.summary
        );
    }
    text
}

fn children(children: Children) -> &'static str {
    match children {
        Children::None => "none; write it as `<Name … />`",
        Children::Any => "any markup and components",
        Children::FormParts => "`Field` and `Submit`, in the order shown",
        Children::RowContent => "rendered in a last cell of every row, the row named by `as`",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_component_names_its_props_and_what_each_takes() {
        let table = component("Table", || unreachable!("`Table` needs no feature")).unwrap();

        assert!(table.starts_with("component Table: "), "{table}");
        assert!(
            table.contains("  of (required, a data path such as `{data.items}`): "),
            "{table}"
        );
        assert!(
            table.contains("  children: rendered in a last cell"),
            "{table}"
        );
        assert_eq!(component("Nothing", || None), None);
    }

    #[test]
    fn the_list_holds_every_component() {
        let list = component_list(None);

        for spec in SPECS {
            assert!(
                list.contains(&format!("component {}: ", spec.name)),
                "{}",
                spec.name
            );
        }
    }

    #[test]
    fn a_feature_reads_as_on_or_off_when_the_app_is_known() {
        let enabled = || Some(vec!["engine".to_owned(), "i18n".to_owned()]);

        let badge = component("Badge", enabled).unwrap();
        let card = component("Card", enabled).unwrap();
        let unknown = component("Card", || None).unwrap();

        assert!(badge.contains("same name, on in this app)"), "{badge}");
        assert!(
            card.contains("off in this app: `sofie feature add layout`, or the MCP tool `feature`"),
            "{card}"
        );
        assert!(
            unknown.contains("(sofie's feature of the same name)\n"),
            "{unknown}"
        );
    }

    #[test]
    fn table_documents_its_row_link() {
        let table = component("Table", || None).unwrap();

        assert!(table.contains("  link (optional, "), "{table}");
    }
}
