mod challenge;
mod code;
mod qr;
mod screen;

use std::time::Duration;

use sofie::core::Instant;
use sofie::db::{Outcome, Plan, Reply, Stamp};
use sofie::http::Sealing;
use sofie::prelude::*;

pub(crate) use challenge::{FactorCode, asking, routes as challenge_routes};
pub use screen::routes;

use crate::AccountId;

const SECRET: &str = "sofie_identity.totp";
const RECOVERY: &str = "sofie_identity.recovery";
/// Wrong codes allowed per window before codes are refused until it ends.
const MAXIMUM_FAILURES: i32 = 5;
const FAILURE_WINDOW: Duration = Duration::from_mins(15);
const NOT_RIGHT: &str =
    "is not the current code of your authenticator app or an unused recovery code";
const TOO_MANY: &str = "Too many wrong codes; try again in fifteen minutes.";

/// Its key is sealed with the app's keys; a recovery code is kept only as its digest
/// once the factor is confirmed.
#[derive(Aggregate)]
pub struct SecondFactor {
    meta: Meta,
    #[owner]
    account: AccountId,
    secret: String,
    /// Until a code proved the app holds the key, signing in asks for nothing.
    confirmed_at: Option<Instant>,
    /// A code is accepted once: only for a later step than this one.
    last_step: i64,
    /// Digests of the unused recovery codes, separated by spaces.
    recovery: String,
    /// The recovery codes themselves, sealed, shown until the factor is confirmed.
    pending_recovery: Option<String>,
    failures: i32,
    failing_since: Option<Instant>,
}

impl SecondFactor {
    pub(crate) fn is_confirmed(&self) -> bool {
        self.confirmed_at.is_some()
    }

    fn key(&self, sealing: &Sealing) -> Result<Vec<u8>> {
        sealing.open(SECRET, &self.secret).ok_or_else(|| {
            Error::internal_message("a second factor's key does not open with the app's keys")
        })
    }

    /// Checks `code` and records what it used up or that it failed; the caller commits either.
    pub(crate) fn prove(
        &mut self,
        code: &str,
        sealing: &Sealing,
        now: Instant,
    ) -> Result<Result<()>> {
        if self.failing(now)? && self.failures >= MAXIMUM_FAILURES {
            return Ok(Err(Error::Rule(TOO_MANY.to_owned())));
        }
        if self.used_recovery(code) {
            return Ok(Ok(()));
        }
        let key = self.key(sealing)?;
        // Resealed with the current key, so retiring the one that sealed it locks nobody out.
        self.secret = sealing.seal(SECRET, &key)?;
        let now_seconds = now.timestamp().as_second();
        if let Some(step) = code::matching_step(&key, code, now_seconds, self.last_step) {
            self.last_step = step;
            self.failures = 0;
            self.failing_since = None;
            return Ok(Ok(()));
        }
        self.failed(now)?;
        Ok(Err(Error::invalid("code", NOT_RIGHT)))
    }

    fn used_recovery(&mut self, code: &str) -> bool {
        let digest = code::recovery_digest(code);
        let unused: Vec<&str> = self.recovery.split_whitespace().collect();
        if !self.is_confirmed() || !unused.contains(&digest.as_str()) {
            return false;
        }
        self.recovery = unused
            .into_iter()
            .filter(|kept| *kept != digest)
            .collect::<Vec<_>>()
            .join(" ");
        true
    }

    fn failing(&self, now: Instant) -> Result<bool> {
        match self.failing_since {
            Some(since) => Ok(since.checked_add(FAILURE_WINDOW)? > now),
            None => Ok(false),
        }
    }

    fn failed(&mut self, now: Instant) -> Result<()> {
        if !self.failing(now)? {
            self.failing_since = Some(now);
            self.failures = 0;
        }
        self.failures = self.failures.saturating_add(1);
        Ok(())
    }
}

/// The factor is committed even when its code is refused, so the limit on wrong codes holds
/// across requests and pending sign-ins; `passed` only when the code was right.
pub(crate) struct Proof<T> {
    factor: SecondFactor,
    passed: Result<T>,
}

impl<T: Outcome> Outcome for Proof<T> {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        self.factor.plan(plan)?;
        match &self.passed {
            Ok(passed) => passed.plan(plan),
            Err(_) => Ok(()),
        }
    }

    fn committed(&mut self, stamp: Stamp) {
        self.factor.committed(stamp);
        if let Ok(passed) = &mut self.passed {
            passed.committed(stamp);
        }
    }

    fn reply(self) -> Result<Reply> {
        self.passed.map(|_| Reply::Empty)
    }
}

/// Whether it is on and since when; never its key or recovery codes.
#[cfg(feature = "account-data")]
pub(crate) async fn exported(read: &Read, account: AccountId) -> Result<sofie::serde_json::Value> {
    Ok(of(read, account)
        .await?
        .map_or(sofie::serde_json::Value::Null, |factor| {
            sofie::serde_json::json!({
                "on": factor.is_confirmed(),
                "confirmed_at": factor.confirmed_at,
            })
        }))
}

pub(crate) async fn of(read: &Read, account: AccountId) -> Result<Option<SecondFactor>> {
    SecondFactor::query()
        .where_(SecondFactor::ACCOUNT.eq(account))
        .first(read)
        .await
}

pub(crate) async fn required(read: &Read, account: AccountId) -> Result<Option<SecondFactor>> {
    Ok(of(read, account).await?.filter(SecondFactor::is_confirmed))
}

pub(crate) async fn removing(read: &Read, account: AccountId) -> Result<Vec<SecondFactor>> {
    let mut factors: Vec<_> = of(read, account).await?.into_iter().collect();
    for factor in &mut factors {
        factor.secret = String::new();
        factor.confirmed_at = None;
        factor.recovery = String::new();
        factor.pending_recovery = None;
    }
    Ok(factors)
}
