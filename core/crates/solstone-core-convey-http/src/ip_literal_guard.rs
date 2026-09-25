// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure IP-literal request guard logic for LAN direct access.

use axum::http::{HeaderMap, Method, Uri, header};

use crate::loopback_guard::{LoopbackRefusal, SEC_FETCH_SITE};

/// Pure evaluation of LAN IP-literal request host, origin, and sec-fetch-site headers.
pub fn evaluate_ip_literal_request(
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    port: u16,
) -> Option<LoopbackRefusal> {
    let Some(admitted_host) = admitted_ip_literal_host(uri, headers, port) else {
        return Some(LoopbackRefusal::HostNotAllowed);
    };
    if matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    ) {
        return None;
    }
    for value in headers.get_all(&SEC_FETCH_SITE) {
        if !matches!(value.to_str(), Ok("same-origin" | "same-site" | "none")) {
            return Some(LoopbackRefusal::CrossSite);
        }
    }
    let expected_origin = format!("https://{admitted_host}");
    for value in headers.get_all(header::ORIGIN) {
        let Ok(origin_str) = value.to_str() else {
            return Some(LoopbackRefusal::CrossOrigin);
        };
        if origin_str != expected_origin {
            return Some(LoopbackRefusal::CrossOrigin);
        }
    }
    None
}

/// Check that the request carries exactly one Host header matching an IP literal and `port`,
/// and that any absolute-form URI authority agrees with it. Returns the admitted Host string on success.
fn admitted_ip_literal_host(uri: &Uri, headers: &HeaderMap, port: u16) -> Option<String> {
    let mut host_iter = headers.get_all(header::HOST).iter();
    let host_value = host_iter.next()?;
    if host_iter.next().is_some() {
        return None;
    }
    let host_str = host_value.to_str().ok()?;
    if !ip_literal_authority(host_str, port) {
        return None;
    }
    if let Some(authority) = uri.authority() {
        let authority_str = authority.as_str();
        if !ip_literal_authority(authority_str, port) || authority_str != host_str {
            return None;
        }
    }
    Some(host_str.to_owned())
}

/// `host:port` where host is strictly an IPv4 decimal quad or bracketed IPv6 literal,
/// and port text is exactly `port.to_string()`.
pub fn ip_literal_authority(value: &str, port: u16) -> bool {
    if value.bytes().any(|b| b <= 0x1f || b == 0x7f) {
        return false;
    }
    let expected_port = port.to_string();
    if let Some(inner) = value.strip_prefix('[') {
        let Some((literal, tail)) = inner.split_once(']') else {
            return false;
        };
        let Some(port_str) = tail.strip_prefix(':') else {
            return false;
        };
        if port_str != expected_port {
            return false;
        }
        if literal.contains('%') {
            return false;
        }
        literal.parse::<std::net::Ipv6Addr>().is_ok()
    } else {
        let Some((host, port_str)) = value.split_once(':') else {
            return false;
        };
        if port_str != expected_port {
            return false;
        }
        is_strict_ipv4(host)
    }
}

/// Strictly parses standard dotted-decimal IPv4: four octets (0..=255) with no leading zeros.
fn is_strict_ipv4(host: &str) -> bool {
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() != 4 {
        return false;
    }
    for part in parts {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        if part.len() > 1 && part.starts_with('0') {
            return false;
        }
        if part.parse::<u8>().is_err() {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn ip_literal_guard_cases() {
        let uri: Uri = "/test".parse().unwrap();
        let port = 7660;

        // Missing Host -> HostNotAllowed
        let headers = HeaderMap::new();
        assert_eq!(
            evaluate_ip_literal_request(&Method::GET, &uri, &headers, port),
            Some(LoopbackRefusal::HostNotAllowed)
        );

        // Host: journal.local -> HostNotAllowed
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("journal.local"));
        assert_eq!(
            evaluate_ip_literal_request(&Method::GET, &uri, &headers, port),
            Some(LoopbackRefusal::HostNotAllowed)
        );

        // Host: localhost:<port> -> HostNotAllowed
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("localhost:7660"));
        assert_eq!(
            evaluate_ip_literal_request(&Method::GET, &uri, &headers, port),
            Some(LoopbackRefusal::HostNotAllowed)
        );

        // Two Host headers -> HostNotAllowed
        let mut headers = HeaderMap::new();
        headers.append(header::HOST, HeaderValue::from_static("127.0.0.1:7660"));
        headers.append(header::HOST, HeaderValue::from_static("127.0.0.1:7660"));
        assert_eq!(
            evaluate_ip_literal_request(&Method::GET, &uri, &headers, port),
            Some(LoopbackRefusal::HostNotAllowed)
        );

        // Absolute-form authority different from Host -> HostNotAllowed
        let diff_uri: Uri = "http://10.0.0.1:7660/test".parse().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("127.0.0.1:7660"));
        assert_eq!(
            evaluate_ip_literal_request(&Method::GET, &diff_uri, &headers, port),
            Some(LoopbackRefusal::HostNotAllowed)
        );

        // Host with control byte -> false
        assert!(!ip_literal_authority("127.0.0.1\x01:7660", port));

        // 127.0.0.1 with no port -> HostNotAllowed
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("127.0.0.1"));
        assert_eq!(
            evaluate_ip_literal_request(&Method::GET, &uri, &headers, port),
            Some(LoopbackRefusal::HostNotAllowed)
        );

        // A different port -> HostNotAllowed
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("127.0.0.1:7659"));
        assert_eq!(
            evaluate_ip_literal_request(&Method::GET, &uri, &headers, port),
            Some(LoopbackRefusal::HostNotAllowed)
        );

        // 127.0.0.1:0<port> (leading zero in port) -> HostNotAllowed
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("127.0.0.1:07660"));
        assert_eq!(
            evaluate_ip_literal_request(&Method::GET, &uri, &headers, port),
            Some(LoopbackRefusal::HostNotAllowed)
        );

        // 127.1:<port> shorthand -> HostNotAllowed
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("127.1:7660"));
        assert_eq!(
            evaluate_ip_literal_request(&Method::GET, &uri, &headers, port),
            Some(LoopbackRefusal::HostNotAllowed)
        );

        // 0x7f.0.0.1:<port> hex -> HostNotAllowed
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("0x7f.0.0.1:7660"));
        assert_eq!(
            evaluate_ip_literal_request(&Method::GET, &uri, &headers, port),
            Some(LoopbackRefusal::HostNotAllowed)
        );

        // 127.000.000.001:<port> octal/leading zeros -> HostNotAllowed
        let mut headers = HeaderMap::new();
        headers.insert(
            header::HOST,
            HeaderValue::from_static("127.000.000.001:7660"),
        );
        assert_eq!(
            evaluate_ip_literal_request(&Method::GET, &uri, &headers, port),
            Some(LoopbackRefusal::HostNotAllowed)
        );

        // [fe80::1%25en0]:<port> zone id -> HostNotAllowed
        let mut headers = HeaderMap::new();
        headers.insert(
            header::HOST,
            HeaderValue::from_static("[fe80::1%25en0]:7660"),
        );
        assert_eq!(
            evaluate_ip_literal_request(&Method::GET, &uri, &headers, port),
            Some(LoopbackRefusal::HostNotAllowed)
        );

        // Valid IPv4 and IPv6 admitted for GET
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("192.168.1.100:7660"));
        assert_eq!(
            evaluate_ip_literal_request(&Method::GET, &uri, &headers, port),
            None
        );

        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("[::1]:7660"));
        assert_eq!(
            evaluate_ip_literal_request(&Method::GET, &uri, &headers, port),
            None
        );

        // POST /token with Origin: https://evil.example -> 403 cross_origin_blocked
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("127.0.0.1:7660"));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://evil.example"),
        );
        assert_eq!(
            evaluate_ip_literal_request(&Method::POST, &uri, &headers, port),
            Some(LoopbackRefusal::CrossOrigin)
        );

        // POST /token with Origin: http://127.0.0.1:<port> (http scheme) -> 403 cross_origin_blocked
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("127.0.0.1:7660"));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://127.0.0.1:7660"),
        );
        assert_eq!(
            evaluate_ip_literal_request(&Method::POST, &uri, &headers, port),
            Some(LoopbackRefusal::CrossOrigin)
        );

        // POST /authorize with Sec-Fetch-Site: cross-site -> 403 cross_site
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("127.0.0.1:7660"));
        headers.insert(SEC_FETCH_SITE, HeaderValue::from_static("cross-site"));
        assert_eq!(
            evaluate_ip_literal_request(&Method::POST, &uri, &headers, port),
            Some(LoopbackRefusal::CrossSite)
        );

        // POST /token with neither header is admitted
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("127.0.0.1:7660"));
        assert_eq!(
            evaluate_ip_literal_request(&Method::POST, &uri, &headers, port),
            None
        );

        // Origin: https://127.0.0.1:<port> matching Host is admitted
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("127.0.0.1:7660"));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://127.0.0.1:7660"),
        );
        assert_eq!(
            evaluate_ip_literal_request(&Method::POST, &uri, &headers, port),
            None
        );
    }
}
