use sofie::{Module, Tier};

pub const MODULE: Module = Module {
    name: "notes",
    tier: Tier::Core,
    context: None,
    settings: &[],
    ..crate::wiring::GENERATED
};
