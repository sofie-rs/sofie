use sofie::flags::{Feature, Rule};
use sofie::{Module, Tier};

pub const REOPENING: Feature = Feature::new("tracker.reopening")
    .added("2026-11-27")
    .default(Rule::everyone);

pub const MODULE: Module = Module {
    name: "tracker",
    tier: Tier::Core,
    context: None,
    settings: &[],
    ..crate::wiring::GENERATED
};
