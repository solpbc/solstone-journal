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
    // The path and query are written directly into an HTTP/1.1 request line
    // after parsing.  Admit only visible ASCII so a client ID cannot smuggle
    // whitespace or a new header into that request.
    if raw.contains('#') || raw.bytes().any(|byte| !byte.is_ascii_graphic()) {
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

/// Decode `%XX` sequences. `+` is left literal. Invalid escapes or UTF-8 yield `None`.
pub(crate) fn percent_decode(value: &str) -> Option<String> {
    let raw = value.as_bytes();
    let mut bytes = Vec::with_capacity(raw.len());
    let mut index = 0;
    while index < raw.len() {
        if raw[index] == b'%' {
            if index + 2 >= raw.len() {
                return None;
            }
            let high = from_hex(raw[index + 1])?;
            let low = from_hex(raw[index + 2])?;
            bytes.push((high << 4) | low);
            index += 3;
        } else {
            bytes.push(raw[index]);
            index += 1;
        }
    }
    String::from_utf8(bytes).ok()
}

fn from_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use super::{percent_decode, query_value_encode, validate_cimd_url};

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
            "https://example.com/client with space",
            "https://example.com/client\r\nHost: poison.example",
            "https://example.com/client\nHost: poison.example",
            "https://example.com/caf\u{00e9}",
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

    #[test]
    fn percent_decode_round_trips_query_value_encode() {
        for value in [
            "Abc-._~012",
            "a&b=c#d",
            "100%",
            "plus+and space",
            "\u{00e9}",
        ] {
            assert_eq!(
                percent_decode(&query_value_encode(value)).as_deref(),
                Some(value)
            );
        }
        assert_eq!(percent_decode("a%2Bb"), Some("a+b".to_owned()));
        assert_eq!(percent_decode("%"), None);
        assert_eq!(percent_decode("%A"), None);
        assert_eq!(percent_decode("%ZZ"), None);
        assert_eq!(percent_decode("%80"), None);
    }
}
