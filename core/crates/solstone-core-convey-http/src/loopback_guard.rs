// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure loopback request guard logic.

use axum::http::{HeaderMap, HeaderName, Method, Uri, header};

pub const SEC_FETCH_SITE: HeaderName = HeaderName::from_static("sec-fetch-site");

/// Three-way refusal for loopback guard evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopbackRefusal {
    HostNotAllowed,
    CrossSite,
    CrossOrigin,
}

/// Pure evaluation of loopback request host, origin, and sec-fetch-site headers.
pub fn evaluate_loopback_request(
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
) -> Option<LoopbackRefusal> {
    if !host_allowed(uri, headers) {
        return Some(LoopbackRefusal::HostNotAllowed);
    }
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
    for value in headers.get_all(header::ORIGIN) {
        if !value.to_str().is_ok_and(loopback_origin) {
            return Some(LoopbackRefusal::CrossOrigin);
        }
    }
    None
}

/// A request must name a host, and every name it carries must be loopback.
pub fn host_allowed(uri: &Uri, headers: &HeaderMap) -> bool {
    let host_count = headers.get_all(header::HOST).iter().count();
    if host_count > 1 {
        return false;
    }
    let mut named = false;
    if let Some(authority) = uri.authority() {
        if !loopback_authority(authority.as_str()) {
            return false;
        }
        named = true;
    }
    for value in headers.get_all(header::HOST) {
        if !value.to_str().is_ok_and(loopback_authority) {
            return false;
        }
        named = true;
    }
    named
}

pub fn loopback_origin(value: &str) -> bool {
    value
        .strip_prefix("http://")
        .or_else(|| value.strip_prefix("https://"))
        .is_some_and(loopback_authority)
}

/// `host[:port]` where the host is exactly a loopback name. Anything else,
/// including user-info, a path, an empty port or an unbracketed IPv6 literal,
/// is not one.
pub fn loopback_authority(value: &str) -> bool {
    let bracketed = value.starts_with('[');
    let (host, port) = if bracketed {
        let Some((literal, tail)) = value[1..].split_once(']') else {
            return false;
        };
        match tail {
            "" => (literal, None),
            _ => match tail.strip_prefix(':') {
                Some(port) => (literal, Some(port)),
                None => return false,
            },
        }
    } else {
        match value.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (value, None),
        }
    };
    let name_allowed = if bracketed {
        host == "::1"
    } else {
        host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1"
    };
    name_allowed && port.is_none_or(valid_port)
}

pub fn valid_port(port: &str) -> bool {
    port.bytes().all(|byte| byte.is_ascii_digit()) && port.parse::<u16>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_guard_pure_decision() {
        let uri: Uri = "/test".parse().unwrap();
        let mut headers = HeaderMap::new();
        assert_eq!(
            evaluate_loopback_request(&Method::GET, &uri, &headers),
            Some(LoopbackRefusal::HostNotAllowed)
        );

        headers.insert(header::HOST, "localhost:8080".parse().unwrap());
        assert_eq!(
            evaluate_loopback_request(&Method::GET, &uri, &headers),
            None
        );

        headers.insert(header::ORIGIN, "https://evil.example".parse().unwrap());
        assert_eq!(
            evaluate_loopback_request(&Method::GET, &uri, &headers),
            None
        );
        assert_eq!(
            evaluate_loopback_request(&Method::POST, &uri, &headers),
            Some(LoopbackRefusal::CrossOrigin)
        );
    }
}
