use sofie::core::encoding::base64::{self, URL_SAFE};
use sofie::http::{Rate, Routes, Sealing, Web};
use sofie::prelude::*;
use sofie::serde_json::json;

use super::webauthn::{self, GET};
use super::{Ceremony, Passkey, challenge, decoded, opened, origin, refused};
use crate::MemberArea;
use crate::membership;
use crate::signing_in::{Door, Entered};

/// Per caller: each try costs the server a signature check.
const TRYING: Rate = Rate::per_minute(10);

pub(crate) fn path<A: MemberArea>() -> String {
    format!("{}/passkey", A::PREFIX)
}

pub(crate) fn routes<A: MemberArea>(routes: &mut Routes<Web>) {
    routes
        .post(&path::<A>(), sign_in::<A>)
        .name_for::<A>("sign_in_with_passkey")
        .throttle(TRYING)
        .then_follow();
}

/// For `navigator.credentials.get` on the sign-in page: any passkey the device holds for the site.
pub(crate) fn options(sealing: &Sealing, clock: &Clock) -> Result<String> {
    let options = json!({
        "challenge": challenge(sealing, GET, None, clock.now())?,
        "userVerification": "required",
        "timeout": 300_000,
    });
    Ok(options.to_string())
}

/// Only a passkey that verified its user signs in: with no password asked, it is both factors.
async fn sign_in<A: MemberArea>(
    ceremony: Ceremony,
    door: Door,
    sealing: Sealing,
) -> Result<Entered<Passkey>> {
    let now = door.clock().now();
    let (origin, rp_id) = origin(door.session())?;
    let client_data = decoded(Some(&ceremony.client_data), "client data")?;
    let signed = webauthn::challenge(&client_data, GET, &origin).map_err(refused)?;
    let challenge = opened(&sealing, &signed, GET, now)?;
    let mut passkey = Passkey::query()
        .where_(Passkey::CREDENTIAL.eq(ceremony.id.clone()))
        .first(door.read())
        .await?
        .ok_or_else(|| refused("it is not a passkey of any account here"))?;
    let authenticator = decoded(ceremony.authenticator_data.as_deref(), "authenticator data")?;
    let signature = decoded(ceremony.signature.as_deref(), "signature")?;
    let data = webauthn::authenticator_data(&authenticator).map_err(refused)?;
    data.check(&rp_id).map_err(refused)?;
    if !data.is_user_verified() {
        return Err(refused("it did not verify its user"));
    }
    let key = base64::decode(&URL_SAFE, &passkey.public_key)
        .ok_or_else(|| Error::internal_message("a stored passkey's key is not base64url"))?;
    if !webauthn::verifies(&key, &authenticator, &client_data, &signature) {
        return Err(refused("its signature does not verify"));
    }
    if challenge.issued <= passkey.challenged_at {
        return Err(refused("its challenge was answered already"));
    }
    let count = i64::from(data.sign_count);
    if (count != 0 || passkey.sign_count != 0) && count <= passkey.sign_count {
        return Err(refused(
            "its signature counter went back, as a copied authenticator's does",
        ));
    }
    passkey.sign_count = count;
    passkey.challenged_at = challenge.issued;
    passkey.last_used_at = Some(now);
    membership::require(door.read(), passkey.account, A::NAME).await?;
    let entered = door.enter_after::<A>(passkey.account, true).await?;
    Ok(entered.located::<A>().saving(passkey))
}
