use std::path::Path;

use sofie_model::ModelError;

use crate::rules::{Finding, Rule};
use crate::workspace::CrateSource;
use crate::{CommentDensity, GodType, load_workspace_crates, rank_god_types};

pub const MAX_COMMENT_RATIO: f64 = 0.0286;

/// A tiny crate's few doc lines would otherwise dominate its ratio.
pub const FREE_COMMENT_LINES: usize = 3;

/// Lower it when the worst type is split; never raise it.
pub const MAX_GOD_TYPE_SCORE: f64 = 3.29;

pub fn check_structure(root: &Path) -> Result<Vec<Finding>, ModelError> {
    let crates = load_workspace_crates(root)?;
    let mut findings: Vec<Finding> = crates
        .iter()
        .filter_map(|krate| density_finding(root, krate))
        .collect();
    findings.extend(
        rank_god_types(&crates)
            .iter()
            .take_while(|god_type| god_type.score() > MAX_GOD_TYPE_SCORE)
            .map(|god_type| god_type_finding(root, god_type)),
    );
    Ok(findings)
}

fn density_finding(root: &Path, krate: &CrateSource) -> Option<Finding> {
    let density = CommentDensity::of_crate(krate);
    if density.within_limit() {
        return None;
    }
    Some(Finding {
        rule: Rule::CommentDensity,
        file: root.join(&krate.name).join("Cargo.toml"),
        line: 1,
        message: format!(
            "`{}` has comments on more than {MAX_COMMENT_RATIO} of its code lines ({density}); \
             comments that repeat the code go stale",
            krate.name
        ),
        fix: "keep comments that say why; say what through names and smaller functions".to_owned(),
    })
}

/// A type is ranked only for an impl found in a file, so `files` is never empty.
fn god_type_finding(root: &Path, god_type: &GodType) -> Finding {
    Finding {
        rule: Rule::GodType,
        file: root.join(&god_type.files[0]),
        line: 1,
        message: format!("a type outgrew the ceiling {MAX_GOD_TYPE_SCORE}: {god_type}"),
        fix: "split the type by responsibility".to_owned(),
    }
}
