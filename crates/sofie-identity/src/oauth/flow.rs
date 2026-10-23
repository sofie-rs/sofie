//! The authorization code flow with PKCE (RFC 6749, RFC 7636).

use ring::digest::{SHA256, digest};
use sofie::core::encoding::base64::{self, URL_SAFE};
use sofie::core::encoding::percent;
use sofie::core::{Error, Instant, Result};
use sofie::db::Uuid;
use sofie::http::{Area, Sealing, Session};
use sofie::serde::{Deserialize, Serialize};
use sofie::serde_json::Value;
use sofie::values::Email;

use super::OAuth;
use super::id_token::{self, Expected};
use super::provider::{Claims, Provider};
use super::sign_in::chosen;

const STATE: &str = "sofie_identity.oauth_state";
/// As long as the flow cookie lives.
const STATE_VALID_SECONDS: i64 = 600;
const BROKEN: &str = "This sign-in was begun elsewhere or too long ago; begin it again.";

#[derive(Serialize, Deserialize)]
#[serde(crate = "sofie::serde")]
pub(crate) struct State {
    pub(crate) provider: String,
    pub(crate) area: String,
    verifier: String,
    nonce: String,
    flow: String,
    issued: i64,
    /// The account a signed-in member connects the provider to; none when signing in.
    pub(crate) link: Option<Uuid>,
}

pub(crate) struct Identified {
    pub(crate) subject: String,
    pub(crate) email: Option<Email>,
    pub(crate) verified: bool,
}

fn random_text() -> Result<String> {
    let mut bytes = [0; 32];
    sofie::core::random::fill(&mut bytes)
        .map_err(|error| Error::internal("drawing an OAuth flow's secrets", error))?;
    Ok(base64::encode(&URL_SAFE, &bytes))
}

pub(crate) fn callback_path<A: Area>() -> String {
    format!("{}/oauth/callback", A::PREFIX)
}

fn callback<A: Area>(oauth: &OAuth) -> String {
    format!("{}{}", oauth.base_url, callback_path::<A>())
}

pub(crate) fn begin<A: Area>(
    oauth: &OAuth,
    provider: &str,
    session: &Session,
    sealing: &Sealing,
    now: Instant,
    link: Option<Uuid>,
) -> Result<String> {
    let provider = chosen(oauth, provider)?;
    let state = State {
        provider: provider.name.clone(),
        area: A::NAME.to_owned(),
        verifier: random_text()?,
        nonce: random_text()?,
        flow: session.begin_flow()?,
        issued: now.timestamp().as_second(),
        link,
    };
    let text = sofie::serde_json::to_vec(&state)
        .map_err(|error| Error::internal("writing an OAuth flow's state", error))?;
    let sealed = sealing.seal(STATE, &text)?;
    let challenge = base64::encode(
        &URL_SAFE,
        digest(&SHA256, state.verifier.as_bytes()).as_ref(),
    );
    let redirect_uri = callback::<A>(oauth);
    let query = percent::form([
        ("response_type", "code"),
        ("client_id", provider.client_id.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
        ("scope", provider.scopes.as_str()),
        ("state", sealed.as_str()),
        ("nonce", state.nonce.as_str()),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
    ]);
    Ok(format!("{}?{query}", provider.authorization_url))
}

/// The state, once it proves to be sealed here, recently, for this browser and area.
pub(crate) fn opened(
    sealing: &Sealing,
    text: &str,
    session: &Session,
    area: &str,
    now: Instant,
) -> Result<State> {
    let broken = || Error::Rule(BROKEN.to_owned());
    let state: State = sealing
        .open(STATE, text)
        .and_then(|bytes| sofie::serde_json::from_slice(&bytes).ok())
        .ok_or_else(broken)?;
    let fresh = state.issued + STATE_VALID_SECONDS > now.timestamp().as_second();
    if !fresh || state.area != area || session.flow() != Some(state.flow.as_str()) {
        return Err(broken());
    }
    Ok(state)
}

pub(crate) async fn identified<A: Area>(
    oauth: &OAuth,
    provider: &Provider,
    state: &State,
    code: &str,
    now: Instant,
) -> Result<Identified> {
    let redirect = callback::<A>(oauth);
    let answer = oauth
        .http
        .post_url(&provider.token_url)
        .header("accept", "application/json")
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", &redirect),
            ("client_id", &provider.client_id),
            ("client_secret", provider.client_secret.expose()),
            ("code_verifier", &state.verifier),
        ])
        .send()
        .await?;
    // A code used twice or too late is the provider's 400, not this side's failure.
    if answer.status() != 200 {
        return Err(refused(provider, "refused the sign-in"));
    }
    let tokens: Value = answer.json()?;
    match &provider.claims {
        Claims::IdToken { issuer, keys } => {
            let token = tokens["id_token"]
                .as_str()
                .ok_or_else(|| refused(provider, "sent no ID token"))?;
            let expected = Expected {
                issuer,
                keys,
                nonce: &state.nonce,
            };
            let now = now.timestamp().as_second();
            id_token::identified(&oauth.http, provider, &expected, token, now).await
        }
        Claims::UserEndpoints { user, emails } => {
            let access = tokens["access_token"]
                .as_str()
                .ok_or_else(|| refused(provider, "sent no access token"))?;
            user_endpoints(oauth, provider, access, (user, emails)).await
        }
    }
}

pub(super) fn refused(provider: &Provider, what: &str) -> Error {
    Error::Rule(format!(
        "{} {what}; try again or sign in another way.",
        provider.label
    ))
}

async fn user_endpoints(
    oauth: &OAuth,
    provider: &Provider,
    access: &str,
    (user, emails): (&str, &str),
) -> Result<Identified> {
    let asked = |url: &str| {
        oauth
            .http
            .get_url(url)
            .bearer(access)
            .header("accept", "application/json")
    };
    let account: Value = asked(user).send().await?.json()?;
    let subject = match &account["id"] {
        Value::Number(id) => id.to_string(),
        Value::String(id) if !id.is_empty() => id.clone(),
        _ => return Err(refused(provider, "named nobody")),
    };
    let listed: Value = asked(emails).send().await?.json()?;
    let primary = listed
        .as_array()
        .into_iter()
        .flatten()
        .find(|email| email["primary"] == true && email["verified"] == true);
    Ok(Identified {
        subject,
        email: primary
            .and_then(|email| email["email"].as_str())
            .and_then(|email| Email::parse(email).ok()),
        verified: primary.is_some(),
    })
}
