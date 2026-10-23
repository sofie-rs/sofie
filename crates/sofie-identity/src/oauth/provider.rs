use sofie::core::{Error, Result};
use sofie::values::Secret;

use super::keys::{Keys, KeysAt};

/// Microsoft's `iss` names the account's tenant, filled from `tid`, never `common`
/// (<https://learn.microsoft.com/entra/identity-platform/access-tokens#validate-the-issuer>).
pub(crate) const TENANT_ID: &str = "{tenantid}";

#[derive(Debug)]
pub(crate) enum Claims {
    /// OIDC: the ID token the token endpoint answers, from `issuer`, signed by `keys`.
    IdToken { issuer: String, keys: Keys },
    /// Plain OAuth 2: a JSON user endpoint, and one listing verified addresses.
    UserEndpoints { user: String, emails: String },
}

/// One provider an account signs in with; its name is the stored identity's and the form's.
#[derive(Debug)]
pub struct Provider {
    pub(crate) name: String,
    pub(crate) label: String,
    pub(crate) authorization_url: String,
    pub(crate) token_url: String,
    pub(crate) claims: Claims,
    pub(crate) client_id: String,
    pub(crate) client_secret: Secret<String>,
    pub(crate) scopes: String,
}

impl Provider {
    pub fn openid(
        name: &str,
        label: &str,
        issuer: &str,
        authorization_url: &str,
        token_url: &str,
        client: (String, Secret<String>),
    ) -> Result<Self> {
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
        {
            return Err(Error::internal_message(format!(
                "an OAuth provider's name is lowercase letters and `-`, not `{name}`"
            )));
        }
        let (client_id, client_secret) = client;
        Ok(Self {
            name: name.to_owned(),
            label: label.to_owned(),
            authorization_url: authorization_url.to_owned(),
            token_url: token_url.to_owned(),
            claims: Claims::IdToken {
                issuer: issuer.to_owned(),
                keys: Keys::new(KeysAt::Discovery),
            },
            client_id,
            client_secret,
            scopes: "openid email".to_owned(),
        })
    }

    pub fn google(client: (String, Secret<String>)) -> Result<Self> {
        Self::openid(
            "google",
            "Google",
            "https://accounts.google.com",
            "https://accounts.google.com/o/oauth2/v2/auth",
            "https://oauth2.googleapis.com/token",
            client,
        )
        .map(|google| google.keys_at("https://www.googleapis.com/oauth2/v3/certs"))
    }

    /// Work, school and personal Microsoft accounts of one tenant (`common` for any).
    pub fn microsoft(tenant: &str, client: (String, Secret<String>)) -> Result<Self> {
        Self::openid(
            "microsoft",
            "Microsoft",
            &format!("https://login.microsoftonline.com/{TENANT_ID}/v2.0"),
            &format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/authorize"),
            &format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token"),
            client,
        )
        .map(|microsoft| {
            microsoft.keys_at(&format!(
                "https://login.microsoftonline.com/{tenant}/discovery/v2.0/keys"
            ))
        })
    }

    /// A provider whose key set is known needs no discovery document.
    fn keys_at(mut self, url: &str) -> Self {
        if let Claims::IdToken { keys, .. } = &mut self.claims {
            *keys = Keys::new(KeysAt::Url(url.to_owned()));
        }
        self
    }

    /// GitHub speaks plain OAuth 2, so the account and its verified addresses are asked of its API.
    pub fn github(client: (String, Secret<String>)) -> Result<Self> {
        let (client_id, client_secret) = client;
        Ok(Self {
            name: "github".to_owned(),
            label: "GitHub".to_owned(),
            authorization_url: "https://github.com/login/oauth/authorize".to_owned(),
            token_url: "https://github.com/login/oauth/access_token".to_owned(),
            claims: Claims::UserEndpoints {
                user: "https://api.github.com/user".to_owned(),
                emails: "https://api.github.com/user/emails".to_owned(),
            },
            client_id,
            client_secret,
            scopes: "read:user user:email".to_owned(),
        })
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Every URL this side requests, so the outgoing client allows their hosts only.
    pub(crate) fn requested_urls(&self) -> Vec<&str> {
        let mut urls = vec![self.token_url.as_str()];
        match &self.claims {
            Claims::IdToken { issuer, keys } => urls.push(keys.url().unwrap_or(issuer)),
            Claims::UserEndpoints { user, emails } => urls.extend([user.as_str(), emails.as_str()]),
        }
        urls
    }
}
