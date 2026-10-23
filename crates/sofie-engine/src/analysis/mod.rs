mod bindings;
mod ids;

pub(crate) use self::bindings::Recorder;
pub use self::bindings::{
    ActionPost, Binding, TableLink, action_posts_with, bindings, bindings_with, table_links_with,
};
#[cfg(feature = "gates")]
pub use self::bindings::{AskedEntitlement, asked_entitlements};
pub use self::ids::{FormPost, IdPattern};
pub use crate::slots::Step;
