//! The ID token's signature and claims (OIDC Core 1.0, §3.1.3.7).

use sofie::core::Result;
use sofie::core::encoding::base64::{self, URL_SAFE};
use sofie::serde::Deserialize;
use sofie::serde_json::Value;
use sofie::values::Email;
use sofie_client::Outgoing;

use super::flow::{Identified, refused};
use super::keys::{Algorithm, Keys};
use super::provider::{Provider, TENANT_ID};

/// Clocks here and at the provider may disagree this much.
const LEEWAY_SECONDS: i64 = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Refusal {
    Unreadable,
    Algorithm,
    Signature,
    Issuer,
    Audience,
    Expired,
    IssuedAt,
    Nonce,
}

impl Refusal {
    fn reason(self) -> &'static str {
        match self {
            Self::Unreadable => "unreadable",
            Self::Algorithm => "algorithm not accepted",
            Self::Signature => "signature not by the provider's keys",
            Self::Issuer => "another issuer",
            Self::Audience => "another audience",
            Self::Expired => "expired",
            Self::IssuedAt => "issued at no time or in the future",
            Self::Nonce => "another sign-in's nonce",
        }
    }
}

#[derive(Deserialize)]
#[serde(crate = "sofie::serde")]
struct Header {
    alg: String,
    kid: Option<String>,
    /// Extensions a verifier must understand (RFC 7515, §4.1.11); none is understood here.
    crit: Option<Value>,
}

struct Signed<'t> {
    message: &'t str,
    header: Header,
    claims: Value,
    signature: Vec<u8>,
}

impl<'t> Signed<'t> {
    fn read(token: &'t str) -> Option<Self> {
        let (message, signature) = token.rsplit_once('.')?;
        let (header, claims) = message.split_once('.')?;
        Some(Self {
            message,
            header: json(header)?,
            claims: json(claims)?,
            signature: base64::decode(&URL_SAFE, signature)?,
        })
    }
}

fn json<T: sofie::serde::de::DeserializeOwned>(part: &str) -> Option<T> {
    sofie::serde_json::from_slice(&base64::decode(&URL_SAFE, part)?).ok()
}

pub(crate) struct Expected<'a> {
    pub(crate) issuer: &'a str,
    pub(crate) keys: &'a Keys,
    pub(crate) nonce: &'a str,
}

/// Refusals are logged with their reason, never with the token.
pub(crate) async fn identified(
    http: &Outgoing,
    provider: &Provider,
    expected: &Expected<'_>,
    token: &str,
    now: i64,
) -> Result<Identified> {
    let refusal = |refusal: Refusal| {
        let provider_name = provider.name.clone();
        let reason = refusal.reason().to_owned();
        sofie::telemetry::warn!(provider_name, reason, "an ID token was refused");
        let shown = match refusal {
            Refusal::Unreadable => "sent an ID token that does not read",
            _ => "sent an ID token for another sign-in",
        };
        refused(provider, shown)
    };
    let signed = Signed::read(token).ok_or_else(|| refusal(Refusal::Unreadable))?;
    signature_holds(http, expected, &signed, now)
        .await?
        .map_err(refusal)?;
    claims_hold(&signed.claims, provider, expected, now).map_err(refusal)?;
    who(&signed.claims).ok_or_else(|| refused(provider, "named nobody"))
}

async fn signature_holds(
    http: &Outgoing,
    expected: &Expected<'_>,
    signed: &Signed<'_>,
    now: i64,
) -> Result<Result<(), Refusal>> {
    let header = &signed.header;
    let Some(algorithm) = Algorithm::named(&header.alg).filter(|_| header.crit.is_none()) else {
        return Ok(Err(Refusal::Algorithm));
    };
    let kid = header.kid.as_deref();
    let keys = expected
        .keys
        .current(http, expected.issuer, kid, now)
        .await?;
    let message = signed.message.as_bytes();
    let verified = keys
        .iter()
        .filter(|key| key.named(kid))
        .any(|key| key.verifies(algorithm, message, &signed.signature));
    Ok(if verified {
        Ok(())
    } else {
        Err(Refusal::Signature)
    })
}

fn claims_hold(
    claims: &Value,
    provider: &Provider,
    expected: &Expected<'_>,
    now: i64,
) -> Result<(), Refusal> {
    if !issued_by(claims, expected.issuer) {
        return Err(Refusal::Issuer);
    }
    if !addressed_to(claims, &provider.client_id) {
        return Err(Refusal::Audience);
    }
    if claims["exp"]
        .as_i64()
        .is_none_or(|expiry| expiry + LEEWAY_SECONDS <= now)
    {
        return Err(Refusal::Expired);
    }
    if claims["iat"]
        .as_i64()
        .is_none_or(|issued| issued - LEEWAY_SECONDS > now)
    {
        return Err(Refusal::IssuedAt);
    }
    if claims["nonce"] != expected.nonce {
        return Err(Refusal::Nonce);
    }
    Ok(())
}

fn issued_by(claims: &Value, issuer: &str) -> bool {
    let Some(tenant) = claims["tid"].as_str().filter(|tenant| !tenant.is_empty()) else {
        return claims["iss"] == issuer;
    };
    claims["iss"] == issuer.replace(TENANT_ID, tenant)
}

/// With several audiences, the authorized party must be this client too (§2).
fn addressed_to(claims: &Value, client_id: &str) -> bool {
    match &claims["aud"] {
        Value::String(audience) => audience == client_id,
        Value::Array(audiences) => {
            audiences.iter().any(|audience| audience == client_id)
                && (audiences.len() == 1 || claims["azp"] == client_id)
        }
        _ => false,
    }
}

fn who(claims: &Value) -> Option<Identified> {
    let subject = claims["sub"]
        .as_str()
        .filter(|subject| !subject.is_empty())?;
    Some(Identified {
        subject: subject.to_owned(),
        email: claims["email"]
            .as_str()
            .and_then(|email| Email::parse(email).ok()),
        verified: claims["email_verified"] == true || claims["email_verified"] == "true",
    })
}

#[cfg(test)]
mod tests {
    use sofie::serde_json::json;
    use sofie::values::Secret;

    use super::super::keys::KeysAt;
    use super::*;

    const ISSUED: &str =
        "https://login.microsoftonline.com/0a1b2c3d-0000-4000-8000-000000000001/v2.0";
    const NOW: i64 = 1_000_000_000;

    fn microsoft() -> Provider {
        let client = ("client".to_owned(), Secret::new("shh".to_owned()));
        Provider::microsoft("common", client).unwrap()
    }

    fn checked(claims: &Value) -> Result<(), Refusal> {
        let provider = microsoft();
        let keys = Keys::new(KeysAt::Discovery);
        let expected = Expected {
            issuer: "https://login.microsoftonline.com/{tenantid}/v2.0",
            keys: &keys,
            nonce: "n",
        };
        claims_hold(claims, &provider, &expected, NOW)
    }

    fn claims(iss: &str, tid: &str) -> Value {
        json!({
            "iss": iss, "tid": tid, "aud": "client", "exp": NOW + 600, "iat": NOW,
            "nonce": "n", "sub": "someone",
        })
    }

    #[test]
    fn microsoft_common_takes_the_tenant_its_token_names() {
        let tenant = "0a1b2c3d-0000-4000-8000-000000000001";
        assert_eq!(checked(&claims(ISSUED, tenant)), Ok(()));
        let other_tenant = "ffffffff-0000-4000-8000-000000000001";
        assert_eq!(checked(&claims(ISSUED, other_tenant)), Err(Refusal::Issuer));
        assert_eq!(checked(&claims(ISSUED, "")), Err(Refusal::Issuer));
        let elsewhere = claims("https://evil.example/v2.0", tenant);
        assert_eq!(checked(&elsewhere), Err(Refusal::Issuer));
    }

    #[test]
    fn audience_time_and_nonce_are_this_sign_ins() {
        let tenant = "0a1b2c3d-0000-4000-8000-000000000001";
        let with = |field: &str, value: Value| {
            let mut claims = claims(ISSUED, tenant);
            claims[field] = value;
            checked(&claims)
        };
        assert_eq!(with("aud", json!("other")), Err(Refusal::Audience));
        assert_eq!(
            with("aud", json!(["client", "other"])),
            Err(Refusal::Audience)
        );
        let mut shared = claims(ISSUED, tenant);
        shared["aud"] = json!(["client", "other"]);
        shared["azp"] = json!("client");
        assert_eq!(checked(&shared), Ok(()));
        assert_eq!(with("exp", json!(NOW - 61)), Err(Refusal::Expired));
        assert_eq!(with("exp", json!(NOW - 30)), Ok(()));
        assert_eq!(with("iat", json!(NOW + 61)), Err(Refusal::IssuedAt));
        assert_eq!(with("iat", Value::Null), Err(Refusal::IssuedAt));
        assert_eq!(with("nonce", json!("m")), Err(Refusal::Nonce));
    }
}
