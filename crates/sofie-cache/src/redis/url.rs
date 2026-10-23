use std::fmt;

use sofie_core::encoding::percent;
use sofie_core::{Error, Result};

const DEFAULT_PORT: u16 = 6379;

/// `redis://[[user]:password@]host[:port][/database]`, or `rediss://` for TLS.
#[derive(Clone, PartialEq, Eq)]
pub struct RedisUrl {
    pub(crate) secure: bool,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) user: Option<String>,
    pub(crate) password: Option<String>,
    pub(crate) database: u32,
}

/// The password never shows, as this may be logged.
impl fmt::Debug for RedisUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let scheme = if self.secure { "rediss" } else { "redis" };
        let user = self.user.as_deref().unwrap_or_default();
        let credentials = match &self.password {
            Some(_) => format!("{user}:***@"),
            None if !user.is_empty() => format!("{user}@"),
            None => String::new(),
        };
        write!(
            formatter,
            "{scheme}://{credentials}{}:{}/{}",
            self.host, self.port, self.database
        )
    }
}

/// Errors say which part is wrong without quoting the URL, which may hold a password.
fn refused(what: &str) -> Error {
    Error::internal_message(format!("the Redis URL is malformed: {what}"))
}

impl RedisUrl {
    pub fn parse(url: &str) -> Result<Self> {
        let (secure, rest) = if let Some(rest) = url.strip_prefix("rediss://") {
            (true, rest)
        } else if let Some(rest) = url.strip_prefix("redis://") {
            (false, rest)
        } else {
            return Err(refused("it starts with neither `redis://` nor `rediss://`"));
        };
        if rest.contains(['?', '#']) {
            return Err(refused("it has a query or fragment, which nothing reads"));
        }
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        let (credentials, address) = match authority.rsplit_once('@') {
            Some((credentials, address)) => (Some(credentials), address),
            None => (None, authority),
        };
        let (user, password) = credentials.map_or(Ok((None, None)), parse_credentials)?;
        let (host, port) = parse_address(address)?;
        let database = match path {
            "" => 0,
            number => number
                .parse()
                .map_err(|_| refused("its path is not a database number"))?,
        };
        Ok(Self {
            secure,
            host,
            port,
            user,
            password,
            database,
        })
    }
}

fn parse_credentials(credentials: &str) -> Result<(Option<String>, Option<String>)> {
    let (user, password) = match credentials.split_once(':') {
        Some((user, password)) => (user, Some(password)),
        None => (credentials, None),
    };
    let user = (!user.is_empty())
        .then(|| percent_decoded(user))
        .transpose()?;
    Ok((user, password.map(percent_decoded).transpose()?))
}

fn parse_address(address: &str) -> Result<(String, u16)> {
    let (host, port) = match address.strip_prefix('[') {
        Some(bracketed) => {
            let (host, after) = bracketed
                .split_once(']')
                .ok_or_else(|| refused("an IPv6 host is not closed by `]`"))?;
            (host, after.strip_prefix(':'))
        }
        None => match address.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (address, None),
        },
    };
    if host.is_empty() {
        return Err(refused("it names no host"));
    }
    let port = port
        .map(|port| {
            port.parse()
                .map_err(|_| refused("its port is not a number"))
        })
        .transpose()?
        .unwrap_or(DEFAULT_PORT);
    Ok((host.to_owned(), port))
}

fn percent_decoded(text: &str) -> Result<String> {
    let bytes = percent::decode(text.as_bytes())
        .ok_or_else(|| refused("a `%` is not followed by two hex digits"))?;
    String::from_utf8(bytes).map_err(|_| refused("its credentials are not UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_part_is_read_and_defaults_fill_the_rest() {
        let full = RedisUrl::parse("rediss://app:p%40ss%3Aw@cache.internal:6380/2").unwrap();
        assert!(full.secure);
        assert_eq!(full.host, "cache.internal");
        assert_eq!(full.port, 6380);
        assert_eq!(full.user.as_deref(), Some("app"));
        assert_eq!(full.password.as_deref(), Some("p@ss:w"));
        assert_eq!(full.database, 2);

        let bare = RedisUrl::parse("redis://localhost").unwrap();
        assert_eq!((bare.port, bare.database, bare.secure), (6379, 0, false));
        assert_eq!((bare.user, bare.password), (None, None));

        let password_only = RedisUrl::parse("redis://:secret@[::1]:7000").unwrap();
        assert_eq!(password_only.host, "::1");
        assert_eq!(password_only.port, 7000);
        assert_eq!(password_only.user, None);
        assert_eq!(password_only.password.as_deref(), Some("secret"));
    }

    #[test]
    fn a_malformed_url_is_refused_without_quoting_it() {
        for url in [
            "http://secret@host",
            "redis://:secret@",
            "redis://:secret@host:port",
            "redis://:secret@host/db",
            "redis://:se%zzcret@host",
            "redis://:secret@host?tls=true",
        ] {
            let error = RedisUrl::parse(url).unwrap_err().to_string();
            assert!(!error.contains("secret"), "{error}");
        }
    }

    #[test]
    fn debug_hides_the_password() {
        let url = RedisUrl::parse("redis://app:hunter2@host/1").unwrap();

        assert_eq!(format!("{url:?}"), "redis://app:***@host:6379/1");
    }
}
