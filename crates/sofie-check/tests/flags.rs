use std::path::PathBuf;

use sofie_check::{Date, Finding, Rule, Screens, check_flag_ages};
use sofie_model::testing::{app, area, module};
use sofie_model::{App, FeatureDeclaration, FeatureGate, MenuEntry, MenuFile, Module};

fn check(app: &App) -> Vec<Finding> {
    sofie_check::check(app, &Screens::read(&app.modules))
}

fn declaration(constant: &str, name: Option<&str>, line: usize) -> FeatureDeclaration {
    FeatureDeclaration {
        constant: vec!["module".to_owned(), constant.to_owned()],
        name: name.map(str::to_owned),
        added: Some("2026-09-01".to_owned()),
        retire_after_days: 30,
        variants: false,
        file: PathBuf::from("/shop/modules/order/src/module.rs"),
        line,
    }
}

fn order_with(declarations: Vec<FeatureDeclaration>, gates: Vec<FeatureGate>) -> Module {
    let mut order = module("order", "/shop/modules/order");
    order.code.features.declarations = declarations;
    order.code.features.gates = gates;
    order
}

fn gate(feature: &str) -> FeatureGate {
    FeatureGate {
        function: vec!["listeners".to_owned(), "audit".to_owned()],
        feature: vec![feature.to_owned()],
        file: PathBuf::from("/shop/modules/order/src/listeners.rs"),
        line: 9,
    }
}

fn flag_findings(app: &App) -> Vec<(Rule, usize)> {
    check(app)
        .into_iter()
        .filter(|finding| finding.rule == Rule::FeatureFlag)
        .map(|finding| (finding.rule, finding.line))
        .collect()
}

#[test]
fn a_well_named_flag_and_its_gate_pass() {
    let order = order_with(
        vec![declaration("SPLIT", Some("order.split"), 3)],
        vec![gate("SPLIT")],
    );
    assert_eq!(flag_findings(&app("/shop", vec![order])), []);
}

#[test]
fn a_flag_of_another_module_or_without_a_literal_name_is_reported() {
    let order = order_with(
        vec![
            declaration("SPLIT", Some("payment.split"), 3),
            declaration("COMPUTED", None, 4),
            declaration("NESTED", Some("order.a.b"), 5),
        ],
        Vec::new(),
    );
    let lines: Vec<usize> = flag_findings(&app("/shop", vec![order]))
        .into_iter()
        .map(|(_, line)| line)
        .collect();
    assert_eq!(lines, [3, 4, 5]);
}

#[test]
fn a_flag_declared_twice_and_a_gate_naming_no_flag_are_reported() {
    let order = order_with(
        vec![
            declaration("SPLIT", Some("order.split"), 3),
            declaration("ALSO_SPLIT", Some("order.split"), 4),
        ],
        vec![gate("MISSING")],
    );
    assert_eq!(
        flag_findings(&app("/shop", vec![order])),
        [(Rule::FeatureFlag, 9), (Rule::FeatureFlag, 4)]
    );
}

#[test]
fn a_flag_declared_outside_the_manifest_is_reported() {
    let mut elsewhere = declaration("SPLIT", Some("order.split"), 6);
    elsewhere.file = PathBuf::from("/shop/modules/order/src/flags.rs");
    let order = order_with(vec![elsewhere], Vec::new());
    let findings = check(&app("/shop", vec![order]));
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(
        findings[0]
            .message
            .contains("outside the module's manifest"),
        "{}",
        findings[0].message
    );
}

#[test]
fn a_flag_older_than_its_declared_age_is_reported_with_its_file_and_line() {
    let mut younger = declaration("LAYOUT", Some("order.layout"), 4);
    younger.added = Some("2026-09-02".to_owned());
    let order = order_with(
        vec![declaration("SPLIT", Some("order.split"), 3), younger],
        Vec::new(),
    );
    let today = Date::parse("2026-10-02").unwrap();
    let findings = check_flag_ages(&app("/shop", vec![order]), today);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].rule, Rule::FlagAge);
    assert_eq!(
        findings[0].file,
        PathBuf::from("/shop/modules/order/src/module.rs")
    );
    assert_eq!(findings[0].line, 3);
    assert!(
        findings[0]
            .message
            .contains("added 31 days ago, past the 30"),
        "{}",
        findings[0].message
    );
}

#[test]
fn a_flag_without_a_readable_day_it_was_added_is_reported() {
    let mut undated = declaration("SPLIT", Some("order.split"), 3);
    undated.added = None;
    let mut misdated = declaration("LAYOUT", Some("order.layout"), 4);
    misdated.added = Some("2026-02-30".to_owned());
    let order = order_with(vec![undated, misdated], Vec::new());
    let lines: Vec<usize> = check(&app("/shop", vec![order]))
        .into_iter()
        .filter(|finding| finding.rule == Rule::FlagAge)
        .map(|finding| finding.line)
        .collect();
    assert_eq!(lines, [3, 4]);
}

#[test]
fn a_menu_entry_naming_no_flag_and_an_unreachable_menu_are_reported() {
    let mut order = order_with(
        vec![declaration("SPLIT", Some("order.split"), 3)],
        Vec::new(),
    );
    let entry = |label: &str, feature: &str, line| MenuEntry {
        label: label.to_owned(),
        feature: Some(feature.to_owned()),
        line,
    };
    let mut console = area(&order, "console", Vec::new());
    console.menu = Some(MenuFile {
        file: PathBuf::from("/shop/modules/order/src/areas/console/menu.rs"),
        declared: false,
        entries: vec![
            entry("Split", "crate::module::SPLIT", 2),
            entry("Beta", "BETA", 3),
        ],
    });
    order.areas.push(console);

    let findings = check(&app("/shop", vec![order]));

    let reported: Vec<(&str, usize)> = findings
        .iter()
        .filter(|finding| finding.file.ends_with("menu.rs"))
        .map(|finding| (finding.rule.name(), finding.line))
        .collect();
    assert_eq!(reported, [("undeclared-routes", 1), ("feature-flag", 3)]);
}
