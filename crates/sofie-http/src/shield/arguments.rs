use std::sync::Arc;

use sofie_core::{Error, Result, Timestamp, Uuid};

use super::blocks::{Grade, Key};
use super::detector::{Message, Signal};
use super::rule::Rule;
use super::{RequestShield, Sentinel, effects};
use crate::throttle::Requester;
use crate::{Area, RequestContext};
use sofie_db::Resolve;

/// Without the shield running it counts nothing and locks nothing.
pub struct SignIns {
    pub(super) request: Option<RequestShield>,
}

impl SignIns {
    /// For this caller: another caller's failures lock it out only past the account-wide count.
    #[must_use]
    pub fn locked(&self, account: Uuid) -> bool {
        self.request.as_ref().is_some_and(|request| {
            let sentinel = &request.sentinel;
            let from = Key::AccountFrom(account, request.anonymous());
            [Key::Account(account), from].into_iter().any(|key| {
                sentinel
                    .blocks
                    .find(key, sentinel.now())
                    .is_some_and(|block| block.grade == Grade::Lock)
            })
        })
    }

    pub fn failed(&self, account: Option<Uuid>) {
        let Some(request) = &self.request else {
            return;
        };
        request.count(Rule::SignIn, Key::Caller(request.anonymous()));
        if let Some(account) = account {
            let from = Key::AccountFrom(account, request.anonymous());
            request.count(Rule::Account, from);
            request.count(Rule::AccountTotal, Key::Account(account));
        }
    }

    pub fn signed_up(&self) {
        if let Some(request) = &self.request {
            request.count(Rule::SignUp, Key::Caller(request.anonymous()));
        }
    }
}

impl RequestShield {
    /// By address even when signed in, as whoever signs in next from there is someone else.
    fn anonymous(&self) -> Requester {
        Requester::of(None, self.address)
    }

    fn count(&self, rule: Rule, key: Key) {
        self.sentinel.post(Message::Signal(Signal {
            rule,
            key,
            address: self.address,
            area: Some(self.area),
            at: self.sentinel.now(),
        }));
    }
}

impl<A: Area> Resolve<RequestContext<A>> for SignIns {
    fn resolve(context: &mut RequestContext<A>) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(Ok(Self {
            request: context.matched.shield.clone(),
        }))
    }
}

pub struct ShieldBlocks {
    sentinel: Option<Arc<Sentinel>>,
    keys: Arc<sofie_auth::SigningKeys>,
}

/// An address is shown by its network only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveBlock {
    pub id: String,
    pub caller: String,
    pub rule: &'static str,
    pub grade: &'static str,
    pub until: Timestamp,
}

impl ShieldBlocks {
    #[must_use]
    pub fn active(&self) -> Vec<ActiveBlock> {
        let Some(sentinel) = &self.sentinel else {
            return Vec::new();
        };
        sentinel
            .blocks
            .active(sentinel.now())
            .into_iter()
            .map(|(key, block)| ActiveBlock {
                id: key.id(&self.keys),
                caller: key.shown(),
                rule: block.rule.name(),
                grade: block.grade.name(),
                until: Timestamp::from_microsecond(block.until).unwrap_or(Timestamp::MAX),
            })
            .collect()
    }

    /// Also on every instance when shared; the caller's counts start over.
    pub async fn lift(&self, id: &str) -> Result<()> {
        let sentinel = self.sentinel.as_ref().ok_or(Error::NotFound)?;
        let key = sentinel
            .blocks
            .lift(id, &self.keys)
            .ok_or(Error::NotFound)?;
        sentinel.post(Message::Lifted(key));
        tracing::warn!(shield.caller = %key.shown(), "a shield block was lifted");
        match &sentinel.shared {
            Some(db) => effects::lift_shared(db, key).await,
            None => Ok(()),
        }
    }

    fn of<A: Area, M>(context: &RequestContext<A, M>) -> Self {
        Self {
            sentinel: context
                .matched
                .shield
                .as_ref()
                .map(|request| Arc::clone(&request.sentinel)),
            keys: Arc::clone(&context.matched.keys),
        }
    }
}

impl<A: Area> Resolve<RequestContext<A>> for ShieldBlocks {
    fn resolve(context: &mut RequestContext<A>) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(Ok(Self::of(context)))
    }
}

/// What a GET may take: lifting a block needs `ShieldBlocks`, in a POST.
pub struct ActiveBlocks(ShieldBlocks);

impl ActiveBlocks {
    #[must_use]
    pub fn list(&self) -> Vec<ActiveBlock> {
        self.0.active()
    }
}

impl<A: Area, M> Resolve<RequestContext<A, M>> for ActiveBlocks {
    fn resolve(context: &mut RequestContext<A, M>) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(Ok(Self(ShieldBlocks::of(context))))
    }
}
