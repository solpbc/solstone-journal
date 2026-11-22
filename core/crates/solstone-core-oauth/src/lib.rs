// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared journal OAuth 2.1 client core.
//!
//! Provides hardened loopback callback reception and paste-URL parsing, atomic
//! single-use attempt claiming, `0o600` credential storage with `flock`-serialized
//! rotating refresh and crash-durable renewal sidecars, SSRF-safe HTTPS discovery
//! (RFC 9728 / RFC 8414), public-client registration (CIMD + RFC 7591 DCR),
//! PKCE S256 authorization code exchange with RFC 9207 `iss` and RFC 8707 `resource`
//! checks, optional RFC 8628 Device Code, and optional PAR (RFC 9126) + DPoP ES256
//! (RFC 9449) extensions.

pub mod atproto;
pub mod atproto_client;
pub mod callback;
pub mod client;
pub mod device_code;
pub mod discovery;
pub mod registration;
pub mod ssrf;
pub mod store;

#[cfg(all(test, feature = "full-tests"))]
mod full_tests;

use std::collections::BTreeMap;

pub use solstone_core_auth_flow::{
    BrowserInvocation, BrowserStdio, DecodeError, PlanError, RandomError, TargetOs,
    base64_url_no_pad, code_challenge_s256, current_target_os, execute_browser_invocation,
    percent_decode, percent_encode, plan_browser, random_token,
};

/// Encode key/value pairs as `application/x-www-form-urlencoded` (deterministic key order).
pub fn encode_form(fields: &BTreeMap<String, String>) -> String {
    fields
        .iter()
        .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Extract an OAuth error code from a JSON error body (`{"error": "..."}` or `{"error": {"code": "..."}}`).
pub fn extract_oauth_error_code(body: &[u8]) -> Option<String> {
    let json = serde_json::from_slice::<serde_json::Value>(body).ok()?;
    json.get("error")
        .and_then(|err| {
            if let Some(s) = err.as_str() {
                Some(s.to_owned())
            } else {
                err.as_object()
                    .and_then(|obj| obj.get("code"))
                    .and_then(|code| code.as_str())
                    .map(ToOwned::to_owned)
            }
        })
        .or_else(|| {
            json.get("code")
                .and_then(|code| code.as_str())
                .map(ToOwned::to_owned)
        })
        .filter(|code| !code.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_form_preserves_sorted_keys_and_percent_encoding() {
        let mut fields = BTreeMap::new();
        fields.insert("b".to_owned(), "hello world".to_owned());
        fields.insert("a".to_owned(), "https://example.test/cb?x=1".to_owned());
        assert_eq!(
            encode_form(&fields),
            "a=https%3A%2F%2Fexample.test%2Fcb%3Fx%3D1&b=hello%20world"
        );
    }

    #[test]
    fn extract_oauth_error_code_handles_flat_and_nested_shapes() {
        assert_eq!(
            extract_oauth_error_code(br#"{"error":"invalid_grant"}"#).as_deref(),
            Some("invalid_grant")
        );
        assert_eq!(
            extract_oauth_error_code(br#"{"error":{"code":"use_dpop_nonce"}}"#).as_deref(),
            Some("use_dpop_nonce")
        );
        assert_eq!(
            extract_oauth_error_code(br#"{"code":"refresh_token_expired"}"#).as_deref(),
            Some("refresh_token_expired")
        );
        assert_eq!(extract_oauth_error_code(br#"not json"#), None);
    }
}
