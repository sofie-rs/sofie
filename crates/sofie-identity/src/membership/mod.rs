mod adding;
pub(crate) mod factory;
#[cfg(feature = "permissions")]
pub(crate) mod roles;

use sofie::core::SqlValue;
use sofie::db::{Outcome, Plan, Reach, Step};
use sofie::prelude::*;

pub use adding::{AddingMember, MemberAdded, add_member, add_member_task};

use crate::AccountId;

#[cfg(any(
    feature = "sign-in-links",
    feature = "totp",
    feature = "passkeys",
    feature = "oauth"
))]
pub(crate) const NOT_A_MEMBER: &str = "This account may not sign in here.";

#[derive(Aggregate)]
pub struct Membership {
    meta: Meta,
    #[owner]
    account: AccountId,
    area: String,
    #[cfg(feature = "permissions")]
    role: Option<crate::role::RoleId>,
}

#[cfg(feature = "account-data")]
impl Membership {
    pub(crate) fn area(&self) -> &str {
        &self.area
    }
}

pub struct Memberships;

impl sofie::http::AreaMembers for Memberships {
    /// Read per request, so a revoked grant stops the next one.
    #[cfg(feature = "permissions")]
    fn grants<'a>(
        &'a self,
        read: &'a Read,
        area: &'static str,
        principal: sofie::db::Uuid,
    ) -> sofie::db::BoxFuture<'a, Result<Vec<String>>> {
        Box::pin(roles::granted(read, area, AccountId::from_uuid(principal)))
    }

    #[cfg(feature = "tokens")]
    fn bearer<'a>(
        &'a self,
        db: &'a sofie::db::Db,
        area: &'static str,
        digest: &'a str,
    ) -> sofie::db::BoxFuture<'a, Result<Option<sofie::http::Bearer>>> {
        Box::pin(crate::tokens::bearer(db, area, digest))
    }

    #[cfg(feature = "shield")]
    fn sign_in_locked<'a>(
        &'a self,
        db: &'a sofie::db::Db,
        account: sofie::db::Uuid,
        until: sofie::core::Timestamp,
    ) -> sofie::db::BoxFuture<'a, Result<()>> {
        Box::pin(crate::lock::tell_owner(db, account, until))
    }

    #[cfg(feature = "sessions")]
    fn session_record<'a>(
        &'a self,
        db: &'a sofie::db::Db,
        session: sofie::db::Uuid,
        seen: Option<sofie::core::Timestamp>,
    ) -> sofie::db::BoxFuture<'a, Result<sofie::http::SessionRecord>> {
        Box::pin(crate::sessions::record(db, session, seen))
    }

    #[cfg(feature = "sessions")]
    fn session_topic(&self) -> Option<sofie::db::Topic> {
        Some(sofie::db::Topic::of::<crate::sessions::AccountSession>())
    }
}

/// A membership removed, and with `sessions` the account's sessions in its area ended in
/// the same commit, so a cookie open there stops at its next request.
pub struct MembershipRemoved {
    membership: Membership,
    #[cfg(feature = "sessions")]
    ended: Vec<crate::sessions::AccountSession>,
}

/// Read and committed beyond the request's reach, as an area's principal seldom owns its
/// members' records. Without `sessions`, a session open in the area stays open until its
/// cookie expires.
pub async fn remove_membership(
    read: &Read,
    account: AccountId,
    area: &str,
    now: sofie::core::Instant,
) -> Result<MembershipRemoved> {
    let membership = held(read, account, area).await?.ok_or(Error::NotFound)?;
    #[cfg(not(feature = "sessions"))]
    let _ = now;
    Ok(MembershipRemoved {
        membership,
        #[cfg(feature = "sessions")]
        ended: crate::sessions::ending_in(read, account, area, now).await?,
    })
}

impl Outcome for MembershipRemoved {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        if let Reach::OwnedBy(owner) = plan.reach()
            && owner != self.membership.account.uuid()
        {
            return Err(Error::NotFound);
        }
        let dialect = plan.dialect();
        let sql = format!(
            "DELETE FROM {} WHERE {} = {}",
            dialect.qualified(Membership::MODULE, Membership::NAME),
            dialect.quote("id"),
            dialect.placeholder(1)
        );
        let id = SqlValue::Uuid(self.membership.id().uuid());
        plan.push(Step::statement(sql, vec![id], Some(1)));
        #[cfg(feature = "sessions")]
        self.ended.plan(plan)?;
        Ok(())
    }

    #[cfg(feature = "sessions")]
    fn committed(&mut self, stamp: sofie::db::Stamp) {
        self.ended.committed(stamp);
    }
}

pub(crate) async fn held(
    read: &Read,
    account: AccountId,
    area: &str,
) -> Result<Option<Membership>> {
    Membership::query()
        .where_(Membership::ACCOUNT.eq(account))
        .where_(Membership::AREA.eq(area.to_owned()))
        .first(read)
        .await
}

/// For a way of signing in that proved the account but not its membership.
#[cfg(any(
    feature = "sign-in-links",
    feature = "totp",
    feature = "passkeys",
    feature = "oauth"
))]
pub(crate) async fn require(read: &Read, account: AccountId, area: &str) -> Result<()> {
    held(read, account, area)
        .await?
        .map(drop)
        .ok_or_else(|| Error::Rule(NOT_A_MEMBER.to_owned()))
}
