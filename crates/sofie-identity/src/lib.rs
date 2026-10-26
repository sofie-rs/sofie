mod account;
#[cfg(feature = "account-data")]
pub mod account_data;
#[cfg(feature = "account-links")]
mod account_link;
mod actions;
#[cfg(feature = "impersonation")]
pub mod impersonation;
#[cfg(feature = "shield")]
mod lock;
#[cfg(feature = "links")]
mod mailed;
mod membership;
#[cfg(any(feature = "shield", feature = "links"))]
mod notice;
#[cfg(feature = "oauth")]
pub mod oauth;
mod pages;
#[cfg(feature = "passkeys")]
pub mod passkeys;
mod password;
#[cfg(feature = "permissions")]
mod permitted;
#[cfg(feature = "permissions")]
mod role;
#[cfg(feature = "permissions")]
mod role_grant;
mod route_table;
#[cfg(feature = "sessions")]
pub mod sessions;
mod signing_in;
#[cfg(feature = "testing")]
pub mod testing;
#[cfg(feature = "tokens")]
pub mod tokens;
#[cfg(feature = "totp")]
pub mod two_factor;

use sofie::Module;
use sofie::db::{File, Migrations};
use sofie::http::{App, Area, AreaPrincipal, Form, Mount, Rate, Routes, Web};
use sofie::worker::Worker;

pub use account::Account;
#[cfg(feature = "shield")]
pub use lock::SignInLocked;
pub use membership::{
    AddingMember, MemberAdded, MembershipRemoved, Memberships, add_member, add_member_task,
    remove_membership,
};
pub use password::{Password, PasswordCheck, PasswordHash};
#[cfg(feature = "permissions")]
pub use permitted::*;
pub use route_table::ROUTES;
pub use signing_in::{SignedIn, sign_in_for};

pub type AccountId = sofie::db::Id<Account>;

/// The whole database, for records beyond the request's reach, such as other accounts'.
#[doc(hidden)]
pub struct Unscoped(pub sofie::db::Db);

impl<C: sofie::db::Context> sofie::db::Resolve<C> for Unscoped {
    fn resolve(context: &mut C) -> impl Future<Output = sofie::core::Result<Self>> + Send {
        std::future::ready(Ok(Self(context.db().clone())))
    }
}

const NAME: &str = "sofie_identity";

macro_rules! screen {
    ($name:literal) => {
        (
            concat!("sofie_identity/web/", $name),
            sofie::db::embed!(
                concat!("src/screens/", $name, ".html"),
                compiled = concat!("screens/sofie_identity/web/", $name, ".sft")
            ),
        )
    };
}

const SCREENS: &[(&str, File)] = &[
    screen!("sign_in"),
    screen!("sign_up"),
    #[cfg(feature = "links")]
    screen!("link_sent"),
    #[cfg(feature = "password-reset")]
    screen!("forgot_password"),
    #[cfg(feature = "password-reset")]
    screen!("reset_password"),
    #[cfg(feature = "email-verification")]
    screen!("ask_verification"),
    #[cfg(feature = "email-verification")]
    screen!("verify_email"),
    #[cfg(feature = "links")]
    screen!("join"),
    #[cfg(feature = "sign-in-links")]
    screen!("ask_sign_in_link"),
    #[cfg(feature = "sign-in-links")]
    screen!("sign_in_link"),
    #[cfg(feature = "totp")]
    screen!("second_factor"),
    #[cfg(feature = "oauth")]
    screen!("oauth_return"),
];

pub const FORMS: &[Form] = &[
    Form::of::<actions::SignInCredentials>("sofie_identity/web/sign_in", "sign-in"),
    Form::of::<actions::Credentials>("sofie_identity/web/sign_up", "sign-up"),
    #[cfg(feature = "links")]
    Form::of::<actions::SignInCredentials>("sofie_identity/web/join", "join"),
    #[cfg(feature = "password-reset")]
    Form::of::<mailed::AskedLink>("sofie_identity/web/forgot_password", "forgot-password"),
    #[cfg(feature = "password-reset")]
    Form::of::<mailed::NewPassword>("sofie_identity/web/reset_password", "reset-password"),
    #[cfg(feature = "email-verification")]
    Form::of::<mailed::AskedLink>("sofie_identity/web/ask_verification", "ask-verification"),
    #[cfg(feature = "sign-in-links")]
    Form::of::<mailed::AskedLink>("sofie_identity/web/ask_sign_in_link", "ask-sign-in-link"),
    #[cfg(feature = "totp")]
    Form::of::<two_factor::FactorCode>("sofie_identity/web/second_factor", "second-factor"),
];

pub const TRANSLATIONS: &[File] = sofie::db::embed!("lang", ["de.toml", "en.toml", "tr.toml"]);

/// Each file has a SQLite one of the same name, as `Migrations::sqlite` matches them by name.
macro_rules! migrations {
    ($($file:literal),+ $(,)?) => {
        Migrations::new(NAME, sofie::db::embed!("migrations", [$($file),+]))
            .sqlite(sofie::db::embed!("migrations/sqlite", [$($file),+]))
    };
}

#[must_use]
pub const fn module(members: &'static [Mount]) -> Module {
    Module {
        migrations: Some(migrations()),
        screens: SCREENS,
        forms: FORMS,
        areas: members,
        jobs: worker,
        ..Module::new(NAME)
    }
}

#[must_use]
pub fn member<A: MemberArea>(app: App, module: &'static str) -> App {
    app.membership(Memberships).area::<Web>(module, routes::<A>)
}

#[must_use]
pub const fn migrations() -> Migrations {
    migrations![
        "0000_module_role.sql",
        "0001_create_account.sql",
        "0002_timestamps.sql",
        "0003_roles.sql",
        "0004_role_grant_index.sql",
        "0005_access_tokens.sql",
        "0006_account_links.sql",
        "0007_account_sessions.sql",
        "0008_sign_in_links.sql",
        "0009_second_factor.sql",
        "0010_passkeys.sql",
        "0011_account_identities.sql",
        "0012_impersonations.sql",
        "0013_passkey_credential_live.sql",
        "0014_account_session_principal.sql",
        "0015_join_links.sql",
        "0016_impersonation_granted.sql",
        "0017_impersonation_started_from.sql",
        "0018_impersonation_impersonator.sql",
        "0019_account_link_expires.sql",
    ]
    .aggregates(AGGREGATES)
}

const AGGREGATES: &[sofie::db::AggregateTable] = &[
    sofie::db::AggregateTable::of::<Account>(),
    sofie::db::AggregateTable::of::<membership::Membership>(),
    #[cfg(feature = "account-links")]
    sofie::db::AggregateTable::of::<account_link::AccountLink>(),
    #[cfg(feature = "permissions")]
    sofie::db::AggregateTable::of::<role::Role>(),
    #[cfg(feature = "permissions")]
    sofie::db::AggregateTable::of::<role_grant::RoleGrant>(),
    #[cfg(feature = "tokens")]
    sofie::db::AggregateTable::of::<tokens::AccessToken>(),
    #[cfg(feature = "sessions")]
    sofie::db::AggregateTable::of::<sessions::AccountSession>(),
    #[cfg(feature = "totp")]
    sofie::db::AggregateTable::of::<two_factor::SecondFactor>(),
    #[cfg(feature = "passkeys")]
    sofie::db::AggregateTable::of::<passkeys::Passkey>(),
    #[cfg(feature = "oauth")]
    sofie::db::AggregateTable::of::<oauth::AccountIdentity>(),
    #[cfg(feature = "impersonation")]
    sofie::db::AggregateTable::of::<impersonation::Impersonation>(),
];

pub trait MemberArea: Area<Principal: AreaPrincipal<Record = Account>> {
    const HOME: &'static str;
    /// Signing up there signs nobody in and answers alike for a new address and a taken one.
    const SIGN_UP: Option<&'static str> = None;
    /// Without it an unconfirmed account signs in, so signing in with the password one just
    /// signed up with tells a new address from a taken one.
    #[cfg(feature = "email-verification")]
    const VERIFIED_EMAIL: bool = false;
}

pub trait SignUp: Area {
    const PATH: &'static str;
}

/// Mounted in `Web`; sign-ins and sign-ups are rate-limited per address.
pub fn routes<A: MemberArea>(routes: &mut Routes<Web>) {
    routes
        .screen(A::SIGN_IN, "sign_in", pages::sign_in_page::<A>)
        .name_for::<A>("sign_in_page");
    routes
        .post(A::SIGN_IN, actions::sign_in::<A>)
        .name_for::<A>("sign_in")
        .throttle(Rate::per_minute(10))
        .then_redirect_or_follow(A::HOME);
    routes
        .post(&format!("{}/sign-out", A::PREFIX), actions::sign_out)
        .name_for::<A>("sign_out")
        .then_redirect(A::SIGN_IN);
    if let Some(sign_up) = A::SIGN_UP {
        routes
            .screen(sign_up, "sign_up", pages::sign_up_page::<A>)
            .name_for::<A>("sign_up_page");
        routes
            .post(sign_up, actions::sign_up::<A>)
            .name_for::<A>("sign_up")
            .throttle(Rate::per_minute(10))
            .then_redirect(&after_sign_up::<A>());
    }
    #[cfg(feature = "links")]
    mailed::routes::<A>(routes);
    #[cfg(feature = "totp")]
    two_factor::challenge_routes::<A>(routes);
    #[cfg(feature = "passkeys")]
    passkeys::sign_in_routes::<A>(routes);
    #[cfg(feature = "oauth")]
    oauth::sign_in_routes::<A>(routes);
}

/// Where a sign-up lands, whether or not its address was free: told to check its mail when a
/// link is on its way, else at signing in.
fn after_sign_up<A: MemberArea>() -> String {
    #[cfg(feature = "email-verification")]
    return mailed::link_sent::<A>();
    #[cfg(not(feature = "email-verification"))]
    A::SIGN_IN.to_owned()
}

#[must_use]
pub fn worker(worker: Worker) -> Worker {
    #[cfg(feature = "links")]
    let worker = worker.on::<mailed::LinkAsked, _>("sofie_identity.mail_link", mailed::mail_link);
    #[cfg(feature = "sessions")]
    let worker = worker.every(
        "sofie_identity.prune_sessions",
        sessions::PRUNE_EVERY,
        sessions::prune,
    );
    #[cfg(feature = "account-links")]
    let worker = worker.every(
        "sofie_identity.prune_account_links",
        account_link::PRUNE_EVERY,
        account_link::prune,
    );
    worker
}

#[cfg(any(
    feature = "session-screens",
    feature = "token-screens",
    feature = "totp",
    feature = "passkeys",
    feature = "oauth",
    feature = "account-data",
    feature = "impersonation"
))]
fn account_of<A: Area>(principal: &sofie::http::Principal<A>) -> AccountId {
    AccountId::from_uuid(principal.id().uuid())
}
