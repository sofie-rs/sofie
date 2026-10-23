use sofie::core::Instant;
use sofie::core::encoding::base64::{self, URL_SAFE};
use sofie::db::Outcome;
use sofie::http::{AppName, Area, Routes, Sealing, Shipped};
use sofie::prelude::*;
use sofie::serde_json::json;

use super::webauthn::{self, ALGORITHMS, CREATE};
use super::{CREDENTIAL_INDEX, Ceremony, Passkey, challenge, decoded, opened, origin, refused};
use crate::account::Account;
use crate::account_of;
use crate::signing_in::{OwnSession, RecentSignIn};

const ALREADY_ADDED: &str = "is added already";

/// Each principal adds and removes only its own account's passkeys.
pub fn routes<A: Area>(routes: &mut Routes<A>) {
    routes
        .shipped(
            "/passkeys",
            "passkeys",
            (include_str!("screen.html"), None),
            list::<A>,
        )
        .name("list_passkeys");
    routes
        .post("/passkeys", register::<A>)
        .name("register_passkey")
        .then_screen("passkeys", "passkeys");
    routes
        .post("/passkeys/{passkey}/remove", remove::<A>)
        .name("remove_passkey")
        .then_screen("passkeys", "passkeys");
}

#[derive(ReadModel)]
#[read(from = Passkey)]
struct Listed {
    id: Id<Passkey>,
    credential: String,
    created_at: Instant,
    last_used_at: Option<Instant>,
}

#[derive(ReadModel)]
pub struct Passkeys {
    passkeys: Vec<Listed>,
    /// For `navigator.credentials.create`, read by the page's script or a client area's runtime.
    options: String,
}

async fn list<A: Area>(
    principal: Principal<A>,
    read: Read,
    sealing: Sealing,
    clock: Clock,
    app: AppName,
) -> Result<Passkeys> {
    let account_id = account_of(&principal);
    let account = Account::stored(&read, account_id).await?;
    let passkeys = Listed::query()
        .where_(Passkey::ACCOUNT.eq(account_id))
        .latest()
        .all(&read)
        .await?;
    let options = json!({
        "challenge": challenge(&sealing, CREATE, Some(account_id), clock.now())?,
        "rp": { "name": app.as_str() },
        "user": {
            "id": base64::encode(&URL_SAFE, account_id.uuid().as_bytes()),
            "name": account.email().as_str(),
            "displayName": account.email().as_str(),
        },
        "pubKeyCredParams": ALGORITHMS
            .iter()
            .map(|algorithm| json!({ "type": "public-key", "alg": algorithm }))
            .collect::<Vec<_>>(),
        "excludeCredentials": passkeys
            .iter()
            .map(|passkey| json!({ "type": "public-key", "id": passkey.credential }))
            .collect::<Vec<_>>(),
        "authenticatorSelection": { "residentKey": "required", "userVerification": "preferred" },
        "attestation": "none",
        "timeout": 300_000,
    });
    Ok(Passkeys {
        passkeys,
        options: options.to_string(),
    })
}

/// Only for the account the challenge was issued to, from the origin the browser posted from.
async fn register<A: Area>(
    principal: Principal<A>,
    _recent: RecentSignIn,
    ceremony: Ceremony,
    session: Session,
    sealing: Sealing,
    clock: Clock,
) -> Result<RegisteredPasskey> {
    let account = account_of(&principal);
    let now = clock.now();
    let (origin, rp_id) = origin(&session)?;
    let client_data = decoded(Some(&ceremony.client_data), "client data")?;
    let signed = webauthn::challenge(&client_data, CREATE, &origin).map_err(refused)?;
    let challenge = opened(&sealing, &signed, CREATE, now)?;
    if challenge.account != Some(account.uuid()) {
        return Err(refused("its challenge was issued to another account"));
    }
    let attestation = decoded(ceremony.attestation_object.as_deref(), "attestation")?;
    let data = webauthn::authenticator_data(webauthn::attested(&attestation).map_err(refused)?)
        .map_err(refused)?;
    data.check(&rp_id).map_err(refused)?;
    let (credential, key) = data
        .credential
        .ok_or_else(|| refused("it names no credential"))?;
    let algorithm = webauthn::algorithm(key).map_err(refused)?;
    Ok(RegisteredPasskey(Passkey {
        meta: Meta::new()?,
        account,
        credential: base64::encode(&URL_SAFE, credential),
        public_key: base64::encode(&URL_SAFE, key),
        algorithm,
        sign_count: i64::from(data.sign_count),
        // A sign-in page opened before this registration still signs in with it.
        challenged_at: 0,
        last_used_at: None,
    }))
}

/// One credential belongs to one account.
pub struct RegisteredPasskey(Passkey);

impl Outcome for RegisteredPasskey {
    fn plan(&self, plan: &mut sofie::db::Plan<'_>) -> Result<()> {
        plan.unique(CREDENTIAL_INDEX, "id", ALREADY_ADDED);
        self.0.plan(plan)
    }

    fn committed(&mut self, stamp: sofie::db::Stamp) {
        self.0.committed(stamp);
    }
}

/// Checked here too, since a `Permitted` area may reach records it does not own.
async fn remove<A: Area>(
    principal: Principal<A>,
    _own: OwnSession,
    mut passkey: Passkey,
) -> Result<Passkey> {
    if passkey.account != account_of(&principal) {
        return Err(Error::NotFound);
    }
    passkey.delete();
    Ok(passkey)
}
