pub(crate) mod factory;

use sofie::prelude::*;

use crate::role::RoleId;

/// Revoking deletes softly, keeping who had what.
#[derive(Aggregate)]
#[aggregate(behaviors(SoftDelete))]
pub struct RoleGrant {
    meta: Meta,
    role: RoleId,
    permission: String,
}

pub(crate) async fn granted(read: &Read, role: RoleId) -> Result<Vec<String>> {
    let grants = RoleGrant::query()
        .where_(RoleGrant::ROLE.eq(role))
        .all(read)
        .await?;
    Ok(grants.into_iter().map(|grant| grant.permission).collect())
}
