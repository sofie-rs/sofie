//! In an organization area: its members and pending invitations, inviting, roles, removal and
//! leaving, and the owners' renaming, handing over and deleting the organization.

use std::collections::HashMap;

use sofie::core::Instant;
use sofie::db::{Queued, in_select};
use sofie::http::{ListedRoute, Rate, Routes, Shipped, SignedUrls};
use sofie::mail::{Address, Mail, Mailer, Template};
use sofie::prelude::*;
use sofie::serde_json::json;

use sofie_identity::{Account, AccountId, MemberArea, Unscoped};

use crate::acting::Acting;
use crate::invitation::{self, Invitation, VALID_FOR};
use crate::organization::{
    Demoted, Held, Invited, Kept, Organization, OrganizationArea, OrganizationId,
    OrganizationMember, OrganizationRole, membership,
};
use crate::organizations::{AccountEmail, invitation_path};

const INVITATION_MAIL: Template = Template::new(
    "sofie_tenancy/invitation",
    sofie::db::embed!(
        "src/mails/invitation.html",
        compiled = "mails/sofie_tenancy/invitation.sft"
    ),
);

const MEMBERS: &str = r#"<Section id="members" title="Members">
  <Table id="member-list" of={data.members} columns="email role" as="member">
    <Action post="members/{member.id}/admin" label="Make admin" target="members" />
    <Action post="members/{member.id}/member" label="Make member" target="members" />
    <Action post="members/{member.id}/remove" label="Remove" target="members" />
    <Action post="members/{member.id}/handover" label="Hand over ownership" target="members" />
  </Table>
  <Table id="invitations" of={data.invitations} columns="email role expires_at" as="invitation">
    <Action post="members/invitations/{invitation.id}/withdraw" label="Withdraw" target="members" />
  </Table>
  <Form id="invite" post="members/invite" target="members">
    <Submit label="Invite" />
  </Form>
  <Form id="rename" post="organization/rename" target="members">
    <Submit label="Rename the organization" />
  </Form>
  <form method="post" action="members/leave"><button type="submit">Leave the organization</button></form>
  <form method="post" action="organization/delete"><button type="submit">Delete the organization</button></form>
</Section>"#;

/// Per caller: each invitation mails an address the caller names.
const INVITING: Rate = Rate::per_minute(10);

#[rustfmt::skip]
pub const ROUTES: &[ListedRoute] = &[
    ListedRoute::new("organization_routes", "GET", "/members", "list_members"),
    ListedRoute::new("organization_routes", "POST", "/members/invite", "invite_member"),
    ListedRoute::new("organization_routes", "POST", "/members/{organization_member}/owner", "make_owner"),
    ListedRoute::new("organization_routes", "POST", "/members/{organization_member}/admin", "make_admin"),
    ListedRoute::new("organization_routes", "POST", "/members/{organization_member}/member", "make_member"),
    ListedRoute::new("organization_routes", "POST", "/members/invitations/{invitation}/withdraw", "withdraw_invitation"),
    ListedRoute::new("organization_routes", "POST", "/members/{organization_member}/remove", "remove_member"),
    ListedRoute::new("organization_routes", "POST", "/members/{organization_member}/handover", "hand_over_organization"),
    ListedRoute::new("organization_routes", "POST", "/members/leave", "leave_organization"),
    ListedRoute::new("organization_routes", "POST", "/organization/rename", "rename_organization"),
    ListedRoute::new("organization_routes", "POST", "/organization/delete", "delete_organization"),
];

/// `M` is the member area an invitation is accepted in.
pub fn routes<O: OrganizationArea, M: MemberArea>(routes: &mut Routes<O>) {
    let describe: sofie::http::Describe = |screens, key| {
        screens.describe_form::<Inviting>(key, "invite")?;
        screens.describe_form::<Renaming>(key, "rename")
    };
    routes
        .shipped("/members", "members", (MEMBERS, Some(describe)), list::<O>)
        .name("list_members");
    routes
        .post("/members/invite", invite::<O, M>)
        .name("invite_member")
        .throttle(INVITING)
        .then_screen("members", "members");
    for (role, path, handler) in [
        (OrganizationRole::Owner, "owner", "make_owner"),
        (OrganizationRole::Admin, "admin", "make_admin"),
        (OrganizationRole::Member, "member", "make_member"),
    ] {
        let assigning =
            move |acting: Acting, member: OrganizationMember, read: Read, clock: Clock| {
                assign(acting, member, read, clock, role)
            };
        routes
            .post(
                &format!("/members/{{organization_member}}/{path}"),
                assigning,
            )
            .name(handler)
            .then_screen("members", "members");
    }
    routes
        .post("/members/invitations/{invitation}/withdraw", withdraw)
        .name("withdraw_invitation")
        .then_screen("members", "members");
    routes
        .post("/members/{organization_member}/remove", remove::<O>)
        .name("remove_member")
        .then_screen("members", "members");
    routes
        .post("/members/{organization_member}/handover", hand_over)
        .name("hand_over_organization")
        .then_screen("members", "members");
    routes
        .post("/members/leave", leave::<O>)
        .name("leave_organization")
        .then_redirect(M::HOME);
    routes
        .post("/organization/rename", rename::<O>)
        .name("rename_organization")
        .then_screen("members", "members");
    routes
        .post("/organization/delete", delete::<O>)
        .name("delete_organization")
        .then_redirect(M::HOME);
}

#[derive(ReadModel)]
#[read(from = OrganizationMember)]
struct Membership {
    id: Id<OrganizationMember>,
    account: AccountId,
    role: OrganizationRole,
}

#[derive(ReadModel)]
#[read(from = Invitation)]
struct Pending {
    id: Id<Invitation>,
    email: Email,
    role: OrganizationRole,
    expires_at: Instant,
}

#[derive(ReadModel)]
struct Listed {
    id: Id<OrganizationMember>,
    email: Option<Email>,
    role: OrganizationRole,
}

#[derive(ReadModel)]
struct Members {
    members: Vec<Listed>,
    invitations: Vec<Pending>,
}

/// Addresses are the accounts', read beyond the organization's reach.
async fn list<O: OrganizationArea>(
    _acting: Acting,
    organization: Principal<O>,
    read: Read,
    unscoped: Unscoped,
    clock: Clock,
) -> Result<Members> {
    let organization = organization.id();
    let memberships = Membership::query()
        .where_(OrganizationMember::ORGANIZATION.eq(organization))
        .all(&read)
        .await?;
    let addresses: HashMap<AccountId, Email> = AccountEmail::query()
        .where_(in_select(
            Account::ID,
            OrganizationMember::ACCOUNT,
            OrganizationMember::ORGANIZATION.eq(organization),
        ))
        .all(&unscoped.0.read())
        .await?
        .into_iter()
        .map(|address| (address.id, address.email))
        .collect();
    let members = memberships
        .into_iter()
        .map(|member| Listed {
            id: member.id,
            email: addresses.get(&member.account).cloned(),
            role: member.role,
        })
        .collect();
    let invitations = Pending::query()
        .scope(|query| invitation::pending(query, organization, clock.now()))
        .all(&read)
        .await?;
    Ok(Members {
        members,
        invitations,
    })
}

#[derive(Input)]
pub struct Inviting {
    email: Email,
    role: OrganizationRole,
}

#[expect(
    clippy::too_many_arguments,
    reason = "each is a separate resolved argument"
)]
async fn invite<O: OrganizationArea, M: MemberArea>(
    acting: Acting,
    organization: Principal<O>,
    inviting: Inviting,
    read: Read,
    unscoped: Unscoped,
    mailer: Mailer,
    urls: SignedUrls,
    clock: Clock,
    #[cfg(feature = "billing")] billing: sofie::billing::Billing,
) -> Result<Held<(Invitation, Queued)>> {
    acting
        .role()
        .managing(inviting.role == OrganizationRole::Owner)?;
    let organization = organization.id();
    let now = clock.now();
    let pending = Invitation::query().scope(|query| invitation::pending(query, organization, now));
    if pending
        .clone()
        .where_(Invitation::EMAIL.eq(inviting.email.clone()))
        .exists(&read)
        .await?
    {
        return Err(Error::invalid("email", "is invited already"));
    }
    let addressed = AccountEmail::query()
        .where_(Account::EMAIL.eq(inviting.email.clone()))
        .first(&unscoped.0.read())
        .await?;
    if let Some(addressed) = addressed
        && membership(&read, organization, addressed.id)
            .await?
            .is_some()
    {
        return Err(Error::invalid("email", "is a member already"));
    }
    #[cfg(feature = "billing")]
    let kept = crate::organization::seats_kept(&read, &billing, organization, now, 1).await?;
    #[cfg(not(feature = "billing"))]
    let kept = Kept::AnOwner;
    let name = stored(&read, organization).await?;
    let inviter = AccountEmail::query()
        .where_(Account::ID.eq(acting.member().account()))
        .first(&unscoped.0.read())
        .await?
        .ok_or(Error::NotFound)?;
    let invitation = Invitation::new(
        organization,
        inviting.email.clone(),
        inviting.role,
        inviter.id,
        now,
    )?;
    let link = urls.sign(&invitation_path::<M>(invitation.id()), VALID_FOR)?;
    let body = mailer.body_from(
        &INVITATION_MAIL,
        &json!({
            "inviter": inviter.email.as_str(),
            "organization": name.name(),
            "link": mailer.link(&link)?,
        }),
    )?;
    let subject = format!("Join {}", name.name());
    let mail = Mail::new(Address::new(&inviting.email)?, &subject, body)?;
    let once = Invited::of(&invitation, now);
    Ok(Held::new(organization, kept, (invitation, mailer.send(mail)?)).inviting(once))
}

async fn withdraw(acting: Acting, mut invitation: Invitation, clock: Clock) -> Result<Invitation> {
    acting
        .role()
        .managing(invitation.role() == OrganizationRole::Owner)?;
    invitation.withdraw(clock.now())?;
    Ok(invitation)
}

async fn not_the_last_owner(read: &Read, member: &OrganizationMember) -> Result<()> {
    if member.role() != OrganizationRole::Owner {
        return Ok(());
    }
    let owners = OrganizationMember::query()
        .where_(OrganizationMember::ORGANIZATION.eq(member.organization()))
        .where_(OrganizationMember::ROLE.eq(OrganizationRole::Owner))
        .count(read)
        .await?;
    if owners <= 1 {
        return Err(Error::Rule(
            "An organization keeps at least one owner; make another member owner first.".to_owned(),
        ));
    }
    Ok(())
}

/// The member's open invitations its new role could not have sent are withdrawn, as on removal.
async fn assign(
    acting: Acting,
    mut member: OrganizationMember,
    read: Read,
    clock: Clock,
    role: OrganizationRole,
) -> Result<Held<(OrganizationMember, Vec<Invitation>)>> {
    acting
        .role()
        .managing(role == OrganizationRole::Owner || member.role() == OrganizationRole::Owner)?;
    if role != OrganizationRole::Owner {
        not_the_last_owner(&read, &member).await?;
    }
    member.assign(role)?;
    let now = clock.now();
    let withdrawn = beyond_role(&read, &member, now).await?;
    let demoted = Demoted {
        account: member.account(),
        role,
        now,
    };
    Ok(Held::new(member.organization(), Kept::AnOwner, (member, withdrawn)).demoting(demoted))
}

/// The member's open invitations its role, given just now, could not have sent, withdrawn.
async fn beyond_role(
    read: &Read,
    member: &OrganizationMember,
    now: Instant,
) -> Result<Vec<Invitation>> {
    let mut withdrawn = sent_by(read, member, now).await?;
    withdrawn.retain(|invitation| {
        member
            .role()
            .managing(invitation.role() == OrganizationRole::Owner)
            .is_err()
    });
    for invitation in &mut withdrawn {
        invitation.withdraw(now)?;
    }
    Ok(withdrawn)
}

async fn sent_by(
    read: &Read,
    member: &OrganizationMember,
    now: Instant,
) -> Result<Vec<Invitation>> {
    Invitation::query()
        .scope(|query| invitation::pending(query, member.organization(), now))
        .where_(Invitation::INVITED_BY.eq(member.account()))
        .all(read)
        .await
}

async fn remove<O: OrganizationArea>(
    acting: Acting,
    member: OrganizationMember,
    read: Read,
    unscoped: Unscoped,
    clock: Clock,
) -> Result<()> {
    if member.id() != acting.member().id() {
        acting
            .role()
            .managing(member.role() == OrganizationRole::Owner)?;
    }
    removing::<O>(member, &read, &unscoped, clock.now()).await
}

async fn leave<O: OrganizationArea>(
    acting: Acting,
    read: Read,
    unscoped: Unscoped,
    clock: Clock,
) -> Result<()> {
    removing::<O>(acting.into_member(), &read, &unscoped, clock.now()).await
}

/// Its sessions acting for the organization end with it; those acting for its other
/// organizations stay. Its open invitations there are withdrawn, so none it no longer stands
/// behind joins anybody or holds a seat.
async fn removing<O: OrganizationArea>(
    mut member: OrganizationMember,
    read: &Read,
    unscoped: &Unscoped,
    now: Instant,
) -> Result<()> {
    not_the_last_owner(read, &member).await?;
    member.leave()?;
    let organization = member.organization();
    let ended = ending_for::<O>(unscoped, organization, &[member.account()], now).await?;
    let mut withdrawn = sent_by(read, &member, now).await?;
    for invitation in &mut withdrawn {
        invitation.withdraw(now)?;
    }
    unscoped
        .0
        .commit(&mut Held::new(
            organization,
            Kept::AnOwner,
            (member, ended, withdrawn),
        ))
        .await
}

async fn ending_for<O: OrganizationArea>(
    unscoped: &Unscoped,
    organization: OrganizationId,
    accounts: &[AccountId],
    now: Instant,
) -> Result<Vec<sofie_identity::sessions::AccountSession>> {
    sofie_identity::sessions::ending_as(
        &unscoped.0.read(),
        accounts,
        O::NAME,
        organization.uuid(),
        now,
    )
    .await
}

/// The owner handing over stays as an admin, and its open invitations of owners are withdrawn.
async fn hand_over(
    acting: Acting,
    mut member: OrganizationMember,
    read: Read,
    clock: Clock,
) -> Result<Held<(OrganizationMember, OrganizationMember, Vec<Invitation>)>> {
    acting.owning()?;
    if member.id() == acting.member().id() {
        return Err(Error::Rule(HAND_OVER_TO_ANOTHER.to_owned()));
    }
    member.assign(OrganizationRole::Owner)?;
    let mut owner = acting.into_member();
    owner.assign(OrganizationRole::Admin)?;
    let now = clock.now();
    let withdrawn = beyond_role(&read, &owner, now).await?;
    let demoted = Demoted {
        account: owner.account(),
        role: OrganizationRole::Admin,
        now,
    };
    Ok(Held::new(
        member.organization(),
        Kept::AnOwner,
        (member, owner, withdrawn),
    )
    .demoting(demoted))
}

const HAND_OVER_TO_ANOTHER: &str = "Hand the organization over to another of its members.";

#[derive(Input)]
pub struct Renaming {
    #[max(100)]
    name: String,
}

async fn rename<O: OrganizationArea>(
    acting: Acting,
    organization: Principal<O>,
    renaming: Renaming,
    read: Read,
) -> Result<Organization> {
    acting.owning()?;
    let mut organization = stored(&read, organization.id()).await?;
    organization.rename(&renaming.name)?;
    Ok(organization)
}

/// Every member leaves, its sessions acting for it end, and every open invitation is
/// withdrawn. What it owns stays for its modules to erase on `OrganizationDeleted`.
async fn delete<O: OrganizationArea>(
    acting: Acting,
    organization: Principal<O>,
    read: Read,
    unscoped: Unscoped,
    clock: Clock,
    #[cfg(feature = "billing")] billing: sofie::billing::Billing,
) -> Result<()> {
    acting.owning()?;
    let id = organization.id();
    #[cfg(feature = "billing")]
    if billing
        .entitlements(sofie::billing::Subscriber::from(id))
        .await?
        .standing()
        .is_some_and(sofie::billing::Standing::renews_at_provider)
    {
        return Err(Error::Rule(PLAN_RUNNING.to_owned()));
    }
    let mut organization = stored(&read, id).await?;
    organization.dissolve()?;
    let now = clock.now();
    let mut members = OrganizationMember::query()
        .where_(OrganizationMember::ORGANIZATION.eq(id))
        .all(&read)
        .await?;
    for member in &mut members {
        member.leave()?;
    }
    let mut withdrawn = Invitation::query()
        .scope(|query| invitation::pending(query, id, now))
        .all(&read)
        .await?;
    for invitation in &mut withdrawn {
        invitation.withdraw(now)?;
    }
    let accounts: Vec<AccountId> = members.iter().map(OrganizationMember::account).collect();
    let ended = ending_for::<O>(&unscoped, id, &accounts, now).await?;
    unscoped
        .0
        .commit(&mut Held::new(
            id,
            Kept::Nothing,
            (organization, members, withdrawn, ended),
        ))
        .await
}

#[cfg(feature = "billing")]
const PLAN_RUNNING: &str = "Cancel the organization's plan before deleting it.";

async fn stored(read: &Read, id: OrganizationId) -> Result<Organization> {
    Organization::query()
        .where_(Organization::ID.eq(id))
        .first(read)
        .await?
        .ok_or(Error::NotFound)
}
