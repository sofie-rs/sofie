use sofie::http::{CurrentSession, RequestContext};
use sofie::prelude::*;
use sofie_identity::AccountId;

use crate::organization::{OrganizationArea, OrganizationMember, OrganizationRole, membership};
use sofie::db::{Context, Resolve};

const GONE: &str = "You are no longer a member of this organization.";

/// Read on every request, so a removed member or a changed role counts at once.
pub struct Acting {
    member: OrganizationMember,
}

impl Acting {
    #[must_use]
    pub fn member(&self) -> &OrganizationMember {
        &self.member
    }

    #[must_use]
    pub fn role(&self) -> OrganizationRole {
        self.member.role()
    }

    #[must_use]
    pub fn into_member(self) -> OrganizationMember {
        self.member
    }

    pub(crate) fn owning(&self) -> Result<()> {
        if self.role() != OrganizationRole::Owner {
            return Err(Error::Rule(ONLY_AN_OWNER.to_owned()));
        }
        Ok(())
    }
}

const ONLY_AN_OWNER: &str = "Only an owner renames, hands over or deletes the organization.";

impl<O: OrganizationArea, M> Resolve<RequestContext<O, M>> for Acting {
    async fn resolve(context: &mut RequestContext<O, M>) -> Result<Self> {
        let organization = Principal::<O>::resolve(context).await?.id();
        let current = CurrentSession::resolve(context).await?;
        let session = current.id().ok_or(Error::NotFound)?;
        let unscoped = context.db().read();
        let account = match current.account() {
            Some(account) => AccountId::from_uuid(account),
            None => sofie_identity::sessions::account_of(&unscoped, session)
                .await?
                .ok_or_else(|| Error::Rule(GONE.to_owned()))?,
        };
        let member = membership(&unscoped, organization, account)
            .await?
            .ok_or_else(|| Error::Rule(GONE.to_owned()))?;
        Ok(Self { member })
    }
}
