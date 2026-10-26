//! In a member area: an account's organizations, founding one, entering one, and
//! accepting an invitation. Organizations' records are beyond the account's reach, so
//! what these change is committed by them, unscoped, after their own checks.

use std::collections::HashMap;
use std::time::Duration;

use sofie::db::{Uuid, in_select};
use sofie::http::{Admitter, ListedRoute, Query, Routes, Shipped, SignInRight, SignedUrls};
use sofie::prelude::*;

use sofie_identity::{Account, AccountId, MemberArea, SignedIn, Unscoped, sign_in_for};

use crate::invitation::{Invitation, NOT_OPEN};
use crate::organization::{
    Organization, OrganizationArea, OrganizationId, OrganizationMember, OrganizationRole,
    membership,
};

/// Entering an organization signs its member in to the organization's area.
struct Tenancy;

impl Admitter for Tenancy {}

const RIGHT: SignInRight = SignInRight::of::<Tenancy>();

const NOT_A_MEMBER: &str = "You are not a member of this organization.";
const ANOTHER_ADDRESS: &str = "This invitation is for another address; sign in with that one.";

const ORGANIZATIONS: &str = r#"<Section id="organizations" title="Organizations">
  {#each data.organizations as organization}
    <form method="post" action="organizations/enter"><input type="hidden" name="organization" value={organization.id}><button type="submit">{organization.name} ({organization.role})</button></form>
  {/each}
  <Form id="found-organization" post="organizations" target="organizations">
    <Submit label="Found an organization" />
  </Form>
</Section>"#;

const INVITATION: &str = r#"<Section title="Invitation">
  {#if data.invitation}
    <p>{data.invitation.organization} invites {data.invitation.email} to join as {data.invitation.role}.</p>
    <form method="post" action={data.invitation.action}><button type="submit">Join</button></form>
  {:else}
    <p>This invitation was accepted already or has expired.</p>
  {/if}
</Section>"#;

#[rustfmt::skip]
pub const ROUTES: &[ListedRoute] = &[
    ListedRoute::new("member_routes", "GET", "/organizations", "list_organizations"),
    ListedRoute::new("member_routes", "POST", "/organizations", "found_organization"),
    ListedRoute::new("member_routes", "POST", "/organizations/enter", "enter_organization"),
    ListedRoute::new("member_routes", "GET", "/invitations", "invitation_page"),
    ListedRoute::new("member_routes", "POST", "/invitations", "accept_invitation"),
];

pub fn routes<M: MemberArea, O: OrganizationArea>(routes: &mut Routes<M>) {
    let describe: sofie::http::Describe =
        |screens, key| screens.describe_form::<Founding>(key, "found-organization");
    routes
        .shipped(
            "/organizations",
            "organizations",
            (ORGANIZATIONS, Some(describe)),
            list::<M>,
        )
        .name("list_organizations");
    routes
        .post("/organizations", found::<M>)
        .name("found_organization")
        .then_screen("organizations", "organizations");
    routes
        .post("/organizations/enter", enter::<M, O>)
        .name("enter_organization")
        .then_redirect(O::HOME);
    routes
        .shipped(
            "/invitations",
            "invitation",
            (INVITATION, None),
            invitation_page::<M>,
        )
        .name("invitation_page")
        .signed();
    routes
        .post("/invitations", accept::<M>)
        .name("accept_invitation")
        .signed()
        .then_redirect(&format!("{}/organizations", M::PREFIX));
}

pub(crate) fn invitation_path<M: MemberArea>(invitation: Id<Invitation>) -> String {
    format!("{}/invitations?invitation={}", M::PREFIX, invitation.uuid())
}

#[derive(ReadModel)]
#[read(from = Account)]
pub(crate) struct AccountEmail {
    pub(crate) id: AccountId,
    pub(crate) email: Email,
}

#[derive(ReadModel)]
#[read(from = OrganizationMember)]
struct Held {
    organization: OrganizationId,
    role: OrganizationRole,
}

#[derive(ReadModel)]
#[read(from = Organization)]
struct Named {
    id: OrganizationId,
    name: String,
}

#[derive(ReadModel)]
struct Listed {
    id: OrganizationId,
    name: String,
    role: OrganizationRole,
}

#[derive(ReadModel)]
struct Organizations {
    organizations: Vec<Listed>,
}

async fn list<M: MemberArea>(principal: Principal<M>, unscoped: Unscoped) -> Result<Organizations> {
    let read = unscoped.0.read();
    let account = principal.id();
    let held = Held::query()
        .where_(OrganizationMember::ACCOUNT.eq(account))
        .all(&read)
        .await?;
    let mut names: HashMap<OrganizationId, String> = Named::query()
        .where_(in_select(
            Organization::ID,
            OrganizationMember::ORGANIZATION,
            OrganizationMember::ACCOUNT.eq(account),
        ))
        .all(&read)
        .await?
        .into_iter()
        .map(|named| (named.id, named.name))
        .collect();
    let organizations = held
        .into_iter()
        .filter_map(|held| {
            Some(Listed {
                id: held.organization,
                name: names.remove(&held.organization)?,
                role: held.role,
            })
        })
        .collect();
    Ok(Organizations { organizations })
}

#[derive(Input)]
pub struct Founding {
    #[max(100)]
    name: String,
}

async fn found<M: MemberArea>(
    principal: Principal<M>,
    founding: Founding,
    unscoped: Unscoped,
) -> Result<()> {
    let organization = Organization::named(&founding.name)?;
    let owner =
        OrganizationMember::new(organization.id(), principal.id(), OrganizationRole::Owner)?;
    unscoped.0.commit(&mut (organization, owner)).await
}

#[derive(Input)]
pub struct Entering {
    organization: OrganizationId,
}

async fn enter<M: MemberArea, O: OrganizationArea>(
    principal: Principal<M>,
    entering: Entering,
    unscoped: Unscoped,
    session: Session,
    clock: Clock,
) -> Result<SignedIn> {
    let account = principal.id();
    if membership(&unscoped.0.read(), entering.organization, account)
        .await?
        .is_none()
    {
        return Err(Error::Rule(NOT_A_MEMBER.to_owned()));
    }
    sign_in_for::<O>(&session, RIGHT, entering.organization, account, &clock)
}

fn invitation_id(query: &Query) -> Result<Id<Invitation>> {
    query
        .get("invitation")
        .and_then(|id| id.parse::<Uuid>().ok())
        .map(Id::from_uuid)
        .ok_or(Error::NotFound)
}

async fn invited(read: &Read, id: Id<Invitation>) -> Result<Invitation> {
    Invitation::query()
        .where_(Invitation::ID.eq(id))
        .first(read)
        .await?
        .ok_or(Error::NotFound)
}

#[derive(ReadModel)]
struct Open {
    organization: String,
    email: Email,
    role: OrganizationRole,
    action: String,
}

#[derive(ReadModel)]
struct InvitationPage {
    invitation: Option<Open>,
}

async fn invitation_page<M: MemberArea>(
    _principal: Principal<M>,
    query: Query,
    unscoped: Unscoped,
    urls: SignedUrls,
    clock: Clock,
) -> Result<InvitationPage> {
    let read = unscoped.0.read();
    let id = invitation_id(&query)?;
    let invitation = invited(&read, id).await?;
    let now = clock.now();
    if !invitation.is_open(now) {
        return Ok(InvitationPage { invitation: None });
    }
    let organization = Named::query()
        .where_(Organization::ID.eq(invitation.organization()))
        .first(&read)
        .await?
        .ok_or(Error::NotFound)?;
    let left = invitation
        .expires_at()
        .timestamp()
        .duration_since(now.timestamp());
    let left = Duration::try_from(left).map_err(|_| Error::Rule(NOT_OPEN.to_owned()))?;
    Ok(InvitationPage {
        invitation: Some(Open {
            organization: organization.name,
            email: invitation.email().clone(),
            role: invitation.role(),
            action: urls.sign(&invitation_path::<M>(id), left)?,
        }),
    })
}

/// Only for the account the invitation's address belongs to, so a forwarded mail adds
/// nobody else; the mailed link it was opened by confirms that address.
/// An account already a member keeps the higher of its role and the invitation's.
async fn accept<M: MemberArea>(
    principal: Principal<M>,
    query: Query,
    unscoped: Unscoped,
    clock: Clock,
    #[cfg(feature = "billing")] billing: sofie::billing::Billing,
) -> Result<()> {
    let read = unscoped.0.read();
    let mut account = Account::query()
        .where_(Account::ID.eq(principal.id()))
        .first(&read)
        .await?
        .ok_or(Error::NotFound)?;
    let mut invitation = invited(&read, invitation_id(&query)?).await?;
    if account.email() != invitation.email() {
        return Err(Error::Rule(ANOTHER_ADDRESS.to_owned()));
    }
    let now = clock.now();
    let confirmed = account.verified_at().is_none().then(|| {
        account.confirm_email(now);
        account
    });
    let organization = invitation.organization();
    #[cfg(feature = "billing")]
    let kept = crate::organization::seats_kept(&read, &billing, organization, now, 0).await?;
    #[cfg(not(feature = "billing"))]
    let kept = crate::organization::Kept::AnOwner;
    invitation.accept(now)?;
    let joined = match membership(&read, organization, principal.id()).await? {
        Some(mut member) if invitation.role().outranks(member.role()) => {
            member.assign(invitation.role())?;
            vec![member]
        }
        Some(_) => Vec::new(),
        None => vec![OrganizationMember::new(
            organization,
            principal.id(),
            invitation.role(),
        )?],
    };
    unscoped
        .0
        .commit(&mut crate::organization::Held::new(
            organization,
            kept,
            (invitation, joined, confirmed),
        ))
        .await
}
