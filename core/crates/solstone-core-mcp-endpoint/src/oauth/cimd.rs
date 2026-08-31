// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! CIMD address classification and known-client table.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Exact CIMD document URLs treated as known clients.
///
/// Empty until vendor-published Claude/Codex URLs are filled in.
pub(crate) const KNOWN_CLIENT_CIMD_URLS: &[&str] = &[];

/// Why a resolved address must not be contacted for a CIMD fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnusableIpClass {
    Unspecified,
    Loopback,
    Private,
    LinkLocal,
    Multicast,
    ReservedDocumentation,
    Broadcast,
}

/// Map IPv4-mapped IPv6 to IPv4; leave every other address unchanged.
pub(crate) fn canonicalize_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        IpAddr::V4(_) => ip,
    }
}

/// Classify an address that must not be used as a CIMD fetch target.
pub(crate) fn classify_unusable_ip(ip: IpAddr) -> Option<UnusableIpClass> {
    match canonicalize_ip(ip) {
        IpAddr::V4(address) => classify_v4(address),
        IpAddr::V6(address) => classify_v6(address),
    }
}

/// True when `url` is an exact known-client CIMD document URL.
pub(crate) fn is_known_cimd_url(url: &str) -> bool {
    KNOWN_CLIENT_CIMD_URLS.contains(&url)
}

fn classify_v4(address: Ipv4Addr) -> Option<UnusableIpClass> {
    if address.is_broadcast() {
        return Some(UnusableIpClass::Broadcast);
    }
    let bits = u32::from(address);
    if in_range(
        bits,
        Ipv4Addr::new(0, 0, 0, 0),
        Ipv4Addr::new(0, 255, 255, 255),
    ) {
        return Some(UnusableIpClass::Unspecified);
    }
    if address.is_loopback() {
        return Some(UnusableIpClass::Loopback);
    }
    if in_range(
        bits,
        Ipv4Addr::new(10, 0, 0, 0),
        Ipv4Addr::new(10, 255, 255, 255),
    ) || in_range(
        bits,
        Ipv4Addr::new(100, 64, 0, 0),
        Ipv4Addr::new(100, 127, 255, 255),
    ) || in_range(
        bits,
        Ipv4Addr::new(172, 16, 0, 0),
        Ipv4Addr::new(172, 31, 255, 255),
    ) || in_range(
        bits,
        Ipv4Addr::new(192, 168, 0, 0),
        Ipv4Addr::new(192, 168, 255, 255),
    ) {
        return Some(UnusableIpClass::Private);
    }
    if in_range(
        bits,
        Ipv4Addr::new(169, 254, 0, 0),
        Ipv4Addr::new(169, 254, 255, 255),
    ) {
        return Some(UnusableIpClass::LinkLocal);
    }
    if in_range(
        bits,
        Ipv4Addr::new(224, 0, 0, 0),
        Ipv4Addr::new(239, 255, 255, 255),
    ) {
        return Some(UnusableIpClass::Multicast);
    }
    if in_range(
        bits,
        Ipv4Addr::new(192, 0, 2, 0),
        Ipv4Addr::new(192, 0, 2, 255),
    ) || in_range(
        bits,
        Ipv4Addr::new(198, 51, 100, 0),
        Ipv4Addr::new(198, 51, 100, 255),
    ) || in_range(
        bits,
        Ipv4Addr::new(203, 0, 113, 0),
        Ipv4Addr::new(203, 0, 113, 255),
    ) || in_range(
        bits,
        Ipv4Addr::new(192, 0, 0, 0),
        Ipv4Addr::new(192, 0, 0, 255),
    ) || in_range(
        bits,
        Ipv4Addr::new(192, 88, 99, 0),
        Ipv4Addr::new(192, 88, 99, 255),
    ) || in_range(
        bits,
        Ipv4Addr::new(198, 18, 0, 0),
        Ipv4Addr::new(198, 19, 255, 255),
    ) || in_range(
        bits,
        Ipv4Addr::new(240, 0, 0, 0),
        Ipv4Addr::new(255, 255, 255, 254),
    ) {
        return Some(UnusableIpClass::ReservedDocumentation);
    }
    None
}

fn classify_v6(address: Ipv6Addr) -> Option<UnusableIpClass> {
    if address.is_unspecified() {
        return Some(UnusableIpClass::Unspecified);
    }
    if address.is_loopback() {
        return Some(UnusableIpClass::Loopback);
    }
    if address.is_unique_local() {
        return Some(UnusableIpClass::Private);
    }
    if address.is_unicast_link_local() {
        return Some(UnusableIpClass::LinkLocal);
    }
    if address.is_multicast() {
        return Some(UnusableIpClass::Multicast);
    }
    let segments = address.segments();
    if segments[0] == 0x2001 && segments[1] == 0x0db8 {
        return Some(UnusableIpClass::ReservedDocumentation);
    }
    None
}

fn in_range(bits: u32, start: Ipv4Addr, end: Ipv4Addr) -> bool {
    let start = u32::from(start);
    let end = u32::from(end);
    start <= bits && bits <= end
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use super::{UnusableIpClass, canonicalize_ip, classify_unusable_ip, is_known_cimd_url};

    #[test]
    fn canonicalize_maps_v4_mapped_v6_only() {
        let mapped = IpAddr::V6(Ipv6Addr::from([
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 10, 1, 2, 3,
        ]));
        assert_eq!(
            canonicalize_ip(mapped),
            IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3))
        );
        let v6 = IpAddr::V6(Ipv6Addr::LOCALHOST);
        assert_eq!(canonicalize_ip(v6), v6);
        let v4 = IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8));
        assert_eq!(canonicalize_ip(v4), v4);
    }

    #[test]
    fn ipv4_classes_match_the_closed_table() {
        let cases = [
            (
                Ipv4Addr::new(0, 0, 0, 0),
                Some(UnusableIpClass::Unspecified),
            ),
            (
                Ipv4Addr::new(0, 1, 2, 3),
                Some(UnusableIpClass::Unspecified),
            ),
            (Ipv4Addr::new(127, 0, 0, 1), Some(UnusableIpClass::Loopback)),
            (Ipv4Addr::new(10, 0, 0, 1), Some(UnusableIpClass::Private)),
            (Ipv4Addr::new(100, 64, 0, 1), Some(UnusableIpClass::Private)),
            (Ipv4Addr::new(172, 16, 0, 1), Some(UnusableIpClass::Private)),
            (
                Ipv4Addr::new(172, 31, 255, 255),
                Some(UnusableIpClass::Private),
            ),
            (
                Ipv4Addr::new(192, 168, 1, 1),
                Some(UnusableIpClass::Private),
            ),
            (
                Ipv4Addr::new(169, 254, 1, 1),
                Some(UnusableIpClass::LinkLocal),
            ),
            (
                Ipv4Addr::new(224, 0, 0, 1),
                Some(UnusableIpClass::Multicast),
            ),
            (
                Ipv4Addr::new(192, 0, 2, 1),
                Some(UnusableIpClass::ReservedDocumentation),
            ),
            (
                Ipv4Addr::new(198, 51, 100, 1),
                Some(UnusableIpClass::ReservedDocumentation),
            ),
            (
                Ipv4Addr::new(203, 0, 113, 1),
                Some(UnusableIpClass::ReservedDocumentation),
            ),
            (
                Ipv4Addr::new(192, 0, 0, 1),
                Some(UnusableIpClass::ReservedDocumentation),
            ),
            (
                Ipv4Addr::new(192, 88, 99, 1),
                Some(UnusableIpClass::ReservedDocumentation),
            ),
            (
                Ipv4Addr::new(198, 18, 0, 1),
                Some(UnusableIpClass::ReservedDocumentation),
            ),
            (
                Ipv4Addr::new(240, 0, 0, 1),
                Some(UnusableIpClass::ReservedDocumentation),
            ),
            (Ipv4Addr::BROADCAST, Some(UnusableIpClass::Broadcast)),
            (Ipv4Addr::new(8, 8, 8, 8), None),
            (Ipv4Addr::new(172, 32, 0, 1), None),
        ];
        for (address, expected) in cases {
            assert_eq!(
                classify_unusable_ip(IpAddr::V4(address)),
                expected,
                "{address}"
            );
        }
    }

    #[test]
    fn ipv6_classes_and_mapped_v4_are_classified() {
        assert_eq!(
            classify_unusable_ip(IpAddr::V6(Ipv6Addr::UNSPECIFIED)),
            Some(UnusableIpClass::Unspecified)
        );
        assert_eq!(
            classify_unusable_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)),
            Some(UnusableIpClass::Loopback)
        );
        assert_eq!(
            classify_unusable_ip("fc00::1".parse().unwrap()),
            Some(UnusableIpClass::Private)
        );
        assert_eq!(
            classify_unusable_ip("fe80::1".parse().unwrap()),
            Some(UnusableIpClass::LinkLocal)
        );
        assert_eq!(
            classify_unusable_ip("ff02::1".parse().unwrap()),
            Some(UnusableIpClass::Multicast)
        );
        assert_eq!(
            classify_unusable_ip("2001:db8::1".parse().unwrap()),
            Some(UnusableIpClass::ReservedDocumentation)
        );
        assert_eq!(
            classify_unusable_ip("2001:4860:4860::8888".parse().unwrap()),
            None
        );
        assert_eq!(
            classify_unusable_ip("::ffff:10.0.0.1".parse().unwrap()),
            Some(UnusableIpClass::Private)
        );
        assert_eq!(
            classify_unusable_ip("::ffff:8.8.8.8".parse().unwrap()),
            None
        );
    }

    #[test]
    fn known_client_table_is_empty_and_exact() {
        assert!(!is_known_cimd_url("https://claude.ai/.well-known/mcp.json"));
        assert!(!is_known_cimd_url(""));
        assert!(super::KNOWN_CLIENT_CIMD_URLS.is_empty());
    }
}
