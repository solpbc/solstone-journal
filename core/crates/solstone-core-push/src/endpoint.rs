// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Web Push endpoint URL normalization and target extraction.

use std::net::Ipv6Addr;
use std::str::FromStr;

/// Check if an endpoint string meets stored shape requirements without URI/crypto parsing.
pub(crate) fn endpoint_stored_shape_ok(endpoint: &str) -> bool {
    if !endpoint.starts_with("https://") {
        return false;
    }
    let len = endpoint.len();
    if !(1..=2048).contains(&len) {
        return false;
    }
    endpoint.bytes().all(|b| {
        (0x21..=0x7E).contains(&b)
            && !matches!(
                b,
                b'"' | b'<' | b'>' | b'\\' | b'^' | b'`' | b'{' | b'|' | b'}' | b'#' | b'@'
            )
    })
}

/// Parse and validate an endpoint URL according to strict push target rules.
/// Returns `Some((host, port))` on success, where `port` is `Some` only when an explicit
/// non-443 port was specified.
pub(crate) fn endpoint_target(endpoint: &str) -> Option<(String, Option<u16>)> {
    if !endpoint_stored_shape_ok(endpoint) {
        return None;
    }

    let uri: http::Uri = endpoint.parse().ok()?;
    if uri.scheme_str() != Some("https") {
        return None;
    }

    let authority = uri.authority()?.as_str();

    // Check host and port from authority string
    let (host_str, port_opt) = if authority.starts_with('[') {
        // Bracketed IPv6 host
        let closing_bracket = authority.find(']')?;
        let inner_ipv6 = &authority[1..closing_bracket];
        if inner_ipv6.contains('%') {
            return None; // No zone identifier allowed
        }
        Ipv6Addr::from_str(inner_ipv6).ok()?;
        let host_formatted = format!("[{}]", inner_ipv6.to_ascii_lowercase());

        let rest = &authority[closing_bracket + 1..];
        let port = if rest.is_empty() {
            None
        } else {
            let port_digits = rest.strip_prefix(':')?;
            let port_val = parse_port_digits(port_digits)?;
            if port_val == 443 {
                None
            } else {
                Some(port_val)
            }
        };
        (host_formatted, port)
    } else {
        // Non-bracketed host (IPv4 or DNS name)
        let (raw_host, raw_port) = match authority.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (authority, None),
        };

        if raw_host.is_empty() {
            return None;
        }

        let is_ipv4 = is_dotted_ipv4(raw_host);
        let is_dns = is_valid_dns_name(raw_host);

        if !is_ipv4 && !is_dns {
            return None;
        }

        let host_formatted = raw_host.to_ascii_lowercase();

        let port = match raw_port {
            Some(port_digits) => {
                let port_val = parse_port_digits(port_digits)?;
                if port_val == 443 {
                    None
                } else {
                    Some(port_val)
                }
            }
            None => None,
        };

        (host_formatted, port)
    };

    Some((host_str, port_opt))
}

fn parse_port_digits(s: &str) -> Option<u16> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let val: u64 = s.parse().ok()?;
    if (1..=65535).contains(&val) {
        Some(val as u16)
    } else {
        None
    }
}

fn is_dotted_ipv4(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 4 {
        return false;
    }
    for part in parts {
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        let num: u16 = part.parse().ok().unwrap_or(300);
        if num > 255 {
            return false;
        }
    }
    true
}

fn is_valid_dns_name(s: &str) -> bool {
    let labels: Vec<&str> = s.split('.').collect();
    for label in labels {
        let len = label.len();
        if !(1..=63).contains(&len) {
            return false;
        }
        if label.starts_with('-') || label.ends_with('-') {
            return false;
        }
        if !label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_target_parsing_and_normalization() {
        assert_eq!(
            endpoint_target("https://[FE80::1]:8443/x"),
            Some(("[fe80::1]".to_owned(), Some(8443)))
        );
        assert_eq!(
            endpoint_target("https://fcm.example:443/x"),
            Some(("fcm.example".to_owned(), None))
        );
        assert_eq!(
            endpoint_target("https://fcm.example/x"),
            Some(("fcm.example".to_owned(), None))
        );
        assert_eq!(
            endpoint_target("https://FCM.Example/a"),
            Some(("fcm.example".to_owned(), None))
        );
        assert_eq!(
            endpoint_target("https://h:0443/x"),
            Some(("h".to_owned(), None))
        );
        assert_eq!(
            endpoint_target("https://h:08443/x"),
            Some(("h".to_owned(), Some(8443)))
        );
        assert_eq!(
            endpoint_target("https://192.168.1.1:8080/push"),
            Some(("192.168.1.1".to_owned(), Some(8080)))
        );

        // Failures
        assert_eq!(endpoint_target("http://fcm.example/x"), None);
        assert_eq!(endpoint_target("https://[::1/x"), None);
        assert_eq!(endpoint_target("https://:443/x"), None);
        assert_eq!(endpoint_target("https://h:99999/x"), None);
        assert_eq!(endpoint_target("https://h:0/x"), None);
        assert_eq!(endpoint_target("https://h:abc/x"), None);
        assert_eq!(endpoint_target("https://ex!ample/x"), None);
        assert_eq!(endpoint_target("https://a_b.example/x"), None);
        assert_eq!(endpoint_target("https://h/a<b"), None);
        assert_eq!(endpoint_target("https://h/a>b"), None);
        assert_eq!(endpoint_target("https://h/a`b"), None);
        assert_eq!(endpoint_target("https://h/a{b"), None);
        assert_eq!(endpoint_target("https://h/a#b"), None);
        assert_eq!(endpoint_target("https://user@h/x"), None);

        // Stored shape vs target distinction
        assert!(endpoint_stored_shape_ok("https://[::1/x"));
        assert!(endpoint_stored_shape_ok("https://:443/x"));
        assert!(endpoint_stored_shape_ok("https://h:99999/x"));
        assert!(endpoint_stored_shape_ok("https://ex!ample/x"));
        assert!(!endpoint_stored_shape_ok("https://h/a<b"));
    }
}
