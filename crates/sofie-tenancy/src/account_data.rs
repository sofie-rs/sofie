use std::collections::HashMap;

use sofie::core::{Cast, SqlValue};
use sofie::db::{Aggregate, BoxFuture, Db, Outcome, Plan, Step, Uuid};
use sofie::prelude::*;
use sofie::serde_json::{Value, json};
use sofie_identity::AccountId;
use sofie_identity::account_data::{Closable, PersonalData};

use crate::organization::{
    Held, Kept, Organization, OrganizationId, OrganizationMember, OrganizationRole,
};

/// For `sofie_identity::account_data::register`. The organizations own the memberships, so
/// they are read beyond the closing account's reach.
pub struct OrganizationMemberships(Db);

impl OrganizationMemberships {
    #[must_use]
    pub fn new(db: &Db) -> Self {
        Self(db.clone())
    }
}

impl PersonalData for OrganizationMemberships {
    fn name(&self) -> &'static str {
        "sofie_tenancy"
    }

    fn export<'a>(&'a self, _read: &'a Read, account: Uuid) -> BoxFuture<'a, Result<Value>> {
        Box::pin(async move {
            let held = of_account(&self.0.read(), account).await?;
            Ok(json!(
                held.iter()
                    .map(|member| json!({
                        "organization": member.organization().uuid(),
                        "role": member.role().name(),
                    }))
                    .collect::<Vec<_>>()
            ))
        })
    }

    fn closing<'a>(
        &'a self,
        _read: &'a Read,
        account: Uuid,
    ) -> BoxFuture<'a, Result<Vec<Box<dyn Closable>>>> {
        Box::pin(async move {
            let read = self.0.read();
            let held = of_account(&read, account).await?;
            let alone = owned_alone(&read, &held).await?;
            if !alone.is_empty() {
                return Err(last_owner_of(&read, alone).await?);
            }
            // In one order, so two accounts closing at once never take each other's rows.
            let mut held = held;
            held.sort_by_key(|member| member.organization().uuid());
            let mut closing = held
                .into_iter()
                .map(|mut member| {
                    member.leave()?;
                    let organization = member.organization();
                    let left: Box<dyn Closable> =
                        Box::new(Held::new(organization, Kept::AnOwner, Left(member)));
                    Ok(left)
                })
                .collect::<Result<Vec<_>>>()?;
            closing.push(Box::new(Withdrawn(AccountId::from_uuid(account))));
            Ok(closing)
        })
    }
}

async fn of_account(read: &Read, account: Uuid) -> Result<Vec<OrganizationMember>> {
    OrganizationMember::query()
        .where_(OrganizationMember::ACCOUNT.eq(AccountId::from_uuid(account)))
        .all(read)
        .await
}

async fn owned_alone(read: &Read, held: &[OrganizationMember]) -> Result<Vec<OrganizationId>> {
    let owned: Vec<OrganizationId> = held
        .iter()
        .filter(|member| member.role() == OrganizationRole::Owner)
        .map(OrganizationMember::organization)
        .collect();
    if owned.is_empty() {
        return Ok(owned);
    }
    let owners = OrganizationMember::query()
        .where_(OrganizationMember::ORGANIZATION.in_(owned.iter().copied()))
        .where_(OrganizationMember::ROLE.eq(OrganizationRole::Owner))
        .all(read)
        .await?;
    let mut counted: HashMap<OrganizationId, usize> = HashMap::new();
    for owner in &owners {
        *counted.entry(owner.organization()).or_default() += 1;
    }
    Ok(owned
        .into_iter()
        .filter(|organization| counted.get(organization).copied().unwrap_or(0) <= 1)
        .collect())
}

async fn last_owner_of(read: &Read, organizations: Vec<OrganizationId>) -> Result<Error> {
    let organizations = Organization::query()
        .where_(Organization::ID.in_(organizations))
        .all(read)
        .await?;
    let mut names: Vec<&str> = organizations.iter().map(Organization::name).collect();
    names.sort_unstable();
    Ok(Error::Rule(format!(
        "This account is the last owner of {}; hand each over to another member or delete it \
         first.",
        names.join(", ")
    )))
}

/// Written as a statement: the closing commits within the account's reach, which the
/// organization's membership is beyond. Holds the member only for the event its leaving raised.
struct Left(OrganizationMember);

impl Outcome for Left {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        let dialect = plan.dialect();
        let [deleted_at, updated_at, version] =
            ["deleted_at", "updated_at", "version"].map(|column| dialect.quote(column));
        let sql = format!(
            "UPDATE {} SET {deleted_at} = {now}, {updated_at} = {now}, {version} = {version} + 1 \
             WHERE {} = {} AND {deleted_at} IS NULL",
            dialect.qualified(OrganizationMember::MODULE, OrganizationMember::NAME),
            dialect.quote("id"),
            dialect.placeholder(1),
            now = dialect.placeholder(2),
        );
        let mut params = vec![SqlValue::Uuid(self.0.id().uuid())];
        plan.stamp().now.encode(&mut params);
        plan.push_as::<OrganizationMember>(Step::statement(sql, params, Some(1)));
        plan.raise(self.0.meta().events());
        Ok(())
    }
}

/// The open invitations the account sent, withdrawn, so none it no longer stands behind joins
/// anybody or holds a seat.
struct Withdrawn(AccountId);

impl Outcome for Withdrawn {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        let dialect = plan.dialect();
        let [expires_at, updated_at, version] =
            ["expires_at", "updated_at", "version"].map(|column| dialect.quote(column));
        let sql = format!(
            "UPDATE {} SET {expires_at} = {now}, {updated_at} = {now}, {version} = {version} + 1 \
             WHERE {} = {} AND {} IS NULL AND {expires_at} > {now}",
            dialect.qualified(crate::Invitation::MODULE, crate::Invitation::NAME),
            dialect.quote("invited_by"),
            dialect.placeholder(1),
            dialect.quote("accepted_at"),
            now = dialect.placeholder(2),
        );
        let mut params = vec![SqlValue::Uuid(self.0.uuid())];
        plan.stamp().now.encode(&mut params);
        plan.push_as::<crate::Invitation>(Step::statement(sql, params, None));
        Ok(())
    }
}
