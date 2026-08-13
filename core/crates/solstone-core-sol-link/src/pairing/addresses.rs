// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pair-link address discovery and wire encoding.

use std::ffi::CStr;
use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};

use serde::Serialize;

/// A classified local endpoint retained for direct pairing.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct LocalEndpoint {
    pub ip: IpAddr,
    pub scope: EndpointScope,
}

/// The source category used by the reference candidate ordering.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EndpointScope {
    Lan,
    Ula,
    Vpn,
}

/// An address returned from the platform enumeration seam.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawInterfaceAddress {
    pub interface: String,
    pub address: IpAddr,
}

/// The level-A injected discovery snapshot.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PairingSnapshot {
    pub endpoints: Vec<LocalEndpoint>,
    pub route_ipv4: Option<Ipv4Addr>,
}

#[derive(Debug)]
pub enum AddressError {
    Enumeration(io::Error),
}

impl fmt::Display for AddressError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Enumeration(error) => {
                write!(formatter, "could not enumerate local interfaces: {error}")
            }
        }
    }
}

impl std::error::Error for AddressError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Enumeration(error) => Some(error),
        }
    }
}

/// Raw enumeration seam. Tests supply synthetic records and never inspect host
/// interfaces.
pub trait RawInterfaceSource {
    fn enumerate(&self) -> Result<Vec<RawInterfaceAddress>, AddressError>;
}

/// Route-probe seam. The production probe calls UDP `connect`, which sends no
/// packets; tests supply a fixed route and never open a socket.
pub trait RouteIpv4Source {
    fn route_ipv4(&self) -> Option<Ipv4Addr>;
}

/// Production raw-interface source.
pub struct SystemInterfaceSource;

impl RawInterfaceSource for SystemInterfaceSource {
    fn enumerate(&self) -> Result<Vec<RawInterfaceAddress>, AddressError> {
        enumerate_system_interfaces()
    }
}

/// Production route source. The route address comes from this probe, never the
/// interface enumeration, so a VPN's selected egress remains observable.
pub struct SystemRouteIpv4Source;

impl RouteIpv4Source for SystemRouteIpv4Source {
    fn route_ipv4(&self) -> Option<Ipv4Addr> {
        let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))).ok()?;
        socket.connect((Ipv4Addr::new(8, 8, 8, 8), 80)).ok()?;
        match socket.local_addr().ok()?.ip() {
            IpAddr::V4(address) => Some(address),
            IpAddr::V6(_) => None,
        }
    }
}

/// Construct a snapshot through the level-B production seams.
pub fn snapshot_from_sources(
    interfaces: &impl RawInterfaceSource,
    route: &impl RouteIpv4Source,
) -> Result<PairingSnapshot, AddressError> {
    Ok(PairingSnapshot {
        endpoints: classify_interface_addresses(&interfaces.enumerate()?),
        route_ipv4: route.route_ipv4(),
    })
}

/// Pure classifier for platform records. This is deliberately separate from
/// `getifaddrs` so tests drive the real classification without host I/O.
pub fn classify_interface_addresses(raw: &[RawInterfaceAddress]) -> Vec<LocalEndpoint> {
    let mut endpoints = raw.iter().filter_map(classify_one).collect::<Vec<_>>();
    endpoints.sort_by_key(|endpoint| (endpoint.scope, endpoint.ip));
    endpoints.dedup();
    endpoints
}

/// Port of Python's `is_usable_ipv4`.
pub fn is_usable_ipv4(address: Ipv4Addr) -> bool {
    !(address.is_loopback()
        || address.is_unspecified()
        || address.is_link_local()
        || address.is_multicast())
}

/// Port of `resolve_pair_link_candidates`: filter first, put a matching route
/// first within its class, de-duplicate before capping, then cap to four.
pub fn resolve_pair_link_candidates(
    endpoints: &[LocalEndpoint],
    route_ipv4: Option<Ipv4Addr>,
) -> Vec<Ipv4Addr> {
    let usable_route = route_ipv4.filter(|address| is_usable_ipv4(*address));
    let filtered = endpoints
        .iter()
        .filter_map(|endpoint| match endpoint.ip {
            IpAddr::V4(address) if is_usable_ipv4(address) => Some((address, endpoint.scope)),
            IpAddr::V4(_) | IpAddr::V6(_) => None,
        })
        .collect::<Vec<_>>();
    if filtered.is_empty() {
        return usable_route.into_iter().collect();
    }
    let mut non_vpn = Vec::new();
    let mut vpn = Vec::new();
    for (address, scope) in filtered {
        if scope == EndpointScope::Vpn {
            vpn.push(address);
        } else {
            non_vpn.push(address);
        }
    }
    if let Some(route) = usable_route {
        for group in [&mut non_vpn, &mut vpn] {
            if let Some(index) = group.iter().position(|address| *address == route) {
                group.remove(index);
                group.insert(0, route);
                break;
            }
        }
    }
    let mut deduplicated = Vec::new();
    for address in non_vpn.into_iter().chain(vpn) {
        if !deduplicated.contains(&address) {
            deduplicated.push(address);
        }
    }
    deduplicated.truncate(4);
    deduplicated
}

/// Encode direct candidates into the pinned SPL pair-link format.
pub fn encode_pair_link(
    candidates: &[Ipv4Addr],
    nonce: [u8; 16],
    ca_fp_prefix: [u8; 16],
) -> Result<String, PairLinkEncodeError> {
    if candidates.is_empty() || candidates.len() > 4 {
        return Err(PairLinkEncodeError::CandidateCount(candidates.len()));
    }
    if candidates
        .iter()
        .any(|address| !is_allowed_direct_ipv4(*address))
    {
        return Err(PairLinkEncodeError::DisallowedAddress);
    }
    Ok(encode_unchecked_pair_link(candidates, nonce, ca_fp_prefix))
}

/// The configured-home branch intentionally bypasses resolver and parser-range
/// eligibility, matching the reference's explicit single-host behavior.
pub fn encode_configured_home_pair_link(
    home: Ipv4Addr,
    nonce: [u8; 16],
    ca_fp_prefix: [u8; 16],
) -> String {
    encode_unchecked_pair_link(&[home], nonce, ca_fp_prefix)
}

#[derive(Debug, Eq, PartialEq)]
pub enum PairLinkEncodeError {
    CandidateCount(usize),
    DisallowedAddress,
}

impl fmt::Display for PairLinkEncodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CandidateCount(count) => write!(
                formatter,
                "pair-link candidate count must be 1 through 4, got {count}"
            ),
            Self::DisallowedAddress => formatter
                .write_str("pair-link candidate is outside SPL's allowed direct IPv4 ranges"),
        }
    }
}

impl std::error::Error for PairLinkEncodeError {}

fn classify_one(entry: &RawInterfaceAddress) -> Option<LocalEndpoint> {
    let interface = entry.interface.as_str();
    if ["lo", "docker", "br-", "vbox", "vmnet", "tap"]
        .iter()
        .any(|prefix| interface.starts_with(prefix))
    {
        return None;
    }
    let overlay = ["utun", "tun", "tailscale"]
        .iter()
        .any(|prefix| interface.starts_with(prefix));
    match entry.address {
        IpAddr::V4(address) if is_rfc1918(address) && !overlay => Some(LocalEndpoint {
            ip: IpAddr::V4(address),
            scope: EndpointScope::Lan,
        }),
        IpAddr::V4(address) if is_cgnat(address) && overlay => Some(LocalEndpoint {
            ip: IpAddr::V4(address),
            scope: EndpointScope::Vpn,
        }),
        IpAddr::V6(address) if is_ula(address) => Some(LocalEndpoint {
            ip: IpAddr::V6(address),
            scope: EndpointScope::Ula,
        }),
        IpAddr::V4(_) | IpAddr::V6(_) => None,
    }
}

fn is_rfc1918(address: Ipv4Addr) -> bool {
    let value = u32::from(address);
    (0x0a00_0000..=0x0aff_ffff).contains(&value)
        || (0xac10_0000..=0xac1f_ffff).contains(&value)
        || (0xc0a8_0000..=0xc0a8_ffff).contains(&value)
}

fn is_cgnat(address: Ipv4Addr) -> bool {
    (0x6440_0000..=0x647f_ffff).contains(&u32::from(address))
}

fn is_ula(address: Ipv6Addr) -> bool {
    (address.octets()[0] & 0xfe) == 0xfc
}

fn is_allowed_direct_ipv4(address: Ipv4Addr) -> bool {
    let value = u32::from(address);
    [
        (0x0a00_0000, 0x0aff_ffff),
        (0xac10_0000, 0xac1f_ffff),
        (0xc0a8_0000, 0xc0a8_ffff),
        (0xa9fe_0000, 0xa9fe_ffff),
        (0x6440_0000, 0x647f_ffff),
        (0x7f00_0000, 0x7fff_ffff),
    ]
    .iter()
    .any(|(low, high)| (*low..=*high).contains(&value))
}

fn encode_unchecked_pair_link(
    candidates: &[Ipv4Addr],
    nonce: [u8; 16],
    ca_fp_prefix: [u8; 16],
) -> String {
    let mut blob = Vec::new();
    if candidates.len() == 1 {
        blob.extend([0x04, 0x01]);
        blob.extend(candidates[0].octets());
        blob.extend(spl_core::DEFAULT_DIRECT_PORT.to_be_bytes());
    } else {
        blob.extend([0x05, 0x01, candidates.len() as u8]);
        blob.extend(spl_core::DEFAULT_DIRECT_PORT.to_be_bytes());
        for address in candidates {
            blob.extend(address.octets());
        }
    }
    blob.extend(nonce);
    blob.extend(ca_fp_prefix);
    format!(
        "https://go.solstone.app/p#{}",
        spl_core::crockford::encode(&blob)
    )
}

#[cfg(unix)]
fn enumerate_system_interfaces() -> Result<Vec<RawInterfaceAddress>, AddressError> {
    let mut head = std::ptr::null_mut::<libc::ifaddrs>();
    // SAFETY: libc initializes `head` on success; the guard below frees exactly
    // that list before return and no list node escapes this function.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return Err(AddressError::Enumeration(io::Error::last_os_error()));
    }
    struct IfAddrs(*mut libc::ifaddrs);
    impl Drop for IfAddrs {
        fn drop(&mut self) {
            // SAFETY: this guard owns the list returned by `getifaddrs`.
            unsafe { libc::freeifaddrs(self.0) };
        }
    }
    let _guard = IfAddrs(head);
    let mut entries = Vec::new();
    let mut current = head;
    while !current.is_null() {
        // SAFETY: `current` walks the valid linked list owned by `_guard`.
        let item = unsafe { &*current };
        if !item.ifa_addr.is_null() && !item.ifa_name.is_null() {
            // SAFETY: both pointers are valid within the owned getifaddrs list.
            let name = unsafe { CStr::from_ptr(item.ifa_name) }
                .to_string_lossy()
                .into_owned();
            // SAFETY: `ifa_addr` points at a sockaddr whose family selects the cast.
            let address = unsafe {
                match (*item.ifa_addr).sa_family as i32 {
                    libc::AF_INET => {
                        let ipv4 = &*(item.ifa_addr.cast::<libc::sockaddr_in>());
                        Some(IpAddr::V4(Ipv4Addr::from(u32::from_be(
                            ipv4.sin_addr.s_addr,
                        ))))
                    }
                    libc::AF_INET6 => {
                        let ipv6 = &*(item.ifa_addr.cast::<libc::sockaddr_in6>());
                        Some(IpAddr::V6(Ipv6Addr::from(ipv6.sin6_addr.s6_addr)))
                    }
                    _ => None,
                }
            };
            if let Some(address) = address {
                entries.push(RawInterfaceAddress {
                    interface: name,
                    address,
                });
            }
        }
        current = item.ifa_next;
    }
    Ok(entries)
}

#[cfg(not(unix))]
fn enumerate_system_interfaces() -> Result<Vec<RawInterfaceAddress>, AddressError> {
    Err(AddressError::Enumeration(io::Error::other(
        "getifaddrs is unavailable on this platform",
    )))
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use super::*;

    fn endpoint(address: &str, scope: EndpointScope) -> LocalEndpoint {
        LocalEndpoint {
            ip: address.parse().expect("address"),
            scope,
        }
    }

    #[test]
    fn classifier_keeps_only_supported_interface_classes() {
        let raw = vec![
            RawInterfaceAddress {
                interface: "eth0".into(),
                address: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 3)),
            },
            RawInterfaceAddress {
                interface: "tailscale0".into(),
                address: IpAddr::V4(Ipv4Addr::new(100, 64, 0, 2)),
            },
            RawInterfaceAddress {
                interface: "eth0".into(),
                address: IpAddr::V6(Ipv6Addr::LOCALHOST),
            },
            RawInterfaceAddress {
                interface: "eth0".into(),
                address: "fd00::2".parse().expect("ula"),
            },
            RawInterfaceAddress {
                interface: "docker0".into(),
                address: IpAddr::V4(Ipv4Addr::new(172, 17, 0, 2)),
            },
        ];
        assert_eq!(
            classify_interface_addresses(&raw),
            vec![
                endpoint("192.168.1.3", EndpointScope::Lan),
                endpoint("fd00::2", EndpointScope::Ula),
                endpoint("100.64.0.2", EndpointScope::Vpn),
            ]
        );
    }

    #[test]
    fn resolver_orders_deduplicates_before_cap_and_keeps_vpn_first_when_alone() {
        let endpoints = vec![
            endpoint("192.168.1.2", EndpointScope::Lan),
            endpoint("192.168.1.3", EndpointScope::Lan),
            endpoint("192.168.1.2", EndpointScope::Lan),
            endpoint("10.0.0.2", EndpointScope::Lan),
            endpoint("10.0.0.3", EndpointScope::Lan),
            endpoint("100.64.0.2", EndpointScope::Vpn),
        ];
        assert_eq!(
            resolve_pair_link_candidates(&endpoints, Some(Ipv4Addr::new(10, 0, 0, 2))),
            vec![
                Ipv4Addr::new(10, 0, 0, 2),
                Ipv4Addr::new(192, 168, 1, 2),
                Ipv4Addr::new(192, 168, 1, 3),
                Ipv4Addr::new(10, 0, 0, 3)
            ]
        );
        assert_eq!(
            resolve_pair_link_candidates(
                &[endpoint("100.64.0.2", EndpointScope::Vpn)],
                Some(Ipv4Addr::new(100, 64, 0, 2))
            ),
            vec![Ipv4Addr::new(100, 64, 0, 2)]
        );
    }

    #[test]
    fn usable_filter_and_empty_filtered_route_twin_match_reference() {
        assert!(is_usable_ipv4(Ipv4Addr::new(10, 0, 0, 2)));
        assert!(!is_usable_ipv4(Ipv4Addr::LOCALHOST));
        assert_eq!(
            resolve_pair_link_candidates(&[], None),
            Vec::<Ipv4Addr>::new()
        );
        assert_eq!(
            resolve_pair_link_candidates(&[], Some(Ipv4Addr::new(10, 0, 0, 2))),
            vec![Ipv4Addr::new(10, 0, 0, 2)]
        );
    }

    #[test]
    fn encoded_links_round_trip_at_the_spl_boundary() {
        let nonce = [7; 16];
        let pin = [9; 16];
        for candidates in [
            vec![Ipv4Addr::new(10, 0, 0, 2)],
            vec![Ipv4Addr::new(10, 0, 0, 2), Ipv4Addr::new(192, 168, 1, 2)],
        ] {
            let link = encode_pair_link(&candidates, nonce, pin).expect("link");
            let blob = spl_core::crockford::decode(link.split('#').nth(1).expect("fragment"))
                .expect("decode");
            assert_eq!(blob[0], if candidates.len() == 1 { 0x04 } else { 0x05 });
            assert!(matches!(
                spl_core::pairlink::parse(&link),
                Ok(spl_core::pairlink::ParsedPairLink::Direct(_))
            ));
        }
        assert_eq!(
            encode_pair_link(&[Ipv4Addr::new(8, 8, 8, 8)], nonce, pin),
            Err(PairLinkEncodeError::DisallowedAddress)
        );
    }

    #[test]
    fn configured_home_is_single_host_v04_without_resolution() {
        let link =
            encode_configured_home_pair_link(Ipv4Addr::new(192, 168, 1, 7), [1; 16], [2; 16]);
        let blob =
            spl_core::crockford::decode(link.split('#').nth(1).expect("fragment")).expect("decode");
        assert_eq!(blob[0], 0x04);
    }
}
