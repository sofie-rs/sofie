use sofie::db::{Outcome, Plan, Stamp};
use sofie::prelude::*;

use super::{Membership, factory, held};
use crate::account::{ALREADY_REGISTERED, Account, Distrusted, EMAIL_INDEX};

const MEMBERSHIP_INDEX: &str = "membership_account_area";
const MEMBER_ALREADY: &str = "is a member already";

/// An account made a member of an area from a task or a seed, as the first staff member of a
/// `Permitted` area is, whose sign-up admits nobody to anything.
pub struct MemberAdded {
    membership: Membership,
    account: Adopted,
}

enum Adopted {
    Created(Account),
    Distrusted(Account, Distrusted),
    Kept,
}

impl MemberAdded {
    #[must_use]
    pub fn account(&self) -> crate::AccountId {
        self.membership.account
    }

    #[cfg(feature = "permissions")]
    pub async fn with_role(
        mut self,
        read: &Read,
        clock: &Clock,
        role: crate::RoleId,
    ) -> Result<Self> {
        if matches!(self.account, Adopted::Kept)
            && super::roles::gives(&self.membership, Some(role))
            && let Some((account, distrusted)) =
                super::roles::unproven(read, self.membership.account, clock).await?
        {
            self.account = Adopted::Distrusted(account, distrusted);
        }
        self.membership.role = Some(role);
        Ok(self)
    }
}

/// An address no account holds gets one without a password, which signs in once its owner
/// sets one (`password-reset`), follows a sign-in link or connects a provider. An account
/// already a member keeps its membership, so adding it again changes nothing but a role.
/// An account adopted, or given a role, whose address nobody proved may have been signed up
/// by someone else expecting this, so it is distrusted.
pub async fn add_member(
    read: &Read,
    clock: &Clock,
    email: Email,
    area: &str,
) -> Result<MemberAdded> {
    if area.trim().is_empty() {
        return Err(Error::invalid("area", "is required"));
    }
    let Some(mut existing) = Account::of_email(read, email.clone()).await? else {
        let account = crate::account::factory::without_password(email)?;
        let membership = factory::grant(account.id(), area)?;
        return Ok(MemberAdded {
            membership,
            account: Adopted::Created(account),
        });
    };
    if let Some(membership) = held(read, existing.id(), area).await? {
        return Ok(MemberAdded {
            membership,
            account: Adopted::Kept,
        });
    }
    let membership = factory::grant(existing.id(), area)?;
    if existing.is_confirmed() {
        return Ok(MemberAdded {
            membership,
            account: Adopted::Kept,
        });
    }
    let distrusted = existing.distrust(read, clock.now()).await?;
    Ok(MemberAdded {
        membership,
        account: Adopted::Distrusted(existing, distrusted),
    })
}

#[derive(Input)]
pub struct AddingMember {
    email: Email,
    #[max(64)]
    area: String,
    #[max(200)]
    role: Option<String>,
}

/// For an app's tasks: `Tasks::new().task("sofie_identity.add_member", add_member_task)`,
/// then `<app> task sofie_identity.add_member --email … --area … [--role …]`.
pub async fn add_member_task(
    adding: AddingMember,
    read: Read,
    clock: Clock,
) -> Result<MemberAdded> {
    let added = add_member(&read, &clock, adding.email, &adding.area).await?;
    let Some(role) = adding.role else {
        return Ok(added);
    };
    #[cfg(feature = "permissions")]
    return added
        .with_role(&read, &clock, crate::role_named(&read, &role).await?)
        .await;
    #[cfg(not(feature = "permissions"))]
    {
        let _ = (added, role);
        Err(Error::invalid(
            "role",
            "needs sofie-identity's `permissions`",
        ))
    }
}

impl Outcome for MemberAdded {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        match &self.account {
            Adopted::Created(account) => created(plan, account)?,
            Adopted::Distrusted(account, distrusted) => {
                created(plan, account)?;
                distrusted.plan(plan)?;
            }
            Adopted::Kept => {}
        }
        plan.unique(MEMBERSHIP_INDEX, "area", MEMBER_ALREADY);
        self.membership.plan(plan)
    }

    fn committed(&mut self, stamp: Stamp) {
        match &mut self.account {
            Adopted::Created(account) => account.committed(stamp),
            Adopted::Distrusted(account, distrusted) => {
                account.committed(stamp);
                distrusted.committed(stamp);
            }
            Adopted::Kept => {}
        }
        self.membership.committed(stamp);
    }
}

fn created(plan: &mut Plan<'_>, account: &Account) -> Result<()> {
    plan.unique(EMAIL_INDEX, "email", ALREADY_REGISTERED);
    account.plan(plan)
}
