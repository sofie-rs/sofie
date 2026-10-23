use std::path::Path;

use sofie_check::{
    AllowList, CommentDensity, CrateSource, GodType, MAX_COMMENT_RATIO, MAX_GOD_TYPE_SCORE,
    MAX_UNUSED_PUBLIC_RATIO, check_dead_code, load_workspace_crates, rank_god_types, report,
};

/// Slack above the worst score would let a later type grow unnoticed.
const MAX_CEILING_SLACK: f64 = 0.5;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("sofie-check lives at <workspace>/crates/sofie-check")
}

fn workspace_crates() -> Vec<CrateSource> {
    load_workspace_crates(workspace_root()).expect("workspace sources read and parse")
}

fn worst_god_type() -> Option<GodType> {
    rank_god_types(&workspace_crates()).into_iter().next()
}

fn describe(worst: Option<&GodType>) -> String {
    worst.map_or_else(
        || "no type has an inherent impl".to_owned(),
        ToString::to_string,
    )
}

#[test]
fn every_crate_keeps_comment_lines_within_the_limit() {
    let offenders: Vec<String> = workspace_crates()
        .iter()
        .map(|crate_source| {
            (
                crate_source.name.as_str(),
                CommentDensity::of_crate(crate_source),
            )
        })
        .filter(|(_, density)| !density.within_limit())
        .map(|(name, density)| format!("  {name}: {density}"))
        .collect();
    assert!(
        offenders.is_empty(),
        "comment lines exceed {MAX_COMMENT_RATIO} of code lines in:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn no_type_outgrows_the_god_type_ceiling() {
    let worst = worst_god_type();
    let worst_score = worst.as_ref().map_or(0.0, GodType::score);
    assert!(
        worst_score <= MAX_GOD_TYPE_SCORE,
        "worst god type exceeds MAX_GOD_TYPE_SCORE {MAX_GOD_TYPE_SCORE}: {}. Split the type; \
         lower the ceiling when the worst type is split; never raise it",
        describe(worst.as_ref())
    );
}

#[test]
fn the_god_type_ceiling_has_no_slack() {
    let worst = worst_god_type();
    let worst_score = worst.as_ref().map_or(0.0, GodType::score);
    assert!(
        MAX_GOD_TYPE_SCORE - worst_score < MAX_CEILING_SLACK,
        "MAX_GOD_TYPE_SCORE {MAX_GOD_TYPE_SCORE} is {MAX_CEILING_SLACK} or more above the worst god type: {}. \
         Lower the ceiling when the worst type is split; never raise it",
        describe(worst.as_ref())
    );
}

#[test]
fn no_crate_keeps_dead_code() {
    let root = workspace_root();
    let crates = sofie_model::workspace_crates(&root.join("Cargo.toml"))
        .expect("the workspace reads and parses");
    let allowed = AllowList::read(root).expect("dead-code-allow.toml reads");
    let checked: Vec<&sofie_model::Crate> = crates.iter().collect();

    let findings = check_dead_code(&checked, &[], &allowed, MAX_UNUSED_PUBLIC_RATIO)
        .expect("every manifest reads");

    assert!(findings.is_empty(), "{}", report(root, &findings));
}
