use sofie::http::Next;
use sofie::prelude::*;

use crate::MemberArea;

/// `other` is a page to go to instead, linked under the form.
#[derive(ReadModel)]
pub struct FormPage {
    action: String,
    other: Option<String>,
}

impl FormPage {
    #[cfg(feature = "account-links")]
    pub(crate) fn new(action: String, other: Option<&'static str>) -> Self {
        Self {
            action,
            other: other.map(str::to_owned),
        }
    }
}

#[derive(ReadModel)]
pub struct SignInPage {
    action: String,
    other: Option<String>,
    by_link: Option<String>,
    passkey: Option<PasskeyForm>,
    oauth: Option<Offered>,
}

/// The options and address `engine.js` signs in with a passkey through.
#[derive(ReadModel)]
pub struct PasskeyForm {
    options: String,
    post: String,
}

#[derive(ReadModel)]
pub struct Offered {
    pub(crate) action: String,
    pub(crate) providers: Vec<Choice>,
}

#[derive(ReadModel)]
pub struct Choice {
    pub(crate) name: String,
    pub(crate) label: String,
}

pub async fn sign_in_page<A: MemberArea>(
    next: Next,
    #[cfg(feature = "passkeys")] sealing: sofie::http::Sealing,
    #[cfg(feature = "passkeys")] clock: Clock,
    #[cfg(feature = "oauth")] installed: crate::oauth::Installed,
) -> Result<SignInPage> {
    Ok(SignInPage {
        action: next.carried_by(A::SIGN_IN),
        other: A::SIGN_UP.map(|sign_up| next.carried_by(sign_up)),
        #[cfg(feature = "sign-in-links")]
        by_link: Some(crate::mailed::sign_in_link::asking::<A>()),
        #[cfg(not(feature = "sign-in-links"))]
        by_link: None,
        #[cfg(feature = "passkeys")]
        passkey: Some(PasskeyForm {
            options: crate::passkeys::sign_in_options(&sealing, &clock)?,
            post: crate::passkeys::sign_in_path::<A>(),
        }),
        #[cfg(not(feature = "passkeys"))]
        passkey: None,
        #[cfg(feature = "oauth")]
        oauth: crate::oauth::offered::<A>(&installed),
        #[cfg(not(feature = "oauth"))]
        oauth: None,
    })
}

/// `next` is carried to the form and the sign-in link, so a new member lands where it was going.
pub async fn sign_up_page<A: MemberArea>(next: Next) -> Result<FormPage> {
    Ok(FormPage {
        action: next.carried_by(A::SIGN_UP.unwrap_or(A::SIGN_IN)),
        other: Some(next.carried_by(A::SIGN_IN)),
    })
}
