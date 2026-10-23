use sofie::db::{Outcome, Plan, Stamp};
use sofie::prelude::*;

use super::{Membership, held};
use crate::AccountId;
use crate::account::{Account, Distrusted};
use crate::role::RoleId;

/// A role given to an account whose address nobody proved, as `add_member` adopting one,
/// distrusts it.
pub struct RoleAssigned {
    membership: Membership,
    distrusted: Option<(Account, Distrusted)>,
}

pub async fn assign_role(
    read: &Read,
    clock: &Clock,
    account: AccountId,
    area: &str,
    role: Option<RoleId>,
) -> Result<RoleAssigned> {
    let mut membership = held(read, account, area).await?.ok_or(Error::NotFound)?;
    let distrusted = if gives(&membership, role) {
        unproven(read, account, clock).await?
    } else {
        None
    };
    membership.role = role;
    Ok(RoleAssigned {
        membership,
        distrusted,
    })
}

pub(super) fn gives(membership: &Membership, role: Option<RoleId>) -> bool {
    role.is_some() && membership.role != role
}

pub(super) async fn unproven(
    read: &Read,
    account: AccountId,
    clock: &Clock,
) -> Result<Option<(Account, Distrusted)>> {
    let mut account = Account::stored(read, account).await?;
    if account.is_confirmed() {
        return Ok(None);
    }
    let distrusted = account.distrust(read, clock.now()).await?;
    Ok(Some((account, distrusted)))
}

impl Outcome for RoleAssigned {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        self.membership.plan(plan)?;
        self.distrusted.plan(plan)
    }

    fn committed(&mut self, stamp: Stamp) {
        self.membership.committed(stamp);
        self.distrusted.committed(stamp);
    }
}

pub(crate) async fn granted(read: &Read, area: &str, account: AccountId) -> Result<Vec<String>> {
    let role = held(read, account, area)
        .await?
        .and_then(|membership| membership.role);
    match role {
        Some(role) => crate::role_grant::granted(read, role).await,
        None => Ok(Vec::new()),
    }
}
