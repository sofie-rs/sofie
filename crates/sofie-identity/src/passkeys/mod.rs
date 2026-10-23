mod cbor;
mod screen;
mod sign_in;
mod webauthn;

use std::time::Duration;

use sofie::core::Instant;
use sofie::core::encoding::base64::{self, URL_SAFE};
use sofie::db::Uuid;
use sofie::http::Sealing;
use sofie::prelude::*;

pub use screen::routes;
pub(crate) use sign_in::{
    options as sign_in_options, path as sign_in_path, routes as sign_in_routes,
};

use crate::AccountId;

const CHALLENGE: &str = "sofie_identity.passkey_challenge";
/// Long enough to find the authenticator, short enough that a stale page signs nothing.
const CHALLENGE_VALID_FOR: Duration = Duration::from_mins(5);
const CREDENTIAL_INDEX: &str = "passkey_credential";

/// Trusted for the account that registered it; attestation is not asked.
#[derive(Aggregate)]
#[aggregate(behaviors(SoftDelete))]
pub struct Passkey {
    meta: Meta,
    #[owner]
    account: AccountId,
    credential: String,
    public_key: String,
    algorithm: i32,
    sign_count: i64,
    /// A ceremony challenged earlier than this one is refused, so none is replayed.
    challenged_at: i64,
    last_used_at: Option<Instant>,
}

/// What the browser answered, each part base64url as `engine.js` sends it; a credential id
/// is at most 1023 bytes and a ceremony a few kilobytes.
#[derive(Input)]
pub struct Ceremony {
    #[max(16384)]
    id: String,
    #[max(16384)]
    client_data: String,
    #[max(16384)]
    attestation_object: Option<String>,
    #[max(16384)]
    authenticator_data: Option<String>,
    #[max(16384)]
    signature: Option<String>,
}

struct Challenge {
    kind: String,
    account: Option<Uuid>,
    issued: i64,
}

impl Challenge {
    fn read(value: &sofie::serde_json::Value) -> Option<Self> {
        let account = match &value["account"] {
            sofie::serde_json::Value::Null => None,
            account => Some(account.as_str()?.parse().ok()?),
        };
        Some(Self {
            kind: value["kind"].as_str()?.to_owned(),
            account,
            issued: value["issued"].as_i64()?,
        })
    }
}

fn refused(reason: &str) -> Error {
    Error::Rule(format!("The passkey was not accepted: {reason}."))
}

fn decoded(part: Option<&str>, name: &str) -> Result<Vec<u8>> {
    part.and_then(|text| base64::decode(&URL_SAFE, text))
        .ok_or_else(|| refused(&format!("its {name} is missing or not base64url")))
}

/// Sealed, so the browser carries it and nothing is stored until the ceremony ends.
fn challenge(
    sealing: &Sealing,
    kind: &str,
    account: Option<AccountId>,
    now: Instant,
) -> Result<String> {
    let text = sofie::serde_json::json!({
        "kind": kind,
        "account": account.map(|account| account.uuid().to_string()),
        "issued": now.timestamp().as_microsecond(),
    });
    let sealed = sealing.seal(CHALLENGE, text.to_string().as_bytes())?;
    Ok(base64::encode(&URL_SAFE, sealed.as_bytes()))
}

/// The challenge the browser signed, once it proves to be one issued here, recently, for `kind`.
fn opened(sealing: &Sealing, signed: &[u8], kind: &str, now: Instant) -> Result<Challenge> {
    let challenge = std::str::from_utf8(signed)
        .ok()
        .and_then(|sealed| sealing.open(CHALLENGE, sealed))
        .and_then(|text| sofie::serde_json::from_slice(&text).ok())
        .and_then(|value: sofie::serde_json::Value| Challenge::read(&value))
        .filter(|challenge| challenge.kind == kind)
        .ok_or_else(|| refused("its challenge was not issued here"))?;
    let expires = challenge
        .issued
        .saturating_add(i64::try_from(CHALLENGE_VALID_FOR.as_micros()).unwrap_or(i64::MAX));
    if expires <= now.timestamp().as_microsecond() {
        return Err(refused("its challenge expired; reload the page"));
    }
    Ok(challenge)
}

fn origin(session: &Session) -> Result<(String, String)> {
    let origin = session
        .origin()
        .ok_or_else(|| refused("the request names no origin"))?
        .to_owned();
    let host = origin
        .split_once("://")
        .map(|(_, rest)| rest)
        .and_then(|rest| rest.split(':').next())
        .filter(|host| !host.is_empty())
        .ok_or_else(|| refused("the request's origin names no host"))?
        .to_owned();
    Ok((origin, host))
}

#[cfg(feature = "account-data")]
pub(crate) async fn exported(read: &Read, account: AccountId) -> Result<sofie::serde_json::Value> {
    let passkeys = Passkey::query()
        .where_(Passkey::ACCOUNT.eq(account))
        .all(read)
        .await?;
    Ok(sofie::serde_json::json!(
        passkeys
            .iter()
            .map(|passkey| sofie::serde_json::json!({
                "credential": passkey.credential,
                "last_used_at": passkey.last_used_at,
            }))
            .collect::<Vec<_>>()
    ))
}

pub(crate) async fn removing(read: &Read, account: AccountId) -> Result<Vec<Passkey>> {
    let mut passkeys = Passkey::query()
        .where_(Passkey::ACCOUNT.eq(account))
        .all(read)
        .await?;
    for passkey in &mut passkeys {
        passkey.public_key = String::new();
        passkey.delete();
    }
    Ok(passkeys)
}
