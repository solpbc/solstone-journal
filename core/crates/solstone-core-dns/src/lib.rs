// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! DNS lookups through the computer's own configured resolvers: the
//! nameservers in `/etc/resolv.conf` on Unix and the network adapters' DNS
//! servers on Windows, with no public fallback and no search suffix. Every
//! query is a single bounded question to those servers.

use std::time::Duration;

use hickory_resolver::config::{NameServerConfig, Protocol};
pub use hickory_resolver::config::{ResolverConfig, ResolverOpts};
use hickory_resolver::error::ResolveErrorKind;

/// The resolver configuration this computer uses.
#[cfg(not(windows))]
pub fn system_resolver_config() -> Result<(ResolverConfig, ResolverOpts), String> {
    let content = std::fs::read_to_string("/etc/resolv.conf").map_err(|e| e.to_string())?;
    let nameservers = content.lines().filter_map(|line| {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.starts_with(';') {
            return None;
        }
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        (parts.len() >= 2 && parts[0] == "nameserver")
            .then(|| parts[1].parse::<std::net::IpAddr>().ok())
            .flatten()
    });
    resolver_config_from_nameservers(nameservers)
        .ok_or_else(|| "no nameservers found in /etc/resolv.conf".to_string())
}

/// The resolver configuration this computer uses.
#[cfg(windows)]
pub fn system_resolver_config() -> Result<(ResolverConfig, ResolverOpts), String> {
    let adapters = ipconfig::get_adapters().map_err(|e| e.to_string())?;
    resolver_config_from_nameservers(
        adapters
            .iter()
            .flat_map(|adapter| adapter.dns_servers().iter().copied()),
    )
    .ok_or_else(|| "no nameservers found in Windows adapter configuration".to_string())
}

/// A configuration that asks exactly these nameservers, over UDP port 53.
pub fn resolver_config_from_nameservers(
    nameservers: impl IntoIterator<Item = std::net::IpAddr>,
) -> Option<(ResolverConfig, ResolverOpts)> {
    let mut config = ResolverConfig::new();
    for ip in nameservers {
        config.add_name_server(NameServerConfig {
            socket_addr: std::net::SocketAddr::new(ip, 53),
            protocol: Protocol::Udp,
            tls_dns_name: None,
            trust_negative_responses: true,
            bind_addr: None,
        });
    }
    if config.name_servers().is_empty() {
        return None;
    }
    Some((config, ResolverOpts::default()))
}

/// Why a TXT lookup gave no answer. A name with no TXT records is not an
/// error: it returns an empty list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxtLookupError {
    NoResolver(String),
    Failed,
}

/// Look up `name`'s TXT records through the system resolvers, blocking for at
/// most `timeout`. Each record's character strings are joined, as RFC 7208
/// reads a split record.
pub fn lookup_txt(name: &str, timeout: Duration) -> Result<Vec<String>, TxtLookupError> {
    let (config, mut opts) = system_resolver_config().map_err(TxtLookupError::NoResolver)?;
    opts.timeout = timeout;
    opts.attempts = 1;
    let resolver =
        hickory_resolver::Resolver::new(config, opts).map_err(|_| TxtLookupError::Failed)?;
    let fqdn = if name.ends_with('.') {
        name.to_owned()
    } else {
        format!("{name}.")
    };
    match resolver.txt_lookup(fqdn) {
        Ok(lookup) => Ok(lookup
            .iter()
            .map(|txt| {
                txt.txt_data()
                    .iter()
                    .map(|part| String::from_utf8_lossy(part))
                    .collect::<String>()
            })
            .collect()),
        Err(err) if matches!(err.kind(), ResolveErrorKind::NoRecordsFound { .. }) => Ok(Vec::new()),
        Err(_) => Err(TxtLookupError::Failed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_nameservers_only_no_public_fallback_or_search_suffix() {
        let ips = [
            "192.0.2.53".parse().unwrap(),
            "2001:db8::53".parse().unwrap(),
        ];
        let (config, _) = resolver_config_from_nameservers(ips).unwrap();
        let servers = config.name_servers();
        assert_eq!(servers.len(), ips.len());
        for (server, ip) in servers.iter().zip(ips) {
            assert_eq!(server.socket_addr, std::net::SocketAddr::new(ip, 53));
            assert_eq!(server.protocol, Protocol::Udp);
        }
        assert!(config.domain().is_none());
        assert!(config.search().is_empty());
        assert!(resolver_config_from_nameservers([]).is_none());
    }

    #[cfg(all(windows, feature = "full-tests"))]
    #[test]
    fn windows_system_resolver_uses_actual_adapter_dns_servers() {
        // Exercise the production loader. This catches a Unix-file read and a
        // substituted public resolver on Windows.
        let expected: Vec<_> = ipconfig::get_adapters()
            .unwrap()
            .iter()
            .flat_map(|adapter| adapter.dns_servers().iter().copied())
            .collect();
        assert!(
            !expected.is_empty(),
            "native host needs configured DNS servers"
        );
        let (config, _) = system_resolver_config().unwrap();
        let actual: Vec<_> = config
            .name_servers()
            .iter()
            .map(|server| server.socket_addr.ip())
            .collect();
        assert_eq!(actual, expected);
        assert!(config.domain().is_none());
        assert!(config.search().is_empty());
    }
}
