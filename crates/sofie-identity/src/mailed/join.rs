use sofie::http::{Rate, Routes, SignedUrls, Web};
use sofie::prelude::*;

use crate::MemberArea;
use crate::account::Account;
use crate::account_link::{AccountLink, LinkPage, LinkPurpose};
use crate::actions::{NOT_RIGHT, SignInCredentials};
use crate::membership::{self, Membership};
use crate::pages::FormPage;
use crate::signing_in::{Door, Entered};

const PURPOSE: LinkPurpose = LinkPurpose::Join;

/// Mounted only for an area whose sign-up is open, as only its sign-up mails the link.
pub(super) fn routes<A: MemberArea>(routes: &mut Routes<Web>) {
    let link = format!("{}{}/{{account_link}}", A::PREFIX, PURPOSE.segment());
    routes
        .screen(&link, "join", join_page::<A>)
        .name_for::<A>("join_page")
        .signed();
    routes
        .post(&link, join::<A>)
        .name_for::<A>("join")
        .signed()
        .throttle(Rate::per_minute(10))
        .then_redirect_or_follow(A::HOME);
}

async fn join_page<A: MemberArea>(
    link: LinkPage,
    urls: SignedUrls,
    clock: Clock,
) -> Result<FormPage> {
    link.form::<A>(PURPOSE, &urls, &clock)
}

/// Signing in as the account the link was mailed to adds its membership. Following the link
/// proves the address, so it confirms it for an area asking a confirmed one; the password
/// proves the account, so, unlike a sign-in link, nothing of it is distrusted.
async fn join<A: MemberArea>(
    mut link: AccountLink,
    credentials: SignInCredentials,
    door: Door,
    #[cfg(feature = "shield")] sign_ins: sofie::http::SignIns,
) -> Result<Entered<(Option<Account>, AccountLink, Option<Membership>)>> {
    let now = door.clock().now();
    link.use_up(PURPOSE, now)?;
    let read = door.read();
    let mut attempt = credentials
        .attempt(
            read,
            #[cfg(feature = "shield")]
            &sign_ins,
        )
        .await?;
    let Some(mut account) = attempt
        .account
        .take_if(|account| attempt.matches && account.id() == link.account())
    else {
        return Err(Error::Rule(NOT_RIGHT.to_owned()));
    };
    let membership = match membership::held(read, account.id(), A::NAME).await? {
        Some(_) => None,
        None => Some(membership::factory::grant(account.id(), A::NAME)?),
    };
    let changed = !account.is_confirmed() | attempt.rehash(&mut account).await?;
    account.confirm_email(now);
    Ok(door
        .enter_confirming::<A>(account.id(), false)
        .await?
        .saving((changed.then_some(account), link, membership)))
}
