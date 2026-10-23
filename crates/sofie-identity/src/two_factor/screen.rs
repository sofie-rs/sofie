use sofie::core::Instant;
use sofie::db::{Outcome, Plan, Stamp};
use sofie::http::{AppName, Area, Routes, Sealing, Shipped};
use sofie::prelude::*;

use super::qr::QrCode;
use super::{Proof, RECOVERY, SECRET, SecondFactor, code, of};
use crate::account::Account;
use crate::signing_in::{OwnSession, RecentSignIn, SignedInRecently};
use crate::{AccountId, account_of};

const ALREADY_ON: &str = "Two-step sign-in is on already; turn it off first to set up another app.";
const NOTHING_TO_CONFIRM: &str = "Set up two-step sign-in first.";
const NOT_ON: &str = "Two-step sign-in is not on.";
const FACTOR_INDEX: &str = "second_factor_account";
const BEING_SET_UP: &str = "is being set up in another window; reload the page";

#[derive(Input)]
struct GivenCode {
    code: String,
}

/// Each principal sets up only its own account's second factor.
pub fn routes<A: Area>(routes: &mut Routes<A>) {
    let describe: sofie::http::Describe = |screens, key| {
        screens.describe_form::<GivenCode>(key, "confirm-two-factor")?;
        screens.describe_form::<GivenCode>(key, "disable-two-factor")
    };
    routes
        .shipped(
            "/two-factor",
            "two_factor",
            (include_str!("screen.html"), Some(describe)),
            status::<A>,
        )
        .name("two_factor_page");
    routes
        .post("/two-factor/start", start::<A>)
        .name("start_two_factor")
        .then_screen("two_factor", "two-factor");
    routes
        .post("/two-factor/confirm", confirm::<A>)
        .name("confirm_two_factor")
        .then_screen("two_factor", "two-factor");
    routes
        .post("/two-factor/disable", disable::<A>)
        .name("disable_two_factor")
        .then_screen("two_factor", "two-factor");
}

#[derive(ReadModel)]
pub struct Status {
    enabled: bool,
    off: bool,
    pending: Option<Pending>,
}

#[derive(ReadModel)]
struct Pending {
    key: String,
    uri: String,
    qr: Qr,
    recovery: Vec<String>,
}

#[derive(ReadModel)]
struct Qr {
    pixels: u64,
    path: String,
}

impl Qr {
    fn of(uri: &str) -> Result<Self> {
        let code = QrCode::encode(uri.as_bytes())?;
        Ok(Self {
            pixels: u64::try_from(code.pixels())
                .map_err(|error| Error::internal("sizing a QR code", error))?,
            path: code.svg_path(),
        })
    }
}

/// Refused in an impersonation too, as a factor being set up shows its key and recovery codes,
/// which only a recent sign-in sees.
async fn status<A: Area>(
    principal: Principal<A>,
    recent: SignedInRecently,
    read: Read,
    sealing: Sealing,
    app: AppName,
) -> Result<Status> {
    let account = account_of(&principal);
    let factor = of(&read, account).await?;
    let enabled = factor.as_ref().is_some_and(SecondFactor::is_confirmed);
    let pending = match factor
        .as_ref()
        .filter(|factor| recent.0 && !factor.is_confirmed())
    {
        Some(
            factor @ SecondFactor {
                pending_recovery: Some(sealed),
                ..
            },
        ) => Some(shown(&read, account, factor, sealed, &sealing, app).await?),
        _ => None,
    };
    Ok(Status {
        enabled,
        off: !enabled && pending.is_none(),
        pending,
    })
}

async fn shown(
    read: &Read,
    account: AccountId,
    factor: &SecondFactor,
    sealed_recovery: &str,
    sealing: &Sealing,
    app: AppName,
) -> Result<Pending> {
    let account = Account::stored(read, account).await?;
    let key = factor.key(sealing)?;
    let recovery = sealing
        .open(RECOVERY, sealed_recovery)
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or_else(|| {
            Error::internal_message("pending recovery codes do not open with the app's keys")
        })?;
    let uri = code::otpauth_uri(app.as_str(), account.email().as_str(), &key);
    Ok(Pending {
        key: code::base32(&key),
        qr: Qr::of(&uri)?,
        uri,
        recovery: recovery.split_whitespace().map(str::to_owned).collect(),
    })
}

async fn start<A: Area>(
    principal: Principal<A>,
    _recent: RecentSignIn,
    read: Read,
    sealing: Sealing,
) -> Result<Started> {
    let account = account_of(&principal);
    let existing = of(&read, account).await?;
    if existing.as_ref().is_some_and(SecondFactor::is_confirmed) {
        return Err(Error::Rule(ALREADY_ON.to_owned()));
    }
    let secret = sealing.seal(SECRET, &code::new_secret()?)?;
    let codes = code::recovery_codes()?;
    let recovery = codes
        .iter()
        .map(|code| code::recovery_digest(code))
        .collect::<Vec<_>>()
        .join(" ");
    let pending_recovery = Some(sealing.seal(RECOVERY, codes.join(" ").as_bytes())?);
    Ok(Started(match existing {
        Some(mut factor) => {
            factor.secret = secret;
            factor.recovery = recovery;
            factor.pending_recovery = pending_recovery;
            factor.last_step = i64::MIN;
            factor
        }
        None => SecondFactor {
            meta: Meta::new()?,
            account,
            secret,
            confirmed_at: None,
            last_step: i64::MIN,
            recovery,
            pending_recovery,
            failures: 0,
            failing_since: None,
        },
    }))
}

/// Two setups begun at once both insert; the later one is refused, not failed.
struct Started(SecondFactor);

impl Outcome for Started {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        plan.unique(FACTOR_INDEX, "code", BEING_SET_UP);
        self.0.plan(plan)
    }

    fn committed(&mut self, stamp: Stamp) {
        self.0.committed(stamp);
    }
}

fn checked(
    mut factor: SecondFactor,
    given: &GivenCode,
    sealing: &Sealing,
    now: Instant,
    then: impl FnOnce(&mut SecondFactor),
) -> Result<Proof<()>> {
    let passed = factor.prove(&given.code, sealing, now)?;
    if passed.is_ok() {
        then(&mut factor);
    }
    Ok(Proof { factor, passed })
}

async fn confirm<A: Area>(
    principal: Principal<A>,
    _recent: RecentSignIn,
    given: GivenCode,
    read: Read,
    sealing: Sealing,
    clock: Clock,
) -> Result<Proof<()>> {
    let factor = of(&read, account_of(&principal))
        .await?
        .filter(|factor| factor.pending_recovery.is_some() && !factor.is_confirmed())
        .ok_or_else(|| Error::Rule(NOTHING_TO_CONFIRM.to_owned()))?;
    let now = clock.now();
    checked(factor, &given, &sealing, now, |factor| {
        factor.confirmed_at = Some(now);
        factor.pending_recovery = None;
    })
}

/// Asks a code too, so a session left open elsewhere cannot turn the second step off.
async fn disable<A: Area>(
    principal: Principal<A>,
    _own: OwnSession,
    given: GivenCode,
    read: Read,
    sealing: Sealing,
    clock: Clock,
) -> Result<Proof<()>> {
    let factor = of(&read, account_of(&principal))
        .await?
        .filter(SecondFactor::is_confirmed)
        .ok_or_else(|| Error::Rule(NOT_ON.to_owned()))?;
    checked(factor, &given, &sealing, clock.now(), |factor| {
        factor.confirmed_at = None;
        factor.recovery = String::new();
    })
}
