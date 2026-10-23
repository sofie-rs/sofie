use sofie::{Module, Tier};

pub const MODULE: Module = Module {
    name: "board",
    tier: Tier::Core,
    context: None,
    settings: &[],
    ..crate::wiring::GENERATED
};
