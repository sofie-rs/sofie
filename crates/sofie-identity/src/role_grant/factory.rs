use sofie::http::{Permissions, is_permission};
use sofie::prelude::*;

use super::RoleGrant;
use crate::role::RoleId;

/// Idempotent: a grant `role` already holds is returned unchanged. Checks nothing of who
/// grants, so it is for seeds and boot; a handler grants with `grant_as`.
pub async fn grant(read: &Read, role: RoleId, permission: &str) -> Result<RoleGrant> {
    refuse_unless_permission("permission", permission)?;
    if let Some(held) = held(read, role, permission).await? {
        return Ok(held);
    }
    Ok(RoleGrant {
        meta: Meta::new()?,
        role,
        permission: permission.to_owned(),
    })
}

/// Refuses a permission `granter` does not hold itself, so nobody widens a role past its own.
pub async fn grant_as(
    read: &Read,
    granter: &Permissions,
    role: RoleId,
    permission: &str,
) -> Result<RoleGrant> {
    refuse_unless_held(granter, permission)?;
    grant(read, role, permission).await
}

pub async fn revoke(read: &Read, role: RoleId, permission: &str) -> Result<RoleGrant> {
    let mut held = held(read, role, permission).await?.ok_or(Error::NotFound)?;
    held.delete();
    Ok(held)
}

pub(crate) fn refuse_unless_permission(field: &str, text: &str) -> Result<()> {
    if is_permission(text) {
        return Ok(());
    }
    Err(Error::invalid(
        field,
        format!("`{text}` is not a permission such as `order.order.*.cancel`"),
    ))
}

fn refuse_unless_held(granter: &Permissions, permission: &str) -> Result<()> {
    refuse_unless_permission("permission", permission)?;
    if granter.can_named(permission) {
        return Ok(());
    }
    Err(Error::invalid(
        "permission",
        format!("`{permission}` is not covered by your own permissions, so you cannot grant it"),
    ))
}

async fn held(read: &Read, role: RoleId, permission: &str) -> Result<Option<RoleGrant>> {
    RoleGrant::query()
        .where_(RoleGrant::ROLE.eq(role))
        .where_(RoleGrant::PERMISSION.eq(permission.to_owned()))
        .first(read)
        .await
}

#[cfg(feature = "permitted-screens")]
#[derive(Input)]
pub struct Granted {
    role: RoleId,
    #[required]
    #[unique]
    permission: String,
}

/// Whoever posts the grant, read from the request, since the resource checks only the route.
#[cfg(feature = "permitted-screens")]
pub struct Granter(Permissions);

#[cfg(feature = "permitted-screens")]
impl<A: sofie::http::Area> sofie::http::OwnerOf<A> for Granter {
    fn of(context: &sofie::http::RequestContext<A>) -> Result<Self> {
        Ok(Self(context.permissions().clone()))
    }
}

#[cfg(feature = "permitted-screens")]
impl sofie::db::Create for RoleGrant {
    type Input = Granted;
    type Owner = Granter;

    fn create(input: Granted, granter: Granter) -> Result<Self> {
        refuse_unless_held(&granter.0, &input.permission)?;
        let mut grant = RoleGrant {
            meta: Meta::new()?,
            role: input.role,
            permission: String::new(),
        };
        sofie::db::Update::update(&mut grant, input)?;
        Ok(grant)
    }
}

#[cfg(feature = "permitted-screens")]
impl sofie::db::Update for RoleGrant {
    type Input = Granted;

    fn update(&mut self, input: Granted) -> Result<()> {
        refuse_unless_permission("permission", &input.permission)?;
        self.role = input.role;
        self.permission = input.permission;
        Ok(())
    }
}
