// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure request guard logic for BYO owner-hostname MCP ingress.

use axum::http::{HeaderMap, HeaderName, Method, Uri, header};

use crate::loopback_guard::{LoopbackRefusal, SEC_FETCH_SITE};

pub static FORWARDED: HeaderName = HeaderName::from_static("forwarded");
pub static X_FORWARDED_FOR: HeaderName = HeaderName::from_static("x-forwarded-for");
pub static X_FORWARDED_HOST: HeaderName = HeaderName::from_static("x-forwarded-host");
pub static X_FORWARDED_PROTO: HeaderName = HeaderName::from_static("x-forwarded-proto");
pub static X_REAL_IP: HeaderName = HeaderName::from_static("x-real-ip");

/// Pure evaluation of BYO hostname request host, origin, sec-fetch-site, and proxy headers.
pub fn evaluate_byo_hostname_request(
    _method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    canonical_hostname: &str,
) -> Option<LoopbackRefusal> {
    // 1. Rejects proxy forwarding headers
    if headers.contains_key(&FORWARDED)
        || headers.contains_key(&X_FORWARDED_FOR)
        || headers.contains_key(&X_FORWARDED_HOST)
        || headers.contains_key(&X_FORWARDED_PROTO)
        || headers.contains_key(&X_REAL_IP)
    {
        return Some(LoopbackRefusal::HostNotAllowed);
    }

    // 2. Validate Host header and URI authority
    if !host_matches_canonical(uri, headers, canonical_hostname) {
        return Some(LoopbackRefusal::HostNotAllowed);
    }

    // 3. Check Sec-Fetch-Site and Origin on ALL methods when present
    for value in headers.get_all(&SEC_FETCH_SITE) {
        if !matches!(value.to_str(), Ok("same-origin" | "same-site" | "none")) {
            return Some(LoopbackRefusal::CrossSite);
        }
    }

    let expected_origin_default = format!("https://{canonical_hostname}");
    let expected_origin_443 = format!("https://{canonical_hostname}:443");

    for value in headers.get_all(header::ORIGIN) {
        let Ok(origin_str) = value.to_str() else {
            return Some(LoopbackRefusal::CrossOrigin);
        };
        if origin_str != expected_origin_default && origin_str != expected_origin_443 {
            return Some(LoopbackRefusal::CrossOrigin);
        }
    }

    None
}

fn host_matches_canonical(uri: &Uri, headers: &HeaderMap, canonical: &str) -> bool {
    let mut host_iter = headers.get_all(header::HOST).iter();
    let Some(host_val) = host_iter.next() else {
        return false;
    };
    if host_iter.next().is_some() {
        return false;
    }
    let Ok(host_str) = host_val.to_str() else {
        return false;
    };

    if !authority_matches_canonical(host_str, canonical) {
        return false;
    }

    if let Some(authority) = uri.authority() {
        let authority_str = authority.as_str();
        if !authority_matches_canonical(authority_str, canonical) || authority_str != host_str {
            return false;
        }
    }

    true
}

fn authority_matches_canonical(authority: &str, canonical: &str) -> bool {
    if authority == canonical {
        return true;
    }
    if let Some(host) = authority.strip_suffix(":443") {
        return host == canonical;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    const CANONICAL: &str = "mcp.example.com";

    #[test]
    fn evaluate_byo_guard_cases() {
        let uri: Uri = "/mcp".parse().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static(CANONICAL));

        // Valid GET
        assert_eq!(
            evaluate_byo_hostname_request(&Method::GET, &uri, &headers, CANONICAL),
            None
        );

        // Valid POST with matching Origin and Sec-Fetch-Site
        let mut post_headers = headers.clone();
        post_headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://mcp.example.com"),
        );
        post_headers.insert(&SEC_FETCH_SITE, HeaderValue::from_static("same-origin"));
        assert_eq!(
            evaluate_byo_hostname_request(&Method::POST, &uri, &post_headers, CANONICAL),
            None
        );

        // Port 443 permitted in Host and Origin
        let mut port443_headers = HeaderMap::new();
        port443_headers.insert(
            header::HOST,
            HeaderValue::from_static("mcp.example.com:443"),
        );
        port443_headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://mcp.example.com:443"),
        );
        assert_eq!(
            evaluate_byo_hostname_request(&Method::POST, &uri, &port443_headers, CANONICAL),
            None
        );

        // Wrong host
        let mut wrong_host = headers.clone();
        wrong_host.insert(header::HOST, HeaderValue::from_static("other.example.com"));
        assert_eq!(
            evaluate_byo_hostname_request(&Method::GET, &uri, &wrong_host, CANONICAL),
            Some(LoopbackRefusal::HostNotAllowed)
        );

        // Duplicate Host
        let mut dup_host = headers.clone();
        dup_host.append(header::HOST, HeaderValue::from_static(CANONICAL));
        assert_eq!(
            evaluate_byo_hostname_request(&Method::GET, &uri, &dup_host, CANONICAL),
            Some(LoopbackRefusal::HostNotAllowed)
        );

        // Forwarded headers rejected
        for fwd_header in [
            &FORWARDED,
            &X_FORWARDED_FOR,
            &X_FORWARDED_HOST,
            &X_FORWARDED_PROTO,
            &X_REAL_IP,
        ] {
            let mut fwd_map = headers.clone();
            fwd_map.insert(fwd_header, HeaderValue::from_static("1.2.3.4"));
            assert_eq!(
                evaluate_byo_hostname_request(&Method::GET, &uri, &fwd_map, CANONICAL),
                Some(LoopbackRefusal::HostNotAllowed)
            );
        }

        // Cross-origin rejected on POST and GET
        let mut cross_origin = headers.clone();
        cross_origin.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://evil.example.com"),
        );
        assert_eq!(
            evaluate_byo_hostname_request(&Method::POST, &uri, &cross_origin, CANONICAL),
            Some(LoopbackRefusal::CrossOrigin)
        );
        assert_eq!(
            evaluate_byo_hostname_request(&Method::GET, &uri, &cross_origin, CANONICAL),
            Some(LoopbackRefusal::CrossOrigin)
        );

        // Cross-site rejected on POST and GET
        let mut cross_site = headers.clone();
        cross_site.insert(&SEC_FETCH_SITE, HeaderValue::from_static("cross-site"));
        assert_eq!(
            evaluate_byo_hostname_request(&Method::POST, &uri, &cross_site, CANONICAL),
            Some(LoopbackRefusal::CrossSite)
        );
        assert_eq!(
            evaluate_byo_hostname_request(&Method::GET, &uri, &cross_site, CANONICAL),
            Some(LoopbackRefusal::CrossSite)
        );
    }
}
