use crate::asset::{Asset, JAVASCRIPT};

pub const DEVELOPMENT_SCRIPT: Asset = Asset::new(
    "development",
    "js",
    include_bytes!("../assets/development.js"),
    JAVASCRIPT,
);
