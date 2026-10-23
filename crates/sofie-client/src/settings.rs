use std::time::Duration;

use sofie_core::{Error, Result};

use crate::target::{Target, normalized_host};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;
const RETRIES: u32 = 2;

/// Keys `<MODULE>_<VENDOR>_*`; all but `BASE_URL` have defaults, and only HTTP vendors need it.
#[derive(Debug, Default)]
pub struct Vendor {
    pub base_url: Option<String>,
    pub hosts: Option<String>,
    /// Allows private and loopback addresses and plain HTTP; link-local and metadata stay refused.
    pub private: Option<bool>,
    pub connect_timeout_ms: Option<u64>,
    pub timeout_ms: Option<u64>,
    pub max_response_bytes: Option<u64>,
    pub retries: Option<u32>,
    pub redirects: Option<u32>,
}

pub(crate) struct Allowed {
    /// Lowercase.
    pub(crate) hosts: Vec<String>,
    pub(crate) private: bool,
}

impl Allowed {
    pub(crate) fn host(&self, host: &str) -> bool {
        self.hosts.iter().any(|allowed| allowed == host)
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    pub(crate) connect: Duration,
    pub(crate) total: Duration,
    pub(crate) response_bytes: usize,
    pub(crate) retries: u32,
    pub(crate) redirects: u32,
}

impl Vendor {
    pub(crate) fn read(self, vendor: &str) -> Result<(Option<Target>, Allowed, Limits)> {
        let base = self
            .base_url
            .as_deref()
            .map(|url| {
                let base = Target::parse(url)?;
                match base.path.query() {
                    Some(_) => {
                        Err("it carries a query, which a path would be joined after".to_owned())
                    }
                    None => Ok(base),
                }
            })
            .transpose()
            .map_err(|problem| {
                Error::internal_message(format!("{vendor}'s BASE_URL is refused: {problem}"))
            })?;
        let listed = self.hosts.as_deref().unwrap_or_default().split(',');
        let mut hosts: Vec<String> = listed
            .map(|host| normalized_host(host.trim()))
            .filter(|host| !host.is_empty())
            .collect();
        hosts.extend(base.iter().map(|base| base.host.clone()));
        let milliseconds =
            |value: Option<u64>, default| value.map_or(default, Duration::from_millis);
        let limits = Limits {
            connect: milliseconds(self.connect_timeout_ms, CONNECT_TIMEOUT),
            total: milliseconds(self.timeout_ms, TOTAL_TIMEOUT),
            response_bytes: usize::try_from(self.max_response_bytes.unwrap_or(MAX_RESPONSE_BYTES))
                .unwrap_or(usize::MAX),
            retries: self.retries.unwrap_or(RETRIES),
            redirects: self.redirects.unwrap_or(0),
        };
        let allowed = Allowed {
            hosts,
            private: self.private.unwrap_or(false),
        };
        Ok((base, allowed, limits))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_base_url_with_a_query_is_refused() {
        let settings = Vendor {
            base_url: Some("https://api.vendor.test/v1?key=abc".to_owned()),
            ..Vendor::default()
        };

        let Err(refused) = settings.read("vendor") else {
            panic!("a base URL with a query is refused");
        };

        assert!(refused.to_string().contains("query"), "{refused}");
    }

    #[test]
    fn a_listed_ipv6_host_matches_with_or_without_brackets() {
        let settings = Vendor {
            hosts: Some("[::1], 2001:DB8::1".to_owned()),
            ..Vendor::default()
        };

        let (_, allowed, _) = settings.read("vendor").unwrap();

        assert!(allowed.host("::1"));
        assert!(allowed.host("2001:db8::1"));
    }
}
