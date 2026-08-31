// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Fail-closed HTTPS URL parsing and query-value encoding.

const MAX_CIMD_URL_BYTES: usize = 2048;

/// A CIMD document URL that passed the HTTPS-only gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedHttpsUrl {
    pub(crate) host: String,
    pub(crate) path: String,
    pub(crate) query: Option<String>,
}

/// Why a CIMD URL was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CimdUrlError;

/// Parse and admit one HTTPS CIMD document URL.
pub(crate) fn validate_cimd_url(raw: &str) -> Result<ParsedHttpsUrl, CimdUrlError> {
    if raw.is_empty() || raw.len() > MAX_CIMD_URL_BYTES {
        return Err(CimdUrlError);
    }
    if raw.contains('#') || raw.bytes().any(|byte| byte == 0) {
        return Err(CimdUrlError);
    }
    let rest = raw.strip_prefix("https://").ok_or(CimdUrlError)?;
    if rest.contains('@') {
        return Err(CimdUrlError);
    }
    let (authority, path_query) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    if authority.is_empty() {
        return Err(CimdUrlError);
    }
    let (host, port) = split_host_port(authority).ok_or(CimdUrlError)?;
    if !is_dns_hostname(host) {
        return Err(CimdUrlError);
    }
    if port.is_some_and(|port| port != 443) {
        return Err(CimdUrlError);
    }
    let (path, query) = split_path_query(path_query).ok_or(CimdUrlError)?;
    Ok(ParsedHttpsUrl {
        host: host.to_owned(),
        path,
        query,
    })
}

fn split_host_port(authority: &str) -> Option<(&str, Option<u16>)> {
    if authority.starts_with('[') {
        return None;
    }
    match authority.rsplit_once(':') {
        Some((host, port))
            if !host.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            if port.is_empty() {
                return None;
            }
            let port = port.parse::<u16>().ok()?;
            Some((host, Some(port)))
        }
        Some(_) => None,
        None => Some((authority, None)),
    }
}

fn is_dns_hostname(host: &str) -> bool {
    if host.is_empty() || host.starts_with('.') || host.ends_with('.') {
        return false;
    }
    if host.parse::<std::net::Ipv4Addr>().is_ok() {
        return false;
    }
    if !host.bytes().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'.' || byte == b'-'
    }) {
        return false;
    }
    host.split('.').all(|label| {
        !label.is_empty()
            && label
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    })
}

fn split_path_query(path_query: &str) -> Option<(String, Option<String>)> {
    if !path_query.starts_with('/') {
        return None;
    }
    match path_query.split_once('?') {
        Some((path, query)) => Some((path.to_owned(), Some(query.to_owned()))),
        None => Some((path_query.to_owned(), None)),
    }
}

/// Percent-encode one query value (RFC 3986 unreserved set passes through).
pub(crate) fn query_value_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if is_unreserved(byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(nibble(byte >> 4));
            encoded.push(nibble(byte & 0x0f));
        }
    }
    encoded
}

fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

fn nibble(value: u8) -> char {
    char::from(if value < 10 {
        b'0' + value
    } else {
        b'A' + (value - 10)
    })
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use super::{query_value_encode, validate_cimd_url};

    #[test]
    fn cimd_url_accepts_https_dns_hosts() {
        let parsed = validate_cimd_url("https://claude.ai/.well-known/mcp.json").unwrap();
        assert_eq!(parsed.host, "claude.ai");
        assert_eq!(parsed.path, "/.well-known/mcp.json");
        assert_eq!(parsed.query, None);

        let parsed = validate_cimd_url("https://example.com:443/client?x=1").unwrap();
        assert_eq!(parsed.host, "example.com");
        assert_eq!(parsed.path, "/client");
        assert_eq!(parsed.query.as_deref(), Some("x=1"));

        assert!(validate_cimd_url("https://a.b-c.example").is_ok());
    }

    #[test]
    fn cimd_url_rejects_the_closed_forbidden_shapes() {
        for raw in [
            "",
            "http://example.com/client",
            "https://user@example.com/client",
            "https://example.com/client#frag",
            "https://127.0.0.1/client",
            "https://[::1]/client",
            "https://example.com:80/client",
            "https://Example.com/client",
            "https://example.com./client",
            ".example.com",
            "https://.example.com/client",
            "https://example..com/client",
            "ftp://example.com/client",
            "https://example.com:65536/client",
            "https://",
            &format!("https://example.com/{}", "x".repeat(2048)),
        ] {
            assert!(
                validate_cimd_url(raw).is_err(),
                "expected reject for {raw:?}"
            );
        }
    }

    #[test]
    fn query_value_encode_leaves_unreserved_and_encodes_the_rest() {
        assert_eq!(query_value_encode("Abc-._~012"), "Abc-._~012");
        assert_eq!(query_value_encode("&"), "%26");
        assert_eq!(query_value_encode("#"), "%23");
        assert_eq!(query_value_encode("%"), "%25");
        assert_eq!(query_value_encode("="), "%3D");
        assert_eq!(query_value_encode("+"), "%2B");
        assert_eq!(query_value_encode(" "), "%20");
        assert_eq!(query_value_encode("\u{0001}"), "%01");
        assert_eq!(query_value_encode("a&b=c#d"), "a%26b%3Dc%23d");
    }
}
