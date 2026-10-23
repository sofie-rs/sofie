pub(crate) mod factory;

use sofie::prelude::*;

pub(crate) const NAME_TAKEN: &str = "is already a role";

#[derive(Aggregate)]
pub struct Role {
    meta: Meta,
    name: String,
}

pub type RoleId = Id<Role>;
