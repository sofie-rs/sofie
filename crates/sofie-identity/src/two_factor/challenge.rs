use sofie::http::{Next, Rate, Routes, Sealing, SignedUrls, Web};
use sofie::prelude::*;

use super::Proof;
use crate::account_link::{AccountLink, INVALID, LinkPage, LinkPurpose, factory, signed_action};
use crate::membership;
use crate::pages::FormPage;
use crate::signing_in::{SignedIn, sign_in};
use crate::{AccountId, MemberArea};

const PURPOSE: LinkPurpose = LinkPurpose::SecondFactor;
/// Per caller, on top of the per-account limit on wrong codes.
const PROVING: Rate = Rate::per_minute(10);

#[derive(Input)]
pub struct FactorCode {
    code: String,
}

pub(crate) fn routes<A: MemberArea>(routes: &mut Routes<Web>) {
    let challenge = format!("{}{}/{{account_link}}", A::PREFIX, PURPOSE.segment());
    routes
        .screen(&challenge, "second_factor", page::<A>)
        .name_for::<A>("second_factor_page")
        .signed();
    routes
        .post(&challenge, prove::<A>)
        .name_for::<A>("prove_second_factor")
        .signed()
        .throttle(PROVING)
        .then_redirect(A::HOME);
}

/// `next` is signed in, so nobody can swap it.
pub(crate) fn asking<A: MemberArea>(
    account: AccountId,
    urls: &SignedUrls,
    clock: &Clock,
    next: &Next,
) -> Result<(AccountLink, String)> {
    let now = clock.now();
    let pending = factory::issue(account, PURPOSE, now)?;
    let path = next.carried_by(&format!(
        "{}{}/{}",
        A::PREFIX,
        PURPOSE.segment(),
        pending.id().uuid()
    ));
    let location = signed_action(urls, &path, pending.expires_at(), now)?;
    Ok((pending, location))
}

async fn page<A: MemberArea>(
    link: LinkPage,
    urls: SignedUrls,
    clock: Clock,
    next: Next,
) -> Result<FormPage> {
    link.form_carrying::<A>(PURPOSE, &urls, &clock, &next)
}

async fn prove<A: MemberArea>(
    mut link: AccountLink,
    given: FactorCode,
    read: Read,
    session: Session,
    sealing: Sealing,
    clock: Clock,
) -> Result<Proof<(AccountLink, SignedIn)>> {
    let now = clock.now();
    if !link.is_open(PURPOSE, now) {
        return Err(Error::Rule(INVALID.to_owned()));
    }
    let account = link.account();
    let mut factor = super::required(&read, account)
        .await?
        .ok_or_else(|| Error::Rule(INVALID.to_owned()))?;
    if let Err(refused) = factor.prove(&given.code, &sealing, now)? {
        return Ok(Proof {
            factor,
            passed: Err(refused),
        });
    }
    link.use_up(PURPOSE, now)?;
    membership::require(&read, account, A::NAME).await?;
    let signed_in = sign_in::<A>(&session, account, &clock)?;
    Ok(Proof {
        factor,
        passed: Ok((link, signed_in)),
    })
}
