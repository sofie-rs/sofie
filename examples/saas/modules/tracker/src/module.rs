use sofie::{Module, Tier};

pub const MODULE: Module = Module {
    name: "tracker",
    tier: Tier::Core,
    context: None,
    settings: &[],
    ..crate::wiring::GENERATED
};
