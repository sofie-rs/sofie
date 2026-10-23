use sofie::http::{Routes, Web};
use sofie::prelude::*;

use super::{Mailed, link_routes};
use crate::MemberArea;
use crate::account::{Account, Distrusted};
use crate::account_link::{AccountLink, LinkPurpose};
use crate::membership;
use crate::signing_in::{Door, Entered};

struct SignInLink;

impl Mailed for SignInLink {
    const PURPOSE: LinkPurpose = LinkPurpose::SignIn;
    const SCREENS: [&'static str; 2] = ["ask_sign_in_link", "sign_in_link"];
    const NAMES: [&'static str; 3] = [
        "ask_sign_in_link_page",
        "ask_sign_in_link",
        "sign_in_link_page",
    ];
}

pub(crate) fn asking<A: MemberArea>() -> String {
    format!("{}{}", A::PREFIX, SignInLink::ASKING)
}

pub(super) fn routes<A: MemberArea>(routes: &mut Routes<Web>) {
    let follow = link_routes::<A, SignInLink>(routes);
    routes
        .post(&follow, sign_in_by_link::<A>)
        .name_for::<A>("sign_in_by_link")
        .signed()
        .then_redirect_or_follow(A::HOME);
}

/// Following the link proves the address too, so it also confirms it.
/// Confirming it first removes the second factor, so none is asked.
async fn sign_in_by_link<A: MemberArea>(
    mut link: AccountLink,
    door: Door,
) -> Result<Entered<(Account, AccountLink, Option<Distrusted>)>> {
    let now = door.clock().now();
    link.use_up(SignInLink::PURPOSE, now)?;
    let mut account = Account::stored(door.read(), link.account()).await?;
    membership::require(door.read(), account.id(), A::NAME).await?;
    let distrusted = account.prove_email(door.read(), now).await?;
    Ok(door
        .enter_confirming::<A>(account.id(), distrusted.is_some())
        .await?
        .saving((account, link, distrusted)))
}
