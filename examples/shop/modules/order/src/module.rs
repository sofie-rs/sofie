use sofie::{Module, Tier};

pub const MODULE: Module = Module {
    name: "order",
    tier: Tier::Core,
    context: Some("commerce"),
    settings: &[],
    ..crate::wiring::GENERATED
};
