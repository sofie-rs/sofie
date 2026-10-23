use std::net::IpAddr;

use http::{HeaderMap, HeaderName};
use sofie_core::{Error, Result};

const FORWARDED_FOR: &str = "x-forwarded-for";
const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");
/// A misnamed header would put a credential in every log line.
const CREDENTIAL_HEADERS: [&str; 3] = ["authorization", "proxy-authorization", "cookie"];

/// Nothing is trusted by default, so a client cannot name its own address.
#[derive(Clone, Debug)]
pub struct TrustedProxies {
    networks: Vec<Network>,
    request_id_header: HeaderName,
}

impl Default for TrustedProxies {
    fn default() -> Self {
        Self {
            networks: Vec::new(),
            request_id_header: REQUEST_ID,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Network {
    address: IpAddr,
    prefix: u8,
}

impl crate::Serving {
    /// `sofie::run` reads them from `SOFIE_TRUSTED_PROXIES`.
    #[must_use]
    pub fn trusted_proxies(mut self, proxies: TrustedProxies) -> Self {
        self.proxies = proxies;
        self
    }
}

impl TrustedProxies {
    pub fn parse(text: &str) -> Result<Self> {
        let networks = text
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(Network::parse)
            .collect::<Result<_>>()?;
        Ok(Self {
            networks,
            ..Self::default()
        })
    }

    /// The header the proxies name their own request id in; `x-request-id` unless set.
    pub fn request_id_header(mut self, name: &str) -> Result<Self> {
        let header = HeaderName::try_from(name.trim()).map_err(|error| {
            Error::internal(
                format!("request id header `{name}` is not a header name"),
                error,
            )
        })?;
        if CREDENTIAL_HEADERS.contains(&header.as_str()) {
            return Err(Error::internal_message(format!(
                "request id header `{header}` carries credentials"
            )));
        }
        self.request_id_header = header;
        Ok(self)
    }

    fn trusts(&self, address: IpAddr) -> bool {
        self.networks
            .iter()
            .any(|network| network.contains(address))
    }

    /// `X-Forwarded-For` is read from the right; an unparsable hop ends the walk (it may be forged).
    pub(crate) fn client_of(&self, peer: IpAddr, headers: &HeaderMap) -> IpAddr {
        let mut client = peer.to_canonical();
        if !self.trusts(client) {
            return client;
        }
        for hop in forwarded_hops(headers).rev() {
            let Some(hop) = parsed_hop(hop) else {
                return client;
            };
            client = hop;
            if !self.trusts(hop) {
                return hop;
            }
        }
        client
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.networks.is_empty()
    }

    pub(crate) fn is_trusted(&self, peer: IpAddr) -> bool {
        self.trusts(peer.to_canonical())
    }

    /// The header naming the edge's request id comes only from a trusted peer.
    pub(crate) fn client(
        &self,
        peer: IpAddr,
        headers: &HeaderMap,
    ) -> (IpAddr, Option<&HeaderName>) {
        let edge_id_header = self.is_trusted(peer).then_some(&self.request_id_header);
        (self.client_of(peer, headers), edge_id_header)
    }
}

/// Split as bytes, so a byte that is not text, which a client can send, spoils only its own hop.
fn forwarded_hops(headers: &HeaderMap) -> impl DoubleEndedIterator<Item = &[u8]> {
    headers
        .get_all(FORWARDED_FOR)
        .into_iter()
        .flat_map(|line| line.as_bytes().split(|byte| *byte == b','))
}

fn parsed_hop(hop: &[u8]) -> Option<IpAddr> {
    let hop = std::str::from_utf8(hop).ok()?.trim();
    hop.parse::<IpAddr>().ok().map(|hop| hop.to_canonical())
}

impl Network {
    fn parse(entry: &str) -> Result<Self> {
        let refused = || {
            Error::internal_message(format!(
                "trusted proxy `{entry}` is not an address or a range such as 10.0.0.0/8"
            ))
        };
        let (address, prefix) = entry.split_once('/').unwrap_or((entry, ""));
        let address: IpAddr = address.parse().map_err(|_| refused())?;
        let address = address.to_canonical();
        let bits = bit_length(address);
        let prefix = match prefix {
            "" => bits,
            prefix => prefix.parse().map_err(|_| refused())?,
        };
        if prefix > bits {
            return Err(refused());
        }
        Ok(Self { address, prefix })
    }

    fn contains(self, address: IpAddr) -> bool {
        match (self.address, address) {
            (IpAddr::V4(network), IpAddr::V4(address)) => same_prefix(
                network.to_bits().into(),
                address.to_bits().into(),
                32,
                self.prefix,
            ),
            (IpAddr::V6(network), IpAddr::V6(address)) => {
                same_prefix(network.to_bits(), address.to_bits(), 128, self.prefix)
            }
            _ => false,
        }
    }
}

fn bit_length(address: IpAddr) -> u8 {
    match address {
        IpAddr::V4(_) => 32,
        IpAddr::V6(_) => 128,
    }
}

fn same_prefix(network: u128, address: u128, bits: u8, prefix: u8) -> bool {
    let ignored = u32::from(bits - prefix);
    network.checked_shr(ignored).unwrap_or(0) == address.checked_shr(ignored).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use http::HeaderValue;

    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    fn forwarded(lines: &[&'static str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for line in lines {
            headers.append(FORWARDED_FOR, HeaderValue::from_static(line));
        }
        headers
    }

    fn client(trusted: &str, peer: &str, lines: &[&'static str]) -> IpAddr {
        TrustedProxies::parse(trusted)
            .unwrap()
            .client_of(ip(peer), &forwarded(lines))
    }

    #[test]
    fn an_untrusted_peer_is_the_client_whatever_it_forwards() {
        assert_eq!(
            client("", "203.0.113.9", &["198.51.100.1"]),
            ip("203.0.113.9")
        );
        assert_eq!(
            client("10.0.0.0/8", "203.0.113.9", &["198.51.100.1"]),
            ip("203.0.113.9")
        );
    }

    #[test]
    fn behind_trusted_proxies_the_rightmost_untrusted_hop_is_the_client() {
        let lines = ["198.51.100.66, 203.0.113.5", "10.1.2.3"];

        assert_eq!(client("10.0.0.0/8", "10.0.0.1", &lines), ip("203.0.113.5"));
        assert_eq!(
            client("10.0.0.0/8, 203.0.113.5", "10.0.0.1", &lines),
            ip("198.51.100.66")
        );
    }

    #[test]
    fn a_hop_that_does_not_parse_ends_the_walk_at_the_last_trusted_one() {
        assert_eq!(
            client(
                "10.0.0.0/8",
                "10.0.0.1",
                &["203.0.113.5, unknown, 10.0.0.2"]
            ),
            ip("10.0.0.2")
        );
        assert_eq!(client("10.0.0.0/8", "10.0.0.1", &[]), ip("10.0.0.1"));
    }

    #[test]
    fn a_hop_that_is_not_text_spoils_only_itself() {
        let mut headers = HeaderMap::new();
        let line = HeaderValue::from_bytes(b"\xff, 203.0.113.5").unwrap();
        headers.append(FORWARDED_FOR, line);
        let proxies = TrustedProxies::parse("10.0.0.0/8").unwrap();

        assert_eq!(
            proxies.client_of(ip("10.0.0.1"), &headers),
            ip("203.0.113.5")
        );
    }

    #[test]
    fn ranges_match_by_prefix_in_either_family() {
        let proxies = TrustedProxies::parse("192.0.2.0/24, 2001:db8::/32, 198.51.100.7").unwrap();

        assert!(proxies.is_trusted(ip("192.0.2.200")));
        assert!(proxies.is_trusted(ip("::ffff:192.0.2.1")));
        assert!(proxies.is_trusted(ip("2001:db8:1::1")));
        assert!(proxies.is_trusted(ip("198.51.100.7")));
        assert!(!proxies.is_trusted(ip("192.0.3.1")));
        assert!(!proxies.is_trusted(ip("198.51.100.8")));
        assert!(
            TrustedProxies::parse("0.0.0.0/0")
                .unwrap()
                .is_trusted(ip("8.8.8.8"))
        );
    }

    #[test]
    fn a_malformed_entry_is_refused_naming_it() {
        for entry in ["10.0.0.0/33", "proxy.test", "10.0.0.0/x", "::/129"] {
            let error = TrustedProxies::parse(entry).unwrap_err();
            assert!(error.to_string().contains(entry), "{error}");
        }
    }
}
