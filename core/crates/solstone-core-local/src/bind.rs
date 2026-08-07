// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An exact loopback bind address for a supervisor-owned local listener.
///
/// Same-UID local reachability is an accepted trust boundary: a process that
/// can reach this listener can already access the journal configuration and
/// equivalent local credentials. The protections that matter are an exact
/// loopback bind, an ephemeral port assigned by the supervisor, and a server
/// lifetime scoped to its owning process or session so stale or failed servers
/// are torn down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LoopbackAddr(IpAddr);

impl LoopbackAddr {
    pub const IPV4_LOOPBACK: Self = Self(IpAddr::V4(Ipv4Addr::LOCALHOST));
    pub const IPV6_LOOPBACK: Self = Self(IpAddr::V6(Ipv6Addr::LOCALHOST));

    #[must_use]
    pub const fn ip(self) -> IpAddr {
        self.0
    }

    fn from_exact_ip(ip: IpAddr) -> Option<Self> {
        match ip {
            IpAddr::V4(Ipv4Addr::LOCALHOST) => Some(Self::IPV4_LOOPBACK),
            IpAddr::V6(Ipv6Addr::LOCALHOST) => Some(Self::IPV6_LOOPBACK),
            _ => None,
        }
    }
}

impl fmt::Display for LoopbackAddr {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Serialize for LoopbackAddr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for LoopbackAddr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let ip = value.parse::<IpAddr>().map_err(|_| {
            D::Error::custom("loopback bind address must be exactly 127.0.0.1 or ::1")
        })?;
        Self::from_exact_ip(ip).ok_or_else(|| {
            D::Error::custom("loopback bind address must be exactly 127.0.0.1 or ::1")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_exact_loopback_constants() {
        assert_eq!(
            serde_json::from_str::<LoopbackAddr>(r#""127.0.0.1""#)
                .expect("deserialize IPv4 loopback"),
            LoopbackAddr::IPV4_LOOPBACK
        );
        assert_eq!(
            serde_json::from_str::<LoopbackAddr>(r#""::1""#).expect("deserialize IPv6 loopback"),
            LoopbackAddr::IPV6_LOOPBACK
        );
    }

    #[test]
    fn rejects_non_exact_loopback_addresses_and_hostnames() {
        for value in [r#""0.0.0.0""#, r#""::""#, r#""localhost""#] {
            let error = serde_json::from_str::<LoopbackAddr>(value).expect_err("reject value");
            assert!(
                error
                    .to_string()
                    .contains("loopback bind address must be exactly"),
                "{value}: {error}"
            );
        }
    }
}
