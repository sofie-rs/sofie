//! Data export and account closing. Closing anonymizes rather than deletes, so records of
//! other modules that name the account stay whole; [`AccountClosed`] tells those modules
//! to erase what they hold.

use std::sync::Arc;

use sofie::db::{BoxFuture, Db, Extensions, Outcome, Plan, Reply, Stamp, Uuid};
use sofie::http::{Routes, Shipped};
use sofie::prelude::*;
use sofie::serde_json::{Map, Value, json};

use crate::account::Account;
pub use crate::account::Closable;
use crate::membership::Membership;
use crate::signing_in::OwnSession;
use crate::{AccountId, MemberArea, account_of};

#[cfg(feature = "notify")]
pub use notifications::Notifications;

const CONFIRM: &str = "is not this account's address";

/// What a module holds of an account, exported beside sofie-identity's own records.
pub trait PersonalData: Send + Sync + 'static {
    fn name(&self) -> &'static str;

    fn export<'a>(&'a self, read: &'a Read, account: Uuid) -> BoxFuture<'a, Result<Value>>;

    /// Committed with the closing; an error refuses it, such as one naming what the account
    /// still holds alone.
    fn closing<'a>(
        &'a self,
        read: &'a Read,
        account: Uuid,
    ) -> BoxFuture<'a, Result<Vec<Box<dyn Closable>>>> {
        let _ = (read, account);
        Box::pin(std::future::ready(Ok(Vec::new())))
    }
}

pub fn register(db: &Db, data: Arc<dyn PersonalData>) {
    sofie::db::extend(db, data);
}

/// Raised when an account closes; a module listens with `#[on]` to erase or anonymize its rows.
#[derive(Event)]
pub struct AccountClosed {
    pub account: Uuid,
}

/// Each member exports and closes only its own account.
pub fn routes<A: MemberArea>(routes: &mut Routes<A>) {
    let describe: sofie::http::Describe =
        |screens, key| screens.describe_form::<Closing>(key, "close-account");
    routes
        .shipped(
            "/account-data",
            "account_data",
            (include_str!("screen.html"), Some(describe)),
            page,
        )
        .name("account_data_page");
    routes
        .post("/account-data/export", export::<A>)
        .name("export_account_data");
    routes
        .post("/account-data/close", close::<A>)
        .name("close_account")
        .then_redirect(A::SIGN_IN);
}

#[derive(ReadModel)]
pub struct Page {}

async fn page() -> Result<Page> {
    Ok(Page {})
}

pub struct Exported(Value);

impl Outcome for Exported {
    fn reply(self) -> Result<Reply> {
        Ok(Reply::Json(self.0))
    }
}

async fn export<A: MemberArea>(
    principal: Principal<A>,
    _own: OwnSession,
    read: Read,
    others: Extensions<dyn PersonalData>,
) -> Result<Exported> {
    let account = account_of(&principal);
    let mut exported = Map::new();
    exported.insert("sofie_identity".to_owned(), own(&read, account).await?);
    for data in others.iter() {
        exported.insert(
            data.name().to_owned(),
            data.export(&read, account.uuid()).await?,
        );
    }
    Ok(Exported(Value::Object(exported)))
}

async fn own(read: &Read, account: AccountId) -> Result<Value> {
    let held = Account::stored(read, account).await?;
    let memberships = Membership::query()
        .where_(Membership::ACCOUNT.eq(account))
        .all(read)
        .await?;
    #[cfg_attr(
        not(any(
            feature = "sessions",
            feature = "passkeys",
            feature = "oauth",
            feature = "totp",
            feature = "tokens",
            feature = "impersonation"
        )),
        expect(unused_mut)
    )]
    let mut own = json!({
        "account": {
            "email": held.email().as_str(),
            "created_at": held.created_at(),
            "verified_at": held.verified_at(),
        },
        "memberships": memberships.iter().map(|membership| json!({
            "area": membership.area(),
            "created_at": membership.created_at(),
        })).collect::<Vec<_>>(),
    });
    #[cfg(feature = "sessions")]
    {
        own["sessions"] = crate::sessions::exported(read, account).await?;
    }
    #[cfg(feature = "passkeys")]
    {
        own["passkeys"] = crate::passkeys::exported(read, account).await?;
    }
    #[cfg(feature = "oauth")]
    {
        own["connections"] = crate::oauth::exported(read, account).await?;
    }
    #[cfg(feature = "totp")]
    {
        own["second_factor"] = crate::two_factor::exported(read, account).await?;
    }
    #[cfg(feature = "tokens")]
    {
        own["tokens"] = crate::tokens::exported(read, account).await?;
    }
    #[cfg(feature = "impersonation")]
    {
        own["impersonations"] = crate::impersonation::exported(read, account).await?;
    }
    Ok(own)
}

#[derive(Input)]
pub struct Closing {
    /// Typed again, so no stray click closes an account.
    email: Email,
}

pub struct Closed {
    account: Account,
    records: Vec<Box<dyn Closable>>,
}

impl Outcome for Closed {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        Outcome::plan(&self.account, plan)?;
        self.records
            .iter()
            .try_for_each(|record| record.plan(plan))?;
        sofie::db::forget_principal(plan, self.account.id().uuid());
        Ok(())
    }

    fn committed(&mut self, stamp: Stamp) {
        Outcome::committed(&mut self.account, stamp);
        for record in &mut self.records {
            record.committed(stamp);
        }
    }
}

async fn close<A: MemberArea>(
    principal: Principal<A>,
    _own: OwnSession,
    closing: Closing,
    read: Read,
    others: Extensions<dyn PersonalData>,
    session: Session,
    clock: Clock,
) -> Result<Closed> {
    let id = account_of(&principal);
    let mut account = Account::stored(&read, id).await?;
    if *account.email() != closing.email {
        return Err(Error::invalid("email", CONFIRM));
    }
    account.close()?;
    account.raise(AccountClosed { account: id.uuid() })?;
    let mut records: Vec<Box<dyn Closable>> = Vec::new();
    #[cfg(feature = "account-links")]
    records.push(Box::new(
        crate::account_link::closing(&read, id, clock.now()).await?,
    ));
    #[cfg(feature = "sessions")]
    records.push(Box::new(
        crate::sessions::closing(&read, id, clock.now()).await?,
    ));
    #[cfg(feature = "passkeys")]
    records.push(Box::new(crate::passkeys::removing(&read, id).await?));
    #[cfg(feature = "oauth")]
    records.push(Box::new(crate::oauth::removing(&read, id).await?));
    #[cfg(feature = "totp")]
    records.push(Box::new(crate::two_factor::removing(&read, id).await?));
    #[cfg(feature = "tokens")]
    records.push(Box::new(crate::tokens::revoking(&read, id).await?));
    for data in others.iter() {
        records.extend(data.closing(&read, id.uuid()).await?);
    }
    #[cfg(not(any(feature = "sessions", feature = "account-links")))]
    let _ = clock;
    session.sign_out();
    Ok(Closed { account, records })
}

#[cfg(feature = "notify")]
mod notifications {
    use sofie::db::{BoxFuture, Db, LocalJobs, Reach, Uuid};
    use sofie::prelude::*;
    use sofie::serde_json::Value;

    use super::{Closable, PersonalData};

    /// For [`super::register`] in an app with sofie's `notify`: a closing account's
    /// notifications, choices and browsers are erased and its digest stops.
    pub struct Notifications(Db);

    impl Notifications {
        #[must_use]
        pub fn new(db: &Db) -> Self {
            Self(db.clone())
        }
    }

    impl PersonalData for Notifications {
        fn name(&self) -> &'static str {
            "sofie_notify"
        }

        fn export<'a>(&'a self, read: &'a Read, account: Uuid) -> BoxFuture<'a, Result<Value>> {
            Box::pin(sofie::notify::personal_data::exported(read, account))
        }

        fn closing<'a>(
            &'a self,
            _read: &'a Read,
            account: Uuid,
        ) -> BoxFuture<'a, Result<Vec<Box<dyn Closable>>>> {
            Box::pin(async move {
                let jobs = LocalJobs::within(&self.0, Reach::OwnedBy(account));
                let forgotten = sofie::notify::personal_data::forgetting(&jobs, account).await?;
                let closing: Vec<Box<dyn Closable>> = vec![Box::new(forgotten)];
                Ok(closing)
            })
        }
    }
}
