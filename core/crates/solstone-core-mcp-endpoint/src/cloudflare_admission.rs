// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Cloudflare proxied preface source filter.
//!
//! Source ranges copied 2026-09-26 from:
//! - https://www.cloudflare.com/ips-v4
//! - https://www.cloudflare.com/ips-v6
//!
//! Both range lists and the cutoff date (`2026-11-30T06:16:00Z`) are removed after the cutoff.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use chrono::{DateTime, TimeZone, Utc};

struct Ipv4Cidr {
    network: u32,
    mask: u32,
}

impl Ipv4Cidr {
    const fn new(addr: [u8; 4], prefix: u8) -> Self {
        let network = u32::from_be_bytes(addr);
        let mask = if prefix == 0 {
            0
        } else {
            !0u32 << (32 - prefix)
        };
        Self {
            network: network & mask,
            mask,
        }
    }

    const fn contains(&self, ip: Ipv4Addr) -> bool {
        let val = u32::from_be_bytes(ip.octets());
        (val & self.mask) == self.network
    }
}

struct Ipv6Cidr {
    network: u128,
    mask: u128,
}

impl Ipv6Cidr {
    const fn new(addr: [u8; 16], prefix: u8) -> Self {
        let network = u128::from_be_bytes(addr);
        let mask = if prefix == 0 {
            0
        } else {
            !0u128 << (128 - prefix)
        };
        Self {
            network: network & mask,
            mask,
        }
    }

    const fn contains(&self, ip: Ipv6Addr) -> bool {
        let val = u128::from_be_bytes(ip.octets());
        (val & self.mask) == self.network
    }
}

const IPV4_RANGES: &[Ipv4Cidr] = &[
    Ipv4Cidr::new([173, 245, 48, 0], 20),
    Ipv4Cidr::new([103, 21, 244, 0], 22),
    Ipv4Cidr::new([103, 22, 200, 0], 22),
    Ipv4Cidr::new([103, 31, 4, 0], 22),
    Ipv4Cidr::new([141, 101, 64, 0], 18),
    Ipv4Cidr::new([108, 162, 192, 0], 18),
    Ipv4Cidr::new([190, 93, 240, 0], 20),
    Ipv4Cidr::new([188, 114, 96, 0], 20),
    Ipv4Cidr::new([197, 234, 240, 0], 22),
    Ipv4Cidr::new([198, 41, 128, 0], 17),
    Ipv4Cidr::new([162, 158, 0, 0], 15),
    Ipv4Cidr::new([104, 16, 0, 0], 13),
    Ipv4Cidr::new([104, 24, 0, 0], 14),
    Ipv4Cidr::new([172, 64, 0, 0], 13),
    Ipv4Cidr::new([131, 0, 72, 0], 22),
];

const IPV6_RANGES: &[Ipv6Cidr] = &[
    Ipv6Cidr::new(
        [0x24, 0x00, 0xcb, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        32,
    ),
    Ipv6Cidr::new(
        [0x26, 0x06, 0x47, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        32,
    ),
    Ipv6Cidr::new(
        [0x28, 0x03, 0xf8, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        32,
    ),
    Ipv6Cidr::new(
        [0x24, 0x05, 0xb5, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        32,
    ),
    Ipv6Cidr::new(
        [0x24, 0x05, 0x81, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        32,
    ),
    Ipv6Cidr::new(
        [0x2a, 0x06, 0x98, 0xc0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        29,
    ),
    Ipv6Cidr::new(
        [0x2c, 0x0f, 0xf2, 0x48, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        32,
    ),
];

fn cutoff_time() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 30, 6, 16, 0).unwrap()
}

fn extract_embedded_v4(v6: Ipv6Addr) -> Option<Ipv4Addr> {
    let octets = v6.octets();
    // Mapped: ::ffff:a.b.c.d
    if octets[0..10] == [0; 10] && octets[10] == 0xff && octets[11] == 0xff {
        return Some(Ipv4Addr::new(
            octets[12], octets[13], octets[14], octets[15],
        ));
    }
    // Compatible: ::a.b.c.d
    if octets[0..12] == [0; 12] {
        return Some(Ipv4Addr::new(
            octets[12], octets[13], octets[14], octets[15],
        ));
    }
    None
}

/// Refuse PROXY preface sources originating from Cloudflare ranges before the cutoff.
pub(crate) fn cloudflare_preface_refused(source: IpAddr, now: DateTime<Utc>) -> bool {
    if now >= cutoff_time() {
        return false;
    }
    match source {
        IpAddr::V4(v4) => IPV4_RANGES.iter().any(|range| range.contains(v4)),
        IpAddr::V6(v6) => {
            if let Some(embedded_v4) = extract_embedded_v4(v6) {
                IPV4_RANGES.iter().any(|range| range.contains(embedded_v4))
            } else {
                IPV6_RANGES.iter().any(|range| range.contains(v6))
            }
        }
    }
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use chrono::Duration;

    use super::{cloudflare_preface_refused, cutoff_time};

    #[test]
    fn cloudflare_admission_predicate_evaluates_exact_ranges_and_cutoff() {
        let before_cutoff = cutoff_time() - Duration::seconds(1);
        let at_cutoff = cutoff_time();
        let after_cutoff = cutoff_time() + Duration::seconds(1);

        // IPv4 boundary checks (first, last, just-outside)
        // 173.245.48.0/20 -> 173.245.48.0 .. 173.245.63.255
        let first_173 = IpAddr::V4(Ipv4Addr::new(173, 245, 48, 0));
        let last_173 = IpAddr::V4(Ipv4Addr::new(173, 245, 63, 255));
        let before_173 = IpAddr::V4(Ipv4Addr::new(173, 245, 47, 255));
        let after_173 = IpAddr::V4(Ipv4Addr::new(173, 245, 64, 0));

        assert!(cloudflare_preface_refused(first_173, before_cutoff));
        assert!(cloudflare_preface_refused(last_173, before_cutoff));
        assert!(!cloudflare_preface_refused(before_173, before_cutoff));
        assert!(!cloudflare_preface_refused(after_173, before_cutoff));

        // 104.16.0.0/13 -> 104.16.0.0 .. 104.23.255.255
        // 104.24.0.0/14 -> 104.24.0.0 .. 104.27.255.255
        // Adjacent block boundary: 104.15.255.255 (outside), 104.16.0.0 (first), 104.23.255.255 (last of /13),
        // 104.24.0.0 (first of /14), 104.27.255.255 (last of /14), 104.28.0.0 (outside)
        let outside_low_104 = IpAddr::V4(Ipv4Addr::new(104, 15, 255, 255));
        let first_104_16 = IpAddr::V4(Ipv4Addr::new(104, 16, 0, 0));
        let last_104_16 = IpAddr::V4(Ipv4Addr::new(104, 23, 255, 255));
        let first_104_24 = IpAddr::V4(Ipv4Addr::new(104, 24, 0, 0));
        let last_104_24 = IpAddr::V4(Ipv4Addr::new(104, 27, 255, 255));
        let outside_high_104 = IpAddr::V4(Ipv4Addr::new(104, 28, 0, 0));

        assert!(!cloudflare_preface_refused(outside_low_104, before_cutoff));
        assert!(cloudflare_preface_refused(first_104_16, before_cutoff));
        assert!(cloudflare_preface_refused(last_104_16, before_cutoff));
        assert!(cloudflare_preface_refused(first_104_24, before_cutoff));
        assert!(cloudflare_preface_refused(last_104_24, before_cutoff));
        assert!(!cloudflare_preface_refused(outside_high_104, before_cutoff));

        // 103.21.244.0/22 -> 103.21.244.0 .. 103.21.247.255
        assert!(cloudflare_preface_refused(
            IpAddr::V4(Ipv4Addr::new(103, 21, 244, 0)),
            before_cutoff
        ));
        assert!(cloudflare_preface_refused(
            IpAddr::V4(Ipv4Addr::new(103, 21, 247, 255)),
            before_cutoff
        ));
        assert!(!cloudflare_preface_refused(
            IpAddr::V4(Ipv4Addr::new(103, 21, 243, 255)),
            before_cutoff
        ));
        assert!(!cloudflare_preface_refused(
            IpAddr::V4(Ipv4Addr::new(103, 21, 248, 0)),
            before_cutoff
        ));

        // 103.22.200.0/22 -> 103.22.200.0 .. 103.22.203.255
        assert!(cloudflare_preface_refused(
            IpAddr::V4(Ipv4Addr::new(103, 22, 200, 0)),
            before_cutoff
        ));
        assert!(cloudflare_preface_refused(
            IpAddr::V4(Ipv4Addr::new(103, 22, 203, 255)),
            before_cutoff
        ));

        // 103.31.4.0/22 -> 103.31.4.0 .. 103.31.7.255
        assert!(cloudflare_preface_refused(
            IpAddr::V4(Ipv4Addr::new(103, 31, 4, 0)),
            before_cutoff
        ));
        assert!(cloudflare_preface_refused(
            IpAddr::V4(Ipv4Addr::new(103, 31, 7, 255)),
            before_cutoff
        ));

        // 141.101.64.0/18 -> 141.101.64.0 .. 141.101.127.255
        assert!(cloudflare_preface_refused(
            IpAddr::V4(Ipv4Addr::new(141, 101, 64, 0)),
            before_cutoff
        ));
        assert!(cloudflare_preface_refused(
            IpAddr::V4(Ipv4Addr::new(141, 101, 127, 255)),
            before_cutoff
        ));

        // 108.162.192.0/18 -> 108.162.192.0 .. 108.162.255.255
        assert!(cloudflare_preface_refused(
            IpAddr::V4(Ipv4Addr::new(108, 162, 192, 0)),
            before_cutoff
        ));
        assert!(cloudflare_preface_refused(
            IpAddr::V4(Ipv4Addr::new(108, 162, 255, 255)),
            before_cutoff
        ));

        // 190.93.240.0/20 -> 190.93.240.0 .. 190.93.255.255
        assert!(cloudflare_preface_refused(
            IpAddr::V4(Ipv4Addr::new(190, 93, 240, 0)),
            before_cutoff
        ));
        assert!(cloudflare_preface_refused(
            IpAddr::V4(Ipv4Addr::new(190, 93, 255, 255)),
            before_cutoff
        ));

        // 188.114.96.0/20 -> 188.114.96.0 .. 188.114.111.255
        assert!(cloudflare_preface_refused(
            IpAddr::V4(Ipv4Addr::new(188, 114, 96, 0)),
            before_cutoff
        ));
        assert!(cloudflare_preface_refused(
            IpAddr::V4(Ipv4Addr::new(188, 114, 111, 255)),
            before_cutoff
        ));

        // 197.234.240.0/22 -> 197.234.240.0 .. 197.234.243.255
        assert!(cloudflare_preface_refused(
            IpAddr::V4(Ipv4Addr::new(197, 234, 240, 0)),
            before_cutoff
        ));
        assert!(cloudflare_preface_refused(
            IpAddr::V4(Ipv4Addr::new(197, 234, 243, 255)),
            before_cutoff
        ));

        // 198.41.128.0/17 -> 198.41.128.0 .. 198.41.255.255
        assert!(cloudflare_preface_refused(
            IpAddr::V4(Ipv4Addr::new(198, 41, 128, 0)),
            before_cutoff
        ));
        assert!(cloudflare_preface_refused(
            IpAddr::V4(Ipv4Addr::new(198, 41, 255, 255)),
            before_cutoff
        ));

        // 162.158.0.0/15 -> 162.158.0.0 .. 162.159.255.255
        assert!(cloudflare_preface_refused(
            IpAddr::V4(Ipv4Addr::new(162, 158, 0, 0)),
            before_cutoff
        ));
        assert!(cloudflare_preface_refused(
            IpAddr::V4(Ipv4Addr::new(162, 159, 255, 255)),
            before_cutoff
        ));

        // 172.64.0.0/13 -> 172.64.0.0 .. 172.71.255.255
        assert!(cloudflare_preface_refused(
            IpAddr::V4(Ipv4Addr::new(172, 64, 0, 0)),
            before_cutoff
        ));
        assert!(cloudflare_preface_refused(
            IpAddr::V4(Ipv4Addr::new(172, 71, 255, 255)),
            before_cutoff
        ));

        // 131.0.72.0/22 -> 131.0.72.0 .. 131.0.75.255
        assert!(cloudflare_preface_refused(
            IpAddr::V4(Ipv4Addr::new(131, 0, 72, 0)),
            before_cutoff
        ));
        assert!(cloudflare_preface_refused(
            IpAddr::V4(Ipv4Addr::new(131, 0, 75, 255)),
            before_cutoff
        ));

        // IPv6 boundary checks
        // 2400:cb00::/32
        assert!(cloudflare_preface_refused(
            IpAddr::V6("2400:cb00::".parse().unwrap()),
            before_cutoff
        ));
        assert!(cloudflare_preface_refused(
            IpAddr::V6("2400:cb00:ffff:ffff:ffff:ffff:ffff:ffff".parse().unwrap()),
            before_cutoff
        ));
        assert!(!cloudflare_preface_refused(
            IpAddr::V6("2400:caff:ffff:ffff:ffff:ffff:ffff:ffff".parse().unwrap()),
            before_cutoff
        ));
        assert!(!cloudflare_preface_refused(
            IpAddr::V6("2400:cb01::".parse().unwrap()),
            before_cutoff
        ));

        // 2606:4700::/32
        assert!(cloudflare_preface_refused(
            IpAddr::V6("2606:4700::".parse().unwrap()),
            before_cutoff
        ));
        assert!(cloudflare_preface_refused(
            IpAddr::V6("2606:4700:ffff:ffff:ffff:ffff:ffff:ffff".parse().unwrap()),
            before_cutoff
        ));

        // 2803:f800::/32
        assert!(cloudflare_preface_refused(
            IpAddr::V6("2803:f800::".parse().unwrap()),
            before_cutoff
        ));
        assert!(cloudflare_preface_refused(
            IpAddr::V6("2803:f800:ffff:ffff:ffff:ffff:ffff:ffff".parse().unwrap()),
            before_cutoff
        ));

        // 2405:b500::/32
        assert!(cloudflare_preface_refused(
            IpAddr::V6("2405:b500::".parse().unwrap()),
            before_cutoff
        ));
        assert!(cloudflare_preface_refused(
            IpAddr::V6("2405:b500:ffff:ffff:ffff:ffff:ffff:ffff".parse().unwrap()),
            before_cutoff
        ));

        // 2405:8100::/32
        assert!(cloudflare_preface_refused(
            IpAddr::V6("2405:8100::".parse().unwrap()),
            before_cutoff
        ));
        assert!(cloudflare_preface_refused(
            IpAddr::V6("2405:8100:ffff:ffff:ffff:ffff:ffff:ffff".parse().unwrap()),
            before_cutoff
        ));

        // 2a06:98c0::/29 -> 2a06:98c0:: .. 2a06:98c7:ffff:ffff:ffff:ffff:ffff:ffff
        assert!(cloudflare_preface_refused(
            IpAddr::V6("2a06:98c0::".parse().unwrap()),
            before_cutoff
        ));
        assert!(cloudflare_preface_refused(
            IpAddr::V6("2a06:98c7:ffff:ffff:ffff:ffff:ffff:ffff".parse().unwrap()),
            before_cutoff
        ));
        assert!(!cloudflare_preface_refused(
            IpAddr::V6("2a06:98bf:ffff:ffff:ffff:ffff:ffff:ffff".parse().unwrap()),
            before_cutoff
        ));
        assert!(!cloudflare_preface_refused(
            IpAddr::V6("2a06:98c8::".parse().unwrap()),
            before_cutoff
        ));

        // 2c0f:f248::/32
        assert!(cloudflare_preface_refused(
            IpAddr::V6("2c0f:f248::".parse().unwrap()),
            before_cutoff
        ));
        assert!(cloudflare_preface_refused(
            IpAddr::V6("2c0f:f248:ffff:ffff:ffff:ffff:ffff:ffff".parse().unwrap()),
            before_cutoff
        ));

        // Mapped and compatible IPv6 forms of in-range v4
        let mapped_in_range: IpAddr = "::ffff:173.245.48.1".parse().unwrap();
        let compatible_in_range: IpAddr = "::173.245.48.1".parse().unwrap();
        assert!(cloudflare_preface_refused(mapped_in_range, before_cutoff));
        assert!(cloudflare_preface_refused(
            compatible_in_range,
            before_cutoff
        ));

        // ::1 is compatible 0.0.0.1 and not refused
        let loopback_v6: IpAddr = "::1".parse().unwrap();
        assert!(!cloudflare_preface_refused(loopback_v6, before_cutoff));

        // Cutoff timestamp boundary tests
        assert!(!cloudflare_preface_refused(first_173, at_cutoff));
        assert!(!cloudflare_preface_refused(first_173, after_cutoff));
        assert!(!cloudflare_preface_refused(mapped_in_range, at_cutoff));
        assert!(!cloudflare_preface_refused(mapped_in_range, after_cutoff));
    }
}
