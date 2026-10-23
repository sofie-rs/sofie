//! The provider's signing keys (RFC 7517), found through its discovery document
//! (OIDC Discovery 1.0, §4) unless the provider's key set is known.

use std::sync::{Arc, Mutex};

use ring::signature::{
    ECDSA_P256_SHA256_FIXED, RSA_PKCS1_2048_8192_SHA256, RSA_PKCS1_2048_8192_SHA384,
    RSA_PKCS1_2048_8192_SHA512, RsaPublicKeyComponents, UnparsedPublicKey,
};
use sofie::core::encoding::base64::{self, URL_SAFE};
use sofie::core::unpoisoned;
use sofie::core::{Error, Result};
use sofie::serde::Deserialize;
use sofie_client::{Outgoing, Request};

/// Keys rotate rarely and a new one appears in the set before it signs anything.
const KEPT_SECONDS: i64 = 3600;
/// An unknown `kid` fetches the set again at most this often, so tokens naming made-up keys
/// cannot make every sign-in ask the provider.
const REFETCH_AFTER_SECONDS: i64 = 60;
const MAX_KEYS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Algorithm {
    Rs256,
    Rs384,
    Rs512,
    Es256,
}

impl Algorithm {
    /// `none` and the HMAC algorithms are refused: a provider's token is never signed
    /// with a secret this side holds too.
    pub(crate) fn named(name: &str) -> Option<Self> {
        match name {
            "RS256" => Some(Self::Rs256),
            "RS384" => Some(Self::Rs384),
            "RS512" => Some(Self::Rs512),
            "ES256" => Some(Self::Es256),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Rs256 => "RS256",
            Self::Rs384 => "RS384",
            Self::Rs512 => "RS512",
            Self::Es256 => "ES256",
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(crate = "sofie::serde")]
pub(crate) struct Jwk {
    kty: String,
    kid: Option<String>,
    #[serde(rename = "use")]
    usage: Option<String>,
    alg: Option<String>,
    n: Option<String>,
    e: Option<String>,
    crv: Option<String>,
    x: Option<String>,
    y: Option<String>,
}

impl Jwk {
    pub(crate) fn named(&self, kid: Option<&str>) -> bool {
        kid.is_none_or(|kid| self.kid.as_deref() == Some(kid))
    }

    fn signs_with(&self, algorithm: Algorithm) -> bool {
        let usage_holds = self.usage.as_deref().is_none_or(|usage| usage == "sig");
        let alg_holds = self
            .alg
            .as_deref()
            .is_none_or(|alg| alg == algorithm.name());
        usage_holds && alg_holds
    }

    pub(crate) fn verifies(&self, algorithm: Algorithm, message: &[u8], signature: &[u8]) -> bool {
        if !self.signs_with(algorithm) {
            return false;
        }
        match algorithm {
            Algorithm::Es256 => self.verifies_p256(message, signature),
            Algorithm::Rs256 | Algorithm::Rs384 | Algorithm::Rs512 => {
                self.verifies_rsa(algorithm, message, signature)
            }
        }
    }

    fn verifies_rsa(&self, algorithm: Algorithm, message: &[u8], signature: &[u8]) -> bool {
        if self.kty != "RSA" {
            return false;
        }
        let (Some(n), Some(e)) = (decoded(self.n.as_deref()), decoded(self.e.as_deref())) else {
            return false;
        };
        let parameters = match algorithm {
            Algorithm::Rs384 => &RSA_PKCS1_2048_8192_SHA384,
            Algorithm::Rs512 => &RSA_PKCS1_2048_8192_SHA512,
            Algorithm::Rs256 | Algorithm::Es256 => &RSA_PKCS1_2048_8192_SHA256,
        };
        RsaPublicKeyComponents { n, e }
            .verify(parameters, message, signature)
            .is_ok()
    }

    fn verifies_p256(&self, message: &[u8], signature: &[u8]) -> bool {
        if self.kty != "EC" || self.crv.as_deref() != Some("P-256") {
            return false;
        }
        let (Some(x), Some(y)) = (decoded(self.x.as_deref()), decoded(self.y.as_deref())) else {
            return false;
        };
        if x.len() != 32 || y.len() != 32 {
            return false;
        }
        let point = [&[4][..], &x, &y].concat();
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, point)
            .verify(message, signature)
            .is_ok()
    }
}

fn decoded(text: Option<&str>) -> Option<Vec<u8>> {
    base64::decode(&URL_SAFE, text?)
}

#[derive(Debug)]
pub(crate) enum KeysAt {
    Url(String),
    Discovery,
}

#[derive(Debug)]
struct Fetched {
    keys: Arc<[Jwk]>,
    at: i64,
}

/// One provider's keys, kept for `KEPT_SECONDS` and fetched again once on an unknown `kid`.
#[derive(Debug)]
pub(crate) struct Keys {
    at: KeysAt,
    fetched: Mutex<Option<Fetched>>,
}

#[derive(Deserialize)]
#[serde(crate = "sofie::serde")]
struct Discovery {
    issuer: String,
    jwks_uri: String,
}

#[derive(Deserialize)]
#[serde(crate = "sofie::serde")]
struct KeySet {
    keys: Vec<Jwk>,
}

impl Keys {
    pub(crate) fn new(at: KeysAt) -> Self {
        Self {
            at,
            fetched: Mutex::new(None),
        }
    }

    pub(crate) fn url(&self) -> Option<&str> {
        match &self.at {
            KeysAt::Url(url) => Some(url),
            KeysAt::Discovery => None,
        }
    }

    pub(crate) async fn current(
        &self,
        http: &Outgoing,
        issuer: &str,
        kid: Option<&str>,
        now: i64,
    ) -> Result<Arc<[Jwk]>> {
        let (keys, age) = match self.kept(now) {
            Some(kept) => kept,
            None => (self.fetch(http, issuer, now).await?, 0),
        };
        let known = keys.iter().any(|key| key.named(kid));
        if known || age < REFETCH_AFTER_SECONDS {
            return Ok(keys);
        }
        self.fetch(http, issuer, now).await
    }

    fn kept(&self, now: i64) -> Option<(Arc<[Jwk]>, i64)> {
        let fetched = unpoisoned::lock(&self.fetched);
        let fetched = fetched.as_ref()?;
        let age = now - fetched.at;
        (age < KEPT_SECONDS).then(|| (Arc::clone(&fetched.keys), age))
    }

    async fn fetch(&self, http: &Outgoing, issuer: &str, now: i64) -> Result<Arc<[Jwk]>> {
        let (url, set) = match &self.at {
            KeysAt::Url(url) => (url.clone(), key_set(http.get_url(url)).await?),
            KeysAt::Discovery => {
                let url = discovered_keys_url(http, issuer).await?;
                let set = discovered_key_set(http, issuer, &url).await?;
                (url, set)
            }
        };
        if set.keys.len() > MAX_KEYS {
            return Err(Error::internal_message(format!(
                "the key set at {url} lists {} keys, more than {MAX_KEYS}",
                set.keys.len()
            )));
        }
        let keys: Arc<[Jwk]> = set.keys.into();
        *unpoisoned::lock(&self.fetched) = Some(Fetched {
            keys: Arc::clone(&keys),
            at: now,
        });
        Ok(keys)
    }
}

async fn key_set(request: Request<'_>) -> Result<KeySet> {
    request
        .header("accept", "application/json")
        .send()
        .await?
        .json()
}

/// The issuer's own discovery document names it, but no setting allows its host, as Firebase's
/// keys sit apart from its issuer: HTTPS to a public address only, without redirects.
async fn discovered_key_set(http: &Outgoing, issuer: &str, url: &str) -> Result<KeySet> {
    if on_issuer_host(issuer, url) {
        return key_set(http.get_url(url)).await;
    }
    let untrusted = http.untrusted();
    key_set(untrusted.get(url))
        .await
        .map_err(|error| match error {
            Error::BadRequest(refused) => Error::internal_message(refused),
            other => other,
        })
}

fn on_issuer_host(issuer: &str, url: &str) -> bool {
    super::host_of(url).is_some_and(|host| super::host_of(issuer) == Some(host))
}

async fn discovered_keys_url(http: &Outgoing, issuer: &str) -> Result<String> {
    let url = format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    );
    let discovery: Discovery = http
        .get_url(&url)
        .header("accept", "application/json")
        .send()
        .await?
        .json()?;
    if discovery.issuer != issuer {
        return Err(Error::internal_message(format!(
            "{url} names the issuer `{}`, not `{issuer}`",
            discovery.issuer
        )));
    }
    Ok(discovery.jwks_uri)
}

#[cfg(test)]
mod tests {
    use super::on_issuer_host;

    #[test]
    fn keys_discovered_on_another_host_than_the_issuer_s_are_reached_apart() {
        assert!(on_issuer_host(
            "https://accounts.example",
            "https://accounts.example/keys"
        ));
        assert!(!on_issuer_host(
            "https://securetoken.google.com/project",
            "https://www.googleapis.com/service_accounts/v1/jwk/securetoken@system.gserviceaccount.com"
        ));
    }
}
