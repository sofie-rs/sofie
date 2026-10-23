use crate::{App, KnownArea};

impl App {
    #[must_use]
    pub fn has_feature_flags(&self) -> bool {
        self.modules
            .iter()
            .any(|module| !module.code.features.declarations.is_empty())
    }

    #[must_use]
    pub fn has_permitted_areas(&self) -> bool {
        self.areas().iter().any(KnownArea::is_permitted)
    }
}
