//! Addresses are checked after resolution, where the host has become what is connected to.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use sofie_core::Error;

use crate::settings::Allowed;
use crate::target::Target;

const NO_PUBLIC_ADDRESS: &str = "it does not resolve to a public address";

/// Cloud metadata services outside the link-local range.
const METADATA: [IpAddr; 2] = [
    IpAddr::V6(Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254)),
    IpAddr::V4(Ipv4Addr::new(100, 100, 100, 200)),
];

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(crate) enum Reach {
    Vendor,
    Untrusted,
}

pub(crate) enum Failure {
    Refused(Error),
    Failed(Error),
}

impl Failure {
    pub(crate) fn into_error(self) -> Error {
        match self {
            Self::Refused(error) | Self::Failed(error) => error,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Destination {
    Public,
    Internal,
    Forbidden,
}

impl Reach {
    fn private(self, allowed: &Allowed) -> bool {
        self == Self::Vendor && allowed.private
    }

    pub(crate) fn refuse(self, host: &str, reason: &str) -> Failure {
        let message = refusal(host, reason);
        Failure::Refused(match self {
            Self::Vendor => Error::internal_message(message),
            Self::Untrusted => Error::BadRequest(message),
        })
    }

    pub(crate) fn admit(self, target: &Target, allowed: &Allowed) -> Result<(), Failure> {
        if !target.secure && !self.private(allowed) {
            return Err(self.refuse(&target.host, "it is not HTTPS"));
        }
        if self == Self::Vendor && !allowed.host(&target.host) {
            return Err(self.refuse(&target.host, "the host is not one the vendor uses"));
        }
        Ok(())
    }

    /// The caller of an untrusted request chooses its URL, so telling an internal, a forbidden
    /// and an unresolvable host apart by message would let it map the internal network. A failed
    /// lookup still stays a failure, so a resolver outage is retried rather than read as refusal.
    pub(crate) async fn resolve(
        self,
        target: &Target,
        allowed: &Allowed,
    ) -> Result<Vec<SocketAddr>, Failure> {
        let resolved = lookup(target)
            .await
            .and_then(|addresses| self.admitted(addresses, &target.host, allowed));
        match (self, resolved) {
            (Self::Untrusted, Err(Failure::Refused(_))) => {
                Err(self.refuse(&target.host, NO_PUBLIC_ADDRESS))
            }
            (Self::Untrusted, Err(Failure::Failed(_))) => Err(Failure::Failed(Error::Unavailable(
                refusal(&target.host, NO_PUBLIC_ADDRESS),
            ))),
            (_, resolved) => resolved,
        }
    }

    /// One refused address refuses them all: a host answering both public and internal addresses
    /// is trusted with neither.
    fn admitted(
        self,
        addresses: Vec<SocketAddr>,
        host: &str,
        allowed: &Allowed,
    ) -> Result<Vec<SocketAddr>, Failure> {
        for address in &addresses {
            match destination(address.ip()) {
                Destination::Public => {}
                Destination::Internal if self.private(allowed) => {}
                Destination::Internal => {
                    return Err(self.refuse(host, "it resolves to an internal address"));
                }
                Destination::Forbidden => {
                    return Err(self.refuse(host, "it resolves to a forbidden address"));
                }
            }
        }
        Ok(addresses)
    }
}

fn refusal(host: &str, reason: &str) -> String {
    format!("the outgoing request to `{host}` is refused: {reason}")
}

async fn lookup(target: &Target) -> Result<Vec<SocketAddr>, Failure> {
    let addresses: Vec<SocketAddr> = match target.host.parse::<IpAddr>() {
        Ok(address) => vec![SocketAddr::new(address, target.port)],
        Err(_) => tokio::net::lookup_host((target.host.as_str(), target.port))
            .await
            .map_err(|error| {
                Failure::Failed(Error::Unavailable(format!(
                    "resolving `{}` failed: {error}",
                    target.host
                )))
            })?
            .collect(),
    };
    if addresses.is_empty() {
        return Err(Failure::Failed(Error::Unavailable(format!(
            "`{}` resolves to no address",
            target.host
        ))));
    }
    Ok(addresses)
}

fn destination(address: IpAddr) -> Destination {
    if METADATA.contains(&address) {
        return Destination::Forbidden;
    }
    match address {
        IpAddr::V4(address) => destination_v4(address),
        IpAddr::V6(address) => destination_v6(address),
    }
}

/// Reached for embedded addresses too, so a mapped metadata address is still refused.
fn destination_v4(address: Ipv4Addr) -> Destination {
    if METADATA.contains(&IpAddr::V4(address)) {
        return Destination::Forbidden;
    }
    let [first, second, third, _] = address.octets();
    let shared_space = first == 100 && (64..128).contains(&second);
    if address.is_private() || address.is_loopback() || shared_space {
        return Destination::Internal;
    }
    let benchmarking = first == 198 && (18..20).contains(&second);
    let protocol_assignments = first == 192 && second == 0 && third == 0;
    if first == 0
        || first >= 224
        || address.is_link_local()
        || address.is_documentation()
        || benchmarking
        || protocol_assignments
    {
        return Destination::Forbidden;
    }
    Destination::Public
}

/// `::1` and `::` first: `to_ipv4` reads them as `0.0.0.1` and `0.0.0.0`.
fn destination_v6(address: Ipv6Addr) -> Destination {
    if address.is_loopback() || address.is_unique_local() {
        return Destination::Internal;
    }
    if address.is_unspecified() {
        return Destination::Forbidden;
    }
    if let Some(embedded) = embedded_v4(address) {
        return destination_v4(embedded);
    }
    let [first, second, third, ..] = address.segments();
    // RFC 8215: a translator inside the network, so at best an internal address.
    if [first, second, third] == [0x0064, 0xff9b, 1] {
        return match destination_v4(low_v4(address)) {
            Destination::Public => Destination::Internal,
            other => other,
        };
    }
    let documentation = first == 0x2001 && second == 0x0db8;
    let site_local = first & 0xffc0 == 0xfec0;
    if address.is_multicast() || address.is_unicast_link_local() || documentation || site_local {
        return Destination::Forbidden;
    }
    Destination::Public
}

/// IPv4-mapped, IPv4-compatible (which most systems do not route), SIIT's IPv4-translated,
/// RFC 6052 NAT64, 6to4 and Teredo, whose client address is stored inverted.
fn embedded_v4(address: Ipv6Addr) -> Option<Ipv4Addr> {
    if let Some(embedded) = address.to_ipv4() {
        return Some(embedded);
    }
    let segments = address.segments();
    match segments {
        [0x0064, 0xff9b, 0, 0, 0, 0, ..] | [0, 0, 0, 0, 0xffff, 0, ..] => Some(low_v4(address)),
        [0x2002, high, low, ..] => Some(v4(high, low)),
        [0x2001, 0, .., high, low] => Some(v4(!high, !low)),
        _ => None,
    }
}

fn low_v4(address: Ipv6Addr) -> Ipv4Addr {
    let [.., high, low] = address.segments();
    v4(high, low)
}

fn v4(high: u16, low: u16) -> Ipv4Addr {
    Ipv4Addr::from((u32::from(high) << 16) | u32::from(low))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(address: &str) -> Destination {
        destination(address.parse().unwrap())
    }

    #[test]
    fn public_addresses_pass() {
        assert_eq!(of("93.184.215.14"), Destination::Public);
        assert_eq!(of("2606:4700::6810:84e5"), Destination::Public);
        assert_eq!(of("2002:5db8:d70e::1"), Destination::Public);
    }

    #[test]
    fn internal_addresses_are_told_apart_from_forbidden_ones() {
        for internal in [
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "127.0.0.1",
            "100.64.0.1",
        ] {
            assert_eq!(of(internal), Destination::Internal, "{internal}");
        }
        for internal in [
            "::1",
            "fd12::1",
            "::ffff:10.0.0.1",
            "::127.0.0.1",
            "64:ff9b:1::808:808",
            "2002:a00:1::1",
            "2001:0:4136:e378:8000:63bf:f5fe:fffe",
            "::ffff:0:7f00:1",
        ] {
            assert_eq!(of(internal), Destination::Internal, "{internal}");
        }
        for forbidden in [
            "169.254.169.254",
            "0.0.0.0",
            "224.0.0.1",
            "255.255.255.255",
            "192.0.2.1",
            "100.100.100.200",
            "fe80::1",
            "fd00:ec2::254",
            "::",
            "64:ff9b::a9fe:a9fe",
            "::ffff:169.254.169.254",
            "::169.254.169.254",
            "64:ff9b:1::a9fe:a9fe",
            "2002:a9fe:a9fe::1",
            "2001:0:4136:e378:8000:63bf:5601:5601",
            "::ffff:100.100.100.200",
            "64:ff9b::6464:64c8",
            "2002:6464:64c8::1",
        ] {
            assert_eq!(of(forbidden), Destination::Forbidden, "{forbidden}");
        }
    }
}
