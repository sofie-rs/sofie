mod date;
mod density;
mod god_type;
mod lines;
mod rules;
mod screens;
mod structure;
mod workspace;

pub use date::Date;
pub use density::CommentDensity;
pub use god_type::{GodType, rank_god_types};
pub use rules::{
    ALLOW_FILE, AllowList, Finding, MAX_UNUSED_PUBLIC_RATIO, MigratedSchema, Rule, check,
    check_client_feature, check_dead_code, check_flag_ages, missing_gen_exports, report,
    sort_findings,
};
pub use screens::{Screen, Screens};
pub use structure::{MAX_COMMENT_RATIO, MAX_GOD_TYPE_SCORE, check_structure};
pub use workspace::{CrateSource, load_workspace_crates};

#[expect(
    clippy::cast_precision_loss,
    reason = "line and method counts stay far below 2^52"
)]
fn count_as_f64(count: usize) -> f64 {
    count as f64
}
