#[cfg(feature = "groups")]
mod group;

#[cfg(feature = "groups")]
pub use group::Groups;
#[cfg(feature = "listing")]
pub(crate) mod listing;
#[cfg(feature = "menu")]
pub(crate) mod menu;
#[cfg(feature = "resources")]
pub(crate) mod resource;

#[cfg(feature = "listing")]
pub use listing::{Listed, Listing};
#[cfg(feature = "menu")]
pub use menu::{Menu, MenuEntry, Navigation};
#[cfg(feature = "resources")]
pub use resource::{
    Describe, OwnerOf, Resource, ResourceRoutes, Resources, Shipped, built_in_screen_source,
};

/// Once every route's gates are known.
#[cfg(any(feature = "resources", feature = "menu"))]
pub(crate) fn prepare(app: &mut crate::App) -> sofie_core::Result<()> {
    #[cfg(feature = "resources")]
    resource::install(&mut app.screens, &app.routes)?;
    #[cfg(feature = "menu")]
    menu::prepare(app)?;
    Ok(())
}
