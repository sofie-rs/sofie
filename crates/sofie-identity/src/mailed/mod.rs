//! Asking for a link answers the same whether or not the address has an account.

mod join;
#[cfg(feature = "password-reset")]
mod reset;
#[cfg(feature = "sign-in-links")]
pub(crate) mod sign_in_link;
#[cfg(feature = "email-verification")]
mod verification;

use std::time::Duration;

use sofie::db::Queued;
use sofie::http::{Rate, Routes, SignedUrls, Web};
use sofie::mail::{Address, Mail, Mailer, Template};
use sofie::prelude::*;
use sofie::serde_json::json;

use crate::MemberArea;
use crate::account::Account;
use crate::account_link::{AccountLink, LinkPage, LinkPurpose, factory};
use crate::membership::held;
use crate::notice::Notice;
use crate::pages::FormPage;

#[cfg(feature = "password-reset")]
pub(crate) use reset::NewPassword;

const LINK_SENT: &str = "/link-sent";
/// Asking again within the hour mails nothing once this many links were mailed.
const MAXIMUM_PER_HOUR: u64 = 3;
const HOUR: Duration = Duration::from_hours(1);
/// Per address, on top of the per-account limit, since asking costs the asker nothing.
const ASKING: Rate = Rate::per_minute(5);

macro_rules! mail {
    ($name:literal) => {
        Template::new(
            concat!("sofie_identity/", $name),
            sofie::db::embed!(
                concat!("src/mails/", $name, ".html"),
                compiled = concat!("mails/sofie_identity/", $name, ".sft")
            ),
        )
    };
}

const PASSWORD_RESET_MAIL: Template = mail!("password_reset");
const VERIFICATION_MAIL: Template = mail!("email_verification");
const SIGN_IN_MAIL: Template = mail!("sign_in_link");
const SIGN_UP_REPEATED_MAIL: Template = mail!("sign_up_repeated");
const JOIN_MAIL: Template = mail!("join");

/// Committed by the request, so the worker, not the request, finds out whether the address has an account.
#[derive(Event)]
pub(crate) struct LinkAsked {
    email: Email,
    area: String,
    prefix: String,
    purpose: LinkPurpose,
    /// Someone signed up with the address of an account: a member gets a reset link saying
    /// so, an account of another area a link to sign in and join this one.
    sign_up_repeated: bool,
}

impl LinkAsked {
    pub(crate) fn of<A: MemberArea>(email: Email, purpose: LinkPurpose) -> Self {
        Self {
            email,
            area: A::NAME.to_owned(),
            prefix: A::PREFIX.to_owned(),
            purpose,
            sign_up_repeated: false,
        }
    }

    pub(crate) fn sign_up_repeated<A: MemberArea>(email: Email) -> Self {
        Self {
            sign_up_repeated: true,
            ..Self::of::<A>(email, LinkPurpose::PasswordReset)
        }
    }
}

#[derive(Input)]
pub struct AskedLink {
    email: Email,
}

pub(crate) fn link_sent<A: MemberArea>() -> String {
    format!("{}{LINK_SENT}", A::PREFIX)
}

pub(crate) fn routes<A: MemberArea>(routes: &mut Routes<Web>) {
    routes
        .screen(&link_sent::<A>(), "link_sent", link_sent_page::<A>)
        .name_for::<A>("link_sent_page");
    if A::SIGN_UP.is_some() {
        join::routes::<A>(routes);
    }
    #[cfg(feature = "password-reset")]
    reset::routes::<A>(routes);
    #[cfg(feature = "email-verification")]
    verification::routes::<A>(routes);
    #[cfg(feature = "sign-in-links")]
    sign_in_link::routes::<A>(routes);
}

async fn link_sent_page<A: MemberArea>() -> Result<FormPage> {
    Ok(FormPage::new(A::SIGN_IN.to_owned(), Some(A::SIGN_IN)))
}

trait Mailed: 'static {
    const PURPOSE: LinkPurpose;
    const ASKING: &'static str = Self::PURPOSE.segment();
    const SCREENS: [&'static str; 2];
    const NAMES: [&'static str; 3];
}

fn link_routes<A: MemberArea, M: Mailed>(routes: &mut Routes<Web>) -> String {
    let asking = format!("{}{}", A::PREFIX, M::ASKING);
    let [ask_screen, link_screen] = M::SCREENS;
    let [ask_page_name, ask_name, link_page_name] = M::NAMES;
    routes
        .screen(&asking, ask_screen, ask_page::<A, M>)
        .name_for::<A>(ask_page_name);
    routes
        .post(&asking, ask::<A, M>)
        .name_for::<A>(ask_name)
        .throttle(ASKING)
        .then_redirect(&link_sent::<A>());
    let link = format!("{}{}/{{account_link}}", A::PREFIX, M::PURPOSE.segment());
    routes
        .screen(&link, link_screen, link_page::<A, M>)
        .name_for::<A>(link_page_name)
        .signed();
    link
}

async fn ask_page<A: MemberArea, M: Mailed>() -> Result<FormPage> {
    Ok(FormPage::new(
        format!("{}{}", A::PREFIX, M::ASKING),
        Some(A::SIGN_IN),
    ))
}

async fn ask<A: MemberArea, M: Mailed>(asked: AskedLink) -> Result<Notice> {
    Notice::of(LinkAsked::of::<A>(asked.email, M::PURPOSE))
}

async fn link_page<A: MemberArea, M: Mailed>(
    link: LinkPage,
    urls: SignedUrls,
    clock: Clock,
) -> Result<FormPage> {
    link.form::<A>(M::PURPOSE, &urls, &clock)
}

/// Mails nothing to an address without an account in the area, unless it signed up there,
/// nor past the hourly limit.
pub(crate) async fn mail_link(
    asked: LinkAsked,
    read: Read,
    mailer: Mailer,
    urls: SignedUrls,
    clock: Clock,
) -> Result<Vec<(AccountLink, Queued)>> {
    let Some((account, purpose)) = eligible(&asked, &read).await? else {
        return Ok(Vec::new());
    };
    let now = clock.now();
    let recent = AccountLink::query()
        .where_(AccountLink::ACCOUNT.eq(account.id()))
        .where_(AccountLink::PURPOSE.eq(purpose))
        .where_(AccountLink::CREATED_AT.gt(now.checked_sub(HOUR)?))
        .count(&read)
        .await?;
    if recent >= MAXIMUM_PER_HOUR {
        return Ok(Vec::new());
    }
    let link = factory::issue(account.id(), purpose, now)?;
    let path = format!("{}{}/{}", asked.prefix, purpose.segment(), link.id().uuid());
    let signed = crate::account_link::signed_action(&urls, &path, link.expires_at(), now)?;
    let (template, subject) = match purpose {
        LinkPurpose::PasswordReset if asked.sign_up_repeated => {
            (&SIGN_UP_REPEATED_MAIL, "You already have an account")
        }
        LinkPurpose::PasswordReset => (&PASSWORD_RESET_MAIL, "Reset your password"),
        LinkPurpose::EmailVerification => (&VERIFICATION_MAIL, "Confirm your email address"),
        LinkPurpose::SignIn => (&SIGN_IN_MAIL, "Sign in"),
        LinkPurpose::Join => (&JOIN_MAIL, "Sign in to join"),
        LinkPurpose::SecondFactor => {
            return Err(Error::internal_message(
                "a second factor's link is never mailed",
            ));
        }
    };
    let body = mailer.body_from(template, &json!({ "link": mailer.link(&signed)? }))?;
    let mail = Mail::new(Address::new(account.email())?, subject, body)?;
    Ok(vec![(link, mailer.send(mail)?)])
}

async fn eligible(asked: &LinkAsked, read: &Read) -> Result<Option<(Account, LinkPurpose)>> {
    let Some(account) = Account::of_email(read, asked.email.clone()).await? else {
        return Ok(None);
    };
    let member = held(read, account.id(), &asked.area).await?.is_some();
    if asked.sign_up_repeated && !member {
        return Ok(Some((account, LinkPurpose::Join)));
    }
    if !member || (asked.sign_up_repeated && cfg!(not(feature = "password-reset"))) {
        return Ok(None);
    }
    #[cfg(feature = "email-verification")]
    if asked.purpose == LinkPurpose::EmailVerification && account.is_confirmed() {
        return Ok(None);
    }
    Ok(Some((account, asked.purpose)))
}
