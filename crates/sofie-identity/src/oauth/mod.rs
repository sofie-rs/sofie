mod connections;
mod flow;
mod id_token;
mod keys;
mod provider;
mod sign_in;

use std::sync::Arc;

use sofie::db::{Context, Db, Resolve};
use sofie::prelude::*;
use sofie::values::Secret;
use sofie_client::{Outgoing, Vendor};

pub use connections::routes;
pub use provider::Provider;
pub(crate) use sign_in::{offered, routes as sign_in_routes};

use crate::AccountId;
use crate::pages::Choice;

const IDENTITY_INDEX: &str = "account_identity_subject";

pub struct OAuth {
    base_url: String,
    http: Outgoing,
    providers: Vec<Provider>,
}

impl OAuth {
    /// `http` must allow every host of the providers' token, user and key endpoints, and of
    /// each OIDC issuer whose keys are discovered; keys it discovers on another host are
    /// fetched from a public HTTPS address only.
    pub fn new(base_url: &str, http: Outgoing, providers: Vec<Provider>) -> Result<Self> {
        if !(base_url.starts_with("https://") || base_url.starts_with("http://"))
            || base_url.ends_with('/')
        {
            return Err(Error::internal_message(format!(
                "the OAuth base URL is the app's public URL without a trailing `/`, not `{base_url}`"
            )));
        }
        Ok(Self {
            base_url: base_url.to_owned(),
            http,
            providers,
        })
    }

    /// `SOFIE_OAUTH_PROVIDERS` names them (`google`, `microsoft`, `github`, or another
    /// OIDC provider); none set, none offered. Each reads
    /// `SOFIE_OAUTH_<NAME>_CLIENT_ID` and `_CLIENT_SECRET`, Microsoft `_TENANT`, and another
    /// provider `_LABEL`, `_ISSUER`, `_AUTHORIZATION_URL` and `_TOKEN_URL`;
    /// `SOFIE_OAUTH_BASE_URL`, else `SOFIE_BASE_URL`, is the app's public URL.
    pub fn from_environment() -> Result<Option<Self>> {
        let Some(names) = variable("PROVIDERS")? else {
            return Ok(None);
        };
        let providers = names
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(provider_from_environment)
            .collect::<Result<Vec<_>>>()?;
        let hosts = providers
            .iter()
            .flat_map(Provider::requested_urls)
            .filter_map(host_of)
            .collect::<Vec<_>>()
            .join(",");
        let settings = Vendor {
            hosts: Some(hosts),
            ..Vendor::default()
        };
        let http = sofie_client::for_vendor("oauth", settings)?;
        let base_url = base_url(sofie::core::environment_variable)?;
        Self::new(&base_url, http, providers).map(Some)
    }

    fn provider(&self, name: &str) -> Option<&Provider> {
        self.providers.iter().find(|provider| provider.name == name)
    }

    fn choices(&self) -> Vec<Choice> {
        let choice = |provider: &Provider| Choice {
            name: provider.name.clone(),
            label: provider.label.clone(),
        };
        self.providers.iter().map(choice).collect()
    }
}

fn variable(name: &str) -> Result<Option<String>> {
    sofie::core::environment_variable(&format!("SOFIE_OAUTH_{name}"))
}

fn required(name: &str) -> Result<String> {
    variable(name)?.ok_or_else(|| Error::internal_message(format!("SOFIE_OAUTH_{name} is not set")))
}

fn base_url(lookup: impl Fn(&str) -> Result<Option<String>>) -> Result<String> {
    const OWN: &str = "SOFIE_OAUTH_BASE_URL";
    let variable = sofie::core::base_url(OWN, lookup)?
        .ok_or_else(|| sofie::core::base_url_missing(OWN, "OAuth sign-in returns to the app"))?;
    Ok(variable.value)
}

fn provider_from_environment(name: &str) -> Result<Provider> {
    let key = |field: &str| format!("{}_{field}", name.to_ascii_uppercase().replace('-', "_"));
    let client = (
        required(&key("CLIENT_ID"))?,
        Secret::new(required(&key("CLIENT_SECRET"))?),
    );
    match name {
        "google" => Provider::google(client),
        "github" => Provider::github(client),
        "microsoft" => Provider::microsoft(&required(&key("TENANT"))?, client),
        other => Provider::openid(
            other,
            &required(&key("LABEL"))?,
            &required(&key("ISSUER"))?,
            &required(&key("AUTHORIZATION_URL"))?,
            &required(&key("TOKEN_URL"))?,
            client,
        ),
    }
}

fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?']).next()?;
    Some(authority.split(':').next()?.to_ascii_lowercase())
}

/// Handlers then sign in with its providers; without it the sign-in page offers none.
pub fn install(db: &Db, oauth: OAuth) {
    sofie::db::provide(db, Arc::new(oauth));
}

pub(crate) struct Installed(pub(crate) Option<Arc<OAuth>>);

impl<C: Context> Resolve<C> for Installed {
    fn resolve(context: &mut C) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(Ok(Self(sofie::db::provided::<OAuth>(context.db()))))
    }
}

impl Installed {
    fn get(&self) -> Result<&OAuth> {
        self.0.as_deref().ok_or_else(|| {
            Error::internal_message(
                "OAuth is not set up: `sofie_identity::oauth::install` it at boot",
            )
        })
    }
}

#[derive(Aggregate)]
#[aggregate(behaviors(SoftDelete))]
pub struct AccountIdentity {
    meta: Meta,
    #[owner]
    account: AccountId,
    provider: String,
    /// The provider's own id of the person, which never changes, unlike an address.
    subject: String,
    email: Option<Email>,
}

impl AccountIdentity {
    #[must_use]
    pub fn provider(&self) -> &str {
        &self.provider
    }

    #[must_use]
    pub fn email(&self) -> Option<&Email> {
        self.email.as_ref()
    }
}

#[cfg(feature = "account-data")]
pub(crate) async fn exported(read: &Read, account: AccountId) -> Result<sofie::serde_json::Value> {
    let identities = AccountIdentity::query()
        .where_(AccountIdentity::ACCOUNT.eq(account))
        .all(read)
        .await?;
    Ok(sofie::serde_json::json!(
        identities
            .iter()
            .map(|identity| sofie::serde_json::json!({
                "provider": identity.provider,
                "subject": identity.subject,
                "email": identity.email.as_ref().map(Email::as_str),
            }))
            .collect::<Vec<_>>()
    ))
}

pub(crate) async fn removing(read: &Read, account: AccountId) -> Result<Vec<AccountIdentity>> {
    let mut identities = AccountIdentity::query()
        .where_(AccountIdentity::ACCOUNT.eq(account))
        .all(read)
        .await?;
    for identity in &mut identities {
        identity.subject = format!("closed-{}", identity.id().uuid());
        identity.email = None;
        identity.delete();
    }
    Ok(identities)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(variables: &[(&str, &str)]) -> Result<String> {
        base_url(|name| {
            Ok(variables
                .iter()
                .find(|(variable, _)| *variable == name)
                .map(|(_, value)| (*value).to_owned()))
        })
    }

    #[test]
    fn the_shared_base_url_serves_unless_oauth_has_its_own() {
        let shared = ("SOFIE_BASE_URL", "https://app.test");
        assert_eq!(read(&[shared]).unwrap(), "https://app.test");
        let own = ("SOFIE_OAUTH_BASE_URL", "https://login.app.test");
        assert_eq!(read(&[shared, own]).unwrap(), "https://login.app.test");
        let missing = read(&[]).unwrap_err().to_string();
        assert!(
            missing.starts_with("neither SOFIE_OAUTH_BASE_URL nor SOFIE_BASE_URL is set"),
            "{missing}"
        );
    }
}
