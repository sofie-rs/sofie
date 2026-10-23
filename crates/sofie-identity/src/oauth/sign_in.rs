use sofie::db::{Outcome, Plan, Reply, Stamp};
use sofie::http::{Query, Rate, Routes, Sealing, Web};
use sofie::prelude::*;

use super::flow::{self, Identified, State, callback_path};
use super::{AccountIdentity, IDENTITY_INDEX, Installed, OAuth};
use crate::account::{ALREADY_REGISTERED, Account, EMAIL_INDEX, factory};
use crate::membership::{self, Membership};
use crate::pages::Offered;
use crate::signing_in::{Door, Entered};
use crate::{AccountId, MemberArea};

/// Per caller: each try costs a round trip to the provider.
const TRYING: Rate = Rate::per_minute(10);
const CONNECTED_ELSEWHERE: &str = "is connected to another account here";
const ACCOUNT_GONE: &str = "The account this was connecting to is closed.";

fn begin_path<A: MemberArea>() -> String {
    format!("{}/oauth", A::PREFIX)
}

pub(crate) fn routes<A: MemberArea>(routes: &mut Routes<Web>) {
    routes
        .post(&begin_path::<A>(), begin::<A>)
        .name_for::<A>("begin_oauth")
        .throttle(TRYING)
        .then_follow();
    routes
        .screen(&callback_path::<A>(), "oauth_return", returned_page::<A>)
        .name_for::<A>("oauth_return_page");
    routes
        .post(&callback_path::<A>(), finish::<A>)
        .name_for::<A>("finish_oauth")
        .throttle(TRYING)
        .then_redirect_or_follow(A::HOME);
}

pub(crate) fn offered<A: MemberArea>(installed: &Installed) -> Option<Offered> {
    Some(Offered {
        action: begin_path::<A>(),
        providers: installed.0.as_deref()?.choices(),
    })
}

#[derive(Input)]
pub struct Chosen {
    #[max(64)]
    pub(super) provider: String,
}

#[derive(ReadModel)]
pub struct Location {
    pub(super) location: String,
}

pub(crate) fn chosen<'o>(oauth: &'o OAuth, name: &str) -> Result<&'o super::Provider> {
    oauth
        .provider(name)
        .ok_or_else(|| Error::invalid("provider", "is not one this app signs in with"))
}

async fn begin<A: MemberArea>(
    chosen: Chosen,
    installed: Installed,
    session: Session,
    sealing: Sealing,
    clock: Clock,
) -> Result<Location> {
    let oauth = installed.get()?;
    let location = flow::begin::<A>(
        oauth,
        &chosen.provider,
        &session,
        &sealing,
        clock.now(),
        None,
    )?;
    Ok(Location { location })
}

/// Posts the provider's answer on at once, through `engine.js`, or at a click without it.
#[derive(ReadModel)]
pub struct ReturnPage {
    action: String,
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    sign_in: String,
}

async fn returned_page<A: MemberArea>(query: Query) -> Result<ReturnPage> {
    let error = query.get("error").map(|error| {
        error
            .chars()
            .filter(|character| !sofie::core::misleads_display(*character))
            .take(200)
            .collect()
    });
    Ok(ReturnPage {
        action: callback_path::<A>(),
        code: query.get("code"),
        state: query.get("state"),
        error,
        sign_in: A::SIGN_IN.to_owned(),
    })
}

#[derive(Input)]
pub struct Returned {
    #[max(4096)]
    code: String,
    #[max(4096)]
    state: String,
}

pub(crate) struct OAuthEntry {
    joined: Option<(Account, Membership)>,
    identity: Option<AccountIdentity>,
    entered: Option<Entered>,
    location: String,
}

async fn finish<A: MemberArea>(
    returned: Returned,
    door: Door,
    installed: Installed,
    sealing: Sealing,
) -> Result<OAuthEntry> {
    let oauth = installed.get()?;
    let now = door.clock().now();
    let state = flow::opened(&sealing, &returned.state, door.session(), A::NAME, now)?;
    let provider = chosen(oauth, &state.provider)?;
    let who = flow::identified::<A>(oauth, provider, &state, &returned.code, now).await?;
    let known = AccountIdentity::query()
        .where_(AccountIdentity::PROVIDER.eq(provider.name.clone()))
        .where_(AccountIdentity::SUBJECT.eq(who.subject.clone()))
        .first(door.read())
        .await?;
    match (known, state.link) {
        (Some(identity), Some(link)) if identity.account.uuid() != link => Err(Error::Rule(
            format!("This {} account {CONNECTED_ELSEWHERE}.", provider.label),
        )),
        (Some(_), Some(_)) => Ok(connected::<A>(None)),
        (None, Some(link)) => {
            let account = still_open::<A>(door.read(), AccountId::from_uuid(link)).await?;
            let identity = identity(account, &state, who)?;
            Ok(connected::<A>(Some(identity)))
        }
        (Some(identity), None) => signed_in_as::<A>(&door, identity.account).await,
        (None, None) => signed_up::<A>(&door, provider, &state, who).await,
    }
}

/// The account may have closed while its member was at the provider.
async fn still_open<A: MemberArea>(read: &Read, account: AccountId) -> Result<AccountId> {
    let gone = || Error::Rule(ACCOUNT_GONE.to_owned());
    let stored = Account::stored(read, account)
        .await
        .map_err(|error| match error {
            Error::NotFound => gone(),
            other => other,
        })?;
    if stored.is_closed() {
        return Err(gone());
    }
    membership::require(read, account, A::NAME).await?;
    Ok(account)
}

fn identity(account: AccountId, state: &State, who: Identified) -> Result<AccountIdentity> {
    Ok(AccountIdentity {
        meta: Meta::new()?,
        account,
        provider: state.provider.clone(),
        subject: who.subject,
        email: who.email,
    })
}

fn connected<A: MemberArea>(identity: Option<AccountIdentity>) -> OAuthEntry {
    OAuthEntry {
        joined: None,
        identity,
        entered: None,
        location: format!("{}{}", A::PREFIX, super::connections::PATH),
    }
}

async fn signed_in_as<A: MemberArea>(door: &Door, account: AccountId) -> Result<OAuthEntry> {
    membership::require(door.read(), account, A::NAME).await?;
    let entered = door.enter::<A>(account).await?;
    Ok(OAuthEntry {
        joined: None,
        identity: None,
        entered: Some(entered),
        location: String::new(),
    })
}

/// A new account only for an address the provider confirmed and no account here holds,
/// so nobody takes over an account by naming its address at a provider.
async fn signed_up<A: MemberArea>(
    door: &Door,
    provider: &super::Provider,
    state: &State,
    who: Identified,
) -> Result<OAuthEntry> {
    let email =
        who.email.clone().filter(|_| who.verified).ok_or_else(|| {
            Error::Rule(format!("{} confirmed no address of yours.", provider.label))
        })?;
    let taken = Account::query()
        .where_(Account::EMAIL.eq(email.clone()))
        .exists(door.read())
        .await?;
    if taken {
        return Err(Error::Rule(format!(
            "An account with this address exists here: sign in to it and connect {} from your account.",
            provider.label
        )));
    }
    if A::SIGN_UP.is_none() {
        return Err(Error::Rule(membership::NOT_A_MEMBER.to_owned()));
    }
    let mut account = factory::without_password(email)?;
    account.confirm_email(door.clock().now());
    let membership = membership::factory::grant(account.id(), A::NAME)?;
    let identity = identity(account.id(), state, who)?;
    let entered = door.enter_confirming::<A>(account.id(), false).await?;
    Ok(OAuthEntry {
        joined: Some((account, membership)),
        identity: Some(identity),
        entered: Some(entered),
        location: String::new(),
    })
}

impl Outcome for OAuthEntry {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        if let Some(joined) = &self.joined {
            plan.unique(EMAIL_INDEX, "email", ALREADY_REGISTERED);
            joined.plan(plan)?;
        }
        if let Some(identity) = &self.identity {
            plan.unique(IDENTITY_INDEX, "provider", CONNECTED_ELSEWHERE);
            identity.plan(plan)?;
        }
        if let Some(entered) = &self.entered {
            entered.plan(plan)?;
        }
        Ok(())
    }

    fn committed(&mut self, stamp: Stamp) {
        if let Some(joined) = &mut self.joined {
            joined.committed(stamp);
        }
        if let Some(identity) = &mut self.identity {
            identity.committed(stamp);
        }
        if let Some(entered) = &mut self.entered {
            entered.committed(stamp);
        }
    }

    fn reply(self) -> Result<Reply> {
        match self.entered {
            Some(entered) => entered.reply(),
            None => Location {
                location: self.location,
            }
            .reply(),
        }
    }
}
