#[cfg(feature = "permitted-screens")]
mod screens;

use sofie::http::{Access, AreaPrincipal};

#[cfg(feature = "permitted-screens")]
pub use self::screens::permitted_routes;
use crate::Account;
pub use crate::membership::{
    Membership,
    roles::{RoleAssigned, assign_role},
};
pub use crate::role::{Role, RoleId, factory::create_role, factory::role_named};
pub use crate::role_grant::RoleGrant;
pub use crate::role_grant::factory::{grant, grant_as, revoke};

/// Its area's routes reach every record, each only with its permission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Permitted;

impl AreaPrincipal for Permitted {
    const ACCESS: Access = Access::Permitted;
    type Record = Account;
}
