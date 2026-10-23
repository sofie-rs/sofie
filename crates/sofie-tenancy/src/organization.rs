use sofie::core::{Cast, SqlValue};
use sofie::db::{Aggregate, ExpectedRows, Outcome, Plan, Reply, Stamp, Step, Uuid};
use sofie::http::Area;
use sofie::prelude::*;

use sofie_identity::AccountId;

/// Owns itself, as an account does, so its area reaches it and what it owns. Deleted softly,
/// so the records its members' histories name stay.
#[derive(Aggregate)]
#[owner]
#[aggregate(behaviors(SoftDelete))]
pub struct Organization {
    meta: Meta,
    name: String,
}

pub type OrganizationId = Id<Organization>;

impl Organization {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn named(name: &str) -> Result<Self> {
        Ok(Self {
            meta: Meta::new()?,
            name: valid_name(name)?,
        })
    }

    pub(crate) fn rename(&mut self, name: &str) -> Result<()> {
        self.name = valid_name(name)?;
        Ok(())
    }

    pub(crate) fn dissolve(&mut self) -> Result<()> {
        self.delete();
        self.meta.raise(OrganizationDeleted {
            organization: self.id().uuid(),
        })
    }
}

fn valid_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty()
        || name.chars().count() > 100
        || name.chars().any(sofie::core::misleads_display)
    {
        return Err(Error::invalid(
            "name",
            "is 1 to 100 characters, none hidden",
        ));
    }
    Ok(name.to_owned())
}

/// Raised when an owner deletes the organization, after every member left. What it owns in
/// other modules stays until they erase it, listening with `#[on]`.
#[derive(Event)]
pub struct OrganizationDeleted {
    pub organization: Uuid,
}

pub trait OrganizationArea: Area<Principal = Organization> {
    const HOME: &'static str;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Enum)]
pub enum OrganizationRole {
    /// Everything, the owners' own roles included; an organization keeps at least one.
    Owner,
    /// Invites, changes roles and removes members, other than owners.
    Admin,
    Member,
}

impl OrganizationRole {
    #[must_use]
    pub fn manages_members(self) -> bool {
        matches!(self, Self::Owner | Self::Admin)
    }

    pub(crate) fn managing(self, touching_owner: bool) -> Result<()> {
        if !self.manages_members() {
            return Err(Error::Rule(
                "Only an owner or an admin of the organization manages its members.".to_owned(),
            ));
        }
        if touching_owner && self != Self::Owner {
            return Err(Error::Rule(
                "Only an owner gives or takes the owner role.".to_owned(),
            ));
        }
        Ok(())
    }

    pub(crate) fn outranks(self, other: Self) -> bool {
        let rank = |role: Self| match role {
            Self::Owner => 2,
            Self::Admin => 1,
            Self::Member => 0,
        };
        rank(self) > rank(other)
    }
}

/// Raised when an account joins an organization, by founding it or by accepting an invitation.
#[derive(Event)]
pub struct MemberJoined {
    pub organization: Uuid,
    pub account: Uuid,
    pub role: OrganizationRole,
}

#[derive(Event)]
pub struct MemberRoleChanged {
    pub organization: Uuid,
    pub account: Uuid,
    pub role: OrganizationRole,
}

/// Raised when a member leaves, is removed, or closes its account.
#[derive(Event)]
pub struct MemberLeft {
    pub organization: Uuid,
    pub account: Uuid,
}

/// Removed softly, so the record of who belonged stays.
#[derive(Aggregate)]
#[aggregate(behaviors(SoftDelete))]
pub struct OrganizationMember {
    meta: Meta,
    #[owner]
    organization: OrganizationId,
    account: AccountId,
    role: OrganizationRole,
}

impl OrganizationMember {
    pub(crate) fn new(
        organization: OrganizationId,
        account: AccountId,
        role: OrganizationRole,
    ) -> Result<Self> {
        let mut member = Self {
            meta: Meta::new()?,
            organization,
            account,
            role,
        };
        member.meta.raise(MemberJoined {
            organization: organization.uuid(),
            account: account.uuid(),
            role,
        })?;
        Ok(member)
    }

    #[must_use]
    pub fn organization(&self) -> OrganizationId {
        self.organization
    }

    #[must_use]
    pub fn account(&self) -> AccountId {
        self.account
    }

    #[must_use]
    pub fn role(&self) -> OrganizationRole {
        self.role
    }

    pub(crate) fn assign(&mut self, role: OrganizationRole) -> Result<()> {
        if self.role == role {
            return Ok(());
        }
        self.role = role;
        self.meta.raise(MemberRoleChanged {
            organization: self.organization.uuid(),
            account: self.account.uuid(),
            role,
        })
    }

    pub(crate) fn leave(&mut self) -> Result<()> {
        self.delete();
        self.meta.raise(MemberLeft {
            organization: self.organization.uuid(),
            account: self.account.uuid(),
        })
    }
}

pub(crate) async fn membership(
    read: &Read,
    organization: OrganizationId,
    account: AccountId,
) -> Result<Option<OrganizationMember>> {
    OrganizationMember::query()
        .where_(OrganizationMember::ORGANIZATION.eq(organization))
        .where_(OrganizationMember::ACCOUNT.eq(account))
        .first(read)
        .await
}

pub(crate) enum Kept {
    AnOwner,
    /// For the organization's deletion, which every member leaves.
    Nothing,
    #[cfg(feature = "billing")]
    Seats {
        at_most: u32,
        now: sofie::core::Instant,
    },
}

/// Taking the organization's row first makes concurrent changes of its members count each
/// other, and finds a deletion committed meanwhile; a broken `Kept` rolls back as `Conflict`,
/// and the retried request says why.
/// One more seat for an invitation, or none for one accepted, whose seat its invitation held;
/// refused when the plan's seats are filled, as after the plan shrank.
#[cfg(feature = "billing")]
pub(crate) async fn seats_kept(
    read: &Read,
    billing: &sofie::billing::Billing,
    organization: OrganizationId,
    now: sofie::core::Instant,
    adding: u64,
) -> Result<Kept> {
    let members = OrganizationMember::query()
        .where_(OrganizationMember::ORGANIZATION.eq(organization))
        .count(read)
        .await?;
    let pending = crate::Invitation::query()
        .scope(|query| crate::invitation::pending(query, organization, now))
        .count(read)
        .await?;
    let seats = billing
        .seats(sofie::billing::Subscriber::from(organization))
        .await?;
    let filled = members.saturating_add(pending).saturating_add(adding);
    let filled = u32::try_from(filled).unwrap_or(u32::MAX);
    if !seats.allows(filled) {
        return Err(Error::Rule(SEATS_TAKEN.to_owned()));
    }
    Ok(match seats {
        sofie::billing::Seats::AtMost(at_most) => Kept::Seats { at_most, now },
        sofie::billing::Seats::Unlimited => Kept::AnOwner,
    })
}

/// A checkout started before the deletion may complete after it, leaving a plan nobody can
/// manage or cancel; the provider is told to cancel it.
#[cfg(feature = "billing")]
pub(crate) async fn cancel_deleted_plan(
    changed: sofie::billing::SubscriptionChanged,
    read: Read,
    billing: sofie::billing::Billing,
    key: sofie::db::IdempotencyKey,
) -> Result<()> {
    if changed.status == sofie::billing::Status::Canceled {
        return Ok(());
    }
    let deleted = Organization::query()
        .with_trashed()
        .where_(Organization::ID.eq(OrganizationId::from_uuid(changed.subscriber)))
        .first(&read)
        .await?
        .is_some_and(|organization| organization.is_deleted());
    if !deleted {
        return Ok(());
    }
    billing
        .cancel_at_provider(sofie::billing::Subscriber::new(changed.subscriber), &key)
        .await
}

#[cfg(feature = "billing")]
const SEATS_TAKEN: &str =
    "Every seat of the organization's plan is taken; add seats or remove a member first.";

pub(crate) struct Held<T> {
    organization: OrganizationId,
    kept: Kept,
    change: T,
    invited: Option<Invited>,
    demoted: Option<Demoted>,
}

/// A member whose new role sends fewer invitations than its old one.
pub(crate) struct Demoted {
    pub(crate) account: AccountId,
    pub(crate) role: OrganizationRole,
    pub(crate) now: sofie::core::Instant,
}

pub(crate) struct Invited {
    email: Email,
    now: sofie::core::Instant,
    by: AccountId,
    role: OrganizationRole,
}

impl<T> Held<T> {
    pub(crate) fn new(organization: OrganizationId, kept: Kept, change: T) -> Self {
        Self {
            organization,
            kept,
            change,
            invited: None,
            demoted: None,
        }
    }

    /// An invitation the member sent while its demotion was read is found under the lock.
    pub(crate) fn demoting(mut self, demoted: Demoted) -> Self {
        self.demoted = Some(demoted);
        self
    }

    /// Two invitations of one address at once: the later one finds the first under the lock;
    /// an inviter demoted or removed meanwhile is found there too.
    pub(crate) fn inviting(mut self, invited: Invited) -> Self {
        self.invited = Some(invited);
        self
    }
}

impl<T: Outcome> Outcome for Held<T> {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        plan.push(taken(plan, self.organization, None));
        self.change.plan(plan)?;
        let Some((mut condition, mut params)) = condition(plan, &self.kept) else {
            return Ok(());
        };
        if let Some(invited) = &self.invited {
            condition.push_str(&invited.rule(plan, &mut params));
        }
        if let Some(demoted) = &self.demoted {
            condition.push_str(&demoted.rule(plan, &mut params));
        }
        plan.push(taken(plan, self.organization, Some((condition, params))));
        Ok(())
    }

    fn committed(&mut self, stamp: Stamp) {
        self.change.committed(stamp);
    }

    fn reply(self) -> Result<Reply> {
        self.change.reply()
    }
}

fn taken(
    plan: &Plan<'_>,
    organization: OrganizationId,
    condition: Option<(String, Vec<SqlValue>)>,
) -> Step {
    let dialect = plan.dialect();
    let version = dialect.quote("version");
    let mut sql = format!(
        "UPDATE {} SET {version} = {version} WHERE {} = {} AND {} IS NULL",
        dialect.qualified(Organization::MODULE, <Organization as Aggregate>::NAME),
        dialect.quote("id"),
        dialect.placeholder(1),
        dialect.quote("deleted_at"),
    );
    let mut params = vec![SqlValue::Uuid(organization.uuid())];
    let Some((condition, more)) = condition else {
        return Step::Statement {
            sql,
            params,
            expect_rows: Some(ExpectedRows::or_not_found(1)),
        };
    };
    sql.push_str(" AND ");
    sql.push_str(&condition);
    params.extend(more);
    Step::statement(sql, params, Some(1))
}

impl Demoted {
    fn rule(&self, plan: &Plan<'_>, params: &mut Vec<SqlValue>) -> String {
        let dialect = plan.dialect();
        let first = params.len() + 2;
        params.push(SqlValue::Uuid(self.account.uuid()));
        self.now.encode(params);
        let mut refused = Vec::new();
        for role in [
            OrganizationRole::Owner,
            OrganizationRole::Admin,
            OrganizationRole::Member,
        ] {
            if self.role.managing(role == OrganizationRole::Owner).is_err() {
                role.encode(params);
                refused.push(dialect.placeholder(params.len() + 1));
            }
        }
        if refused.is_empty() {
            return String::new();
        }
        format!(
            " AND NOT EXISTS (SELECT 1 FROM {} WHERE {} = {} AND {} = {} AND {} IS NULL AND {} > {} \
             AND {} IN ({}))",
            dialect.qualified(crate::Invitation::MODULE, crate::Invitation::NAME),
            dialect.quote("organization"),
            dialect.placeholder(1),
            dialect.quote("invited_by"),
            dialect.placeholder(first),
            dialect.quote("accepted_at"),
            dialect.quote("expires_at"),
            dialect.placeholder(first + 1),
            dialect.quote("role"),
            refused.join(", "),
        )
    }
}

impl Invited {
    pub(crate) fn of(invitation: &crate::Invitation, now: sofie::core::Instant) -> Self {
        Self {
            email: invitation.email().clone(),
            now,
            by: invitation.invited_by(),
            role: invitation.role(),
        }
    }

    /// Placeholder 1 is the organization; the rest follow `params`.
    fn rule(&self, plan: &Plan<'_>, params: &mut Vec<SqlValue>) -> String {
        let dialect = plan.dialect();
        let first = params.len() + 2;
        params.push(SqlValue::Text(self.email.as_str().to_owned()));
        self.now.encode(params);
        params.push(SqlValue::Uuid(self.by.uuid()));
        let mut allowed = Vec::new();
        for role in [OrganizationRole::Owner, OrganizationRole::Admin] {
            if role.managing(self.role == OrganizationRole::Owner).is_ok() {
                role.encode(params);
                allowed.push(dialect.placeholder(params.len() + 1));
            }
        }
        format!(
            " AND (SELECT count(*) FROM {} WHERE {} = {} AND {} = {} AND {} IS NULL AND {} > {}) <= 1 \
             AND (SELECT {} FROM {} WHERE {} = {} AND {} = {} AND {} IS NULL) IN ({})",
            dialect.qualified(crate::Invitation::MODULE, crate::Invitation::NAME),
            dialect.quote("organization"),
            dialect.placeholder(1),
            dialect.quote("email"),
            dialect.placeholder(first),
            dialect.quote("accepted_at"),
            dialect.quote("expires_at"),
            dialect.placeholder(first + 1),
            dialect.quote("role"),
            dialect.qualified(OrganizationMember::MODULE, OrganizationMember::NAME),
            dialect.quote("organization"),
            dialect.placeholder(1),
            dialect.quote("account"),
            dialect.placeholder(first + 2),
            dialect.quote("deleted_at"),
            allowed.join(", "),
        )
    }
}

fn condition(plan: &Plan<'_>, kept: &Kept) -> Option<(String, Vec<SqlValue>)> {
    let dialect = plan.dialect();
    let members = format!(
        "SELECT count(*) FROM {} WHERE {} = {} AND {} IS NULL",
        dialect.qualified(OrganizationMember::MODULE, OrganizationMember::NAME),
        dialect.quote("organization"),
        dialect.placeholder(1),
        dialect.quote("deleted_at"),
    );
    match kept {
        Kept::AnOwner => {
            let mut params = Vec::new();
            OrganizationRole::Owner.encode(&mut params);
            let owners = format!(
                "({members} AND {} = {}) > 0",
                dialect.quote("role"),
                dialect.placeholder(2)
            );
            Some((owners, params))
        }
        Kept::Nothing => None,
        #[cfg(feature = "billing")]
        Kept::Seats { at_most, now } => {
            let mut params = Vec::new();
            now.encode(&mut params);
            params.push(SqlValue::Int(i64::from(*at_most)));
            let seated = format!(
                "({members}) + (SELECT count(*) FROM {} WHERE {} = {} AND {} IS NULL AND {} > {}) <= {}",
                dialect.qualified(crate::Invitation::MODULE, crate::Invitation::NAME),
                dialect.quote("organization"),
                dialect.placeholder(1),
                dialect.quote("accepted_at"),
                dialect.quote("expires_at"),
                dialect.placeholder(2),
                dialect.placeholder(3),
            );
            Some((seated, params))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::OrganizationRole::{Admin, Member, Owner};

    #[test]
    fn an_accepted_invitation_raises_a_role_but_never_lowers_one() {
        assert!(Owner.outranks(Admin) && Admin.outranks(Member));
        assert!(!Member.outranks(Admin) && !Admin.outranks(Admin));
    }
}
