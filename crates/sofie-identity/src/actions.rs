use sofie::db::{Outcome, Plan, Stamp, Uuid};
use sofie::prelude::*;
use sofie::values::Secret;

use crate::account::{ALREADY_REGISTERED, Account, EMAIL_INDEX, factory};
use crate::membership::{self, Membership};
use crate::signing_in::{Door, Entered};
use crate::{AccountId, MemberArea, Password, PasswordCheck, PasswordHash};

pub(crate) const NOT_RIGHT: &str = "The email or password is not right.";
/// One answer for a wrong password and an unconfirmed address, so signing in with the password
/// one signed up with tells nobody whether the address was free.
#[cfg(feature = "email-verification")]
const NOT_RIGHT_OR_UNCONFIRMED: &str = "The email or password is not right, or the address is \
                                        not confirmed yet through the link mailed to it; a new \
                                        link can be asked for.";

#[derive(Input)]
pub struct Credentials {
    email: Email,
    password: Password,
}

#[derive(Input)]
pub struct SignInCredentials {
    email: Email,
    password: Secret<String>,
}

/// The account of the address given, if any, and whether the password matched it.
pub(crate) struct Attempt {
    pub(crate) account: Option<Account>,
    pub(crate) matches: bool,
    password: Password,
    check: PasswordCheck,
}

impl SignInCredentials {
    /// Takes as long whether or not the address has an account.
    pub(crate) async fn attempt(
        self,
        read: &Read,
        #[cfg(feature = "shield")] sign_ins: &sofie::http::SignIns,
    ) -> Result<Attempt> {
        let account = Account::of_email(read, self.email).await?;
        let hash = account.as_ref().map(Account::password_hash);
        let password = Password::entered(self.password.expose().clone());
        let check = password.check(hash).await?;
        let matches = check.is_right();
        // A right password is no failed attempt, even where it does not admit, so it never locks.
        #[cfg(feature = "shield")]
        let matches = crate::lock::admits(sign_ins, account.as_ref(), matches);
        Ok(Attempt {
            account,
            matches,
            password,
            check,
        })
    }
}

impl Attempt {
    /// An older scheme's hash, or one from before NFC, is replaced once the password proved
    /// right; until then a wrong password takes that scheme's time.
    pub(crate) async fn rehash(&self, account: &mut Account) -> Result<bool> {
        if self.check != PasswordCheck::RightAsTyped && !account.password_hash().needs_rehash() {
            return Ok(false);
        }
        account.change_password(PasswordHash::new(&self.password).await?);
        Ok(true)
    }
}

/// Replies nothing and signs nobody in, whether the address was free or taken.
pub struct Registration {
    opened: Option<(Account, Membership)>,
    /// Tells a taken address's owner that someone tried, or invites it to join.
    #[cfg(feature = "links")]
    told: Option<crate::notice::Notice>,
}

impl Outcome for Registration {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        if let Some((account, membership)) = &self.opened {
            plan.unique(EMAIL_INDEX, "email", ALREADY_REGISTERED);
            account.plan(plan)?;
            membership.plan(plan)?;
        }
        #[cfg(feature = "links")]
        if let Some(told) = &self.told {
            told.plan(plan)?;
        }
        Ok(())
    }

    fn committed(&mut self, stamp: Stamp) {
        if let Some((account, membership)) = &mut self.opened {
            account.committed(stamp);
            membership.committed(stamp);
        }
    }
}

/// Answers alike, after the same hashing, whether or not the address has an account, and
/// signs nobody in. A new address gets its account and, with `email-verification`, a link to
/// confirm it. A taken address's owner is mailed: a member of the area that someone tried,
/// with a link to set a new password (`password-reset`), any other account a link to sign in
/// and join the area. Only two sign-ups racing for one address are told it is taken.
pub async fn sign_up<A: MemberArea>(
    credentials: Credentials,
    read: Read,
    #[cfg(feature = "shield")] sign_ins: sofie::http::SignIns,
) -> Result<Registration> {
    #[cfg(feature = "shield")]
    sign_ins.signed_up();
    let email = credentials.email;
    let Some(account) = factory::register(&read, email.clone(), &credentials.password).await?
    else {
        #[cfg(feature = "links")]
        let told = Some(crate::notice::Notice::of(
            crate::mailed::LinkAsked::sign_up_repeated::<A>(email),
        )?);
        return Ok(Registration {
            opened: None,
            #[cfg(feature = "links")]
            told,
        });
    };
    #[cfg(feature = "email-verification")]
    let account = account.asking_verification::<A>()?;
    let membership = membership::factory::grant(account.id(), A::NAME)?;
    Ok(Registration {
        opened: Some((account, membership)),
        #[cfg(feature = "links")]
        told: None,
    })
}

/// Timing does not tell which emails have accounts, and a right password for an account
/// outside the area, or one still unconfirmed where the area needs it, is answered as a wrong
/// one, so it confirms nothing.
pub async fn sign_in<A: MemberArea>(
    credentials: SignInCredentials,
    door: Door,
    #[cfg(feature = "shield")] sign_ins: sofie::http::SignIns,
) -> Result<Entered<Option<Account>>> {
    let read = door.read();
    let mut attempt = credentials
        .attempt(
            read,
            #[cfg(feature = "shield")]
            &sign_ins,
        )
        .await?;
    // Asked for an address without an account too, under an id no account has.
    let asked = attempt
        .account
        .as_ref()
        .map_or(AccountId::from_uuid(Uuid::from_u128(0)), Account::id);
    let held = membership::held(read, asked, A::NAME).await?;
    let member = held.is_some() && attempt.account.is_some();
    let (confirmed, not_right) = address_rule::<A>(attempt.account.as_ref());
    let Some(mut account) = attempt
        .account
        .take_if(|_| attempt.matches && member && confirmed)
    else {
        return Err(Error::Rule(not_right.to_owned()));
    };
    let entered = door.enter::<A>(account.id()).await?;
    let rehashed = attempt.rehash(&mut account).await?;
    Ok(entered.saving(rehashed.then_some(account)))
}

#[cfg_attr(
    not(feature = "email-verification"),
    expect(
        clippy::extra_unused_type_parameters,
        reason = "only verification asks the area"
    )
)]
fn address_rule<A: MemberArea>(account: Option<&Account>) -> (bool, &'static str) {
    #[cfg(feature = "email-verification")]
    if A::VERIFIED_EMAIL {
        let confirmed = account.is_some_and(Account::is_confirmed);
        return (confirmed, NOT_RIGHT_OR_UNCONFIRMED);
    }
    let _ = account;
    (true, NOT_RIGHT)
}

#[cfg(not(feature = "sessions"))]
pub async fn sign_out(session: Session) -> Result<()> {
    session.sign_out();
    Ok(())
}

/// Its record is ended too, so the cookie, if kept somewhere, opens nothing more.
#[cfg(feature = "sessions")]
pub async fn sign_out(
    session: Session,
    read: Read,
    clock: Clock,
) -> Result<(
    Vec<crate::sessions::AccountSession>,
    crate::sessions::Impersonations,
)> {
    session.sign_out();
    let ended = crate::sessions::ending_current(&read, &session, &clock).await?;
    let impersonations = crate::sessions::impersonations_in(&read, &ended, clock.now()).await?;
    Ok((ended, impersonations))
}
