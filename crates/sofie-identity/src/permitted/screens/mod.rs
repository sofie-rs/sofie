#[cfg(feature = "permitted-flags")]
mod flags;
#[cfg(feature = "permitted-flags")]
mod settings;
#[cfg(feature = "permitted-shield")]
mod shield;

use sofie::http::{Area, Groups, Resource, Resources, Routes};

use super::Permitted;
use crate::{Role, RoleGrant};

/// Each route needs its `sofie_identity.<model>.*.<action>` permission.
pub fn permitted_routes<A: Area<Principal = Permitted>>(routes: &mut Routes<A>) {
    routes.resource::<Role>();
    routes.group("/roles/{role}", |routes| {
        routes
            .resource::<RoleGrant>()
            .only([Resource::Index, Resource::Create, Resource::Delete]);
    });
    #[cfg(feature = "permitted-flags")]
    flags::routes(routes);
    #[cfg(feature = "permitted-flags")]
    settings::routes(routes);
    #[cfg(feature = "permitted-shield")]
    shield::routes(routes);
}
