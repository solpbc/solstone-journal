// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Journal-hosted OAuth helpers for the MCP endpoint.

pub(crate) mod authorize;
pub(crate) mod bulkhead;
pub(crate) mod cimd;
pub(crate) mod dcr;
pub(crate) mod metadata;
pub(crate) mod pairing;
pub(crate) mod rate_limit;
pub(crate) mod redirect;
pub(crate) mod store;
pub(crate) mod token;
pub(crate) mod urlparse;

use std::path::Path;
use std::sync::Arc;

use crate::http1::{HttpRequest, HttpResponse};

use self::bulkhead::CimdBulkhead;
use self::rate_limit::PairingRateLimiter;
use self::store::OAuthStore;

pub(crate) const DCR_MAX_BODY_BYTES: usize = 16 * 1024;
pub(crate) const AUTHORIZE_MAX_BODY_BYTES: usize = 8 * 1024;
pub(crate) const TOKEN_MAX_BODY_BYTES: usize = 8 * 1024;
pub(crate) const MAX_CLIENT_ID_BYTES: usize = 2048;
pub(crate) const MAX_REDIRECT_URI_BYTES: usize = 2048;
pub(crate) const MAX_CLIENT_NAME_BYTES: usize = 256;
pub(crate) const MAX_REDIRECT_URIS_PER_CLIENT: usize = 16;
pub(crate) const MAX_STATE_BYTES: usize = 1024;
pub(crate) const MAX_CODE_BYTES: usize = 512;
const MAX_URLENCODED_PAIRS: usize = 32;

/// Process-local OAuth helpers bound to one journal root.
pub(crate) struct OAuthRuntime {
    pub(crate) store: OAuthStore,
    pub(crate) pairing_limiter: PairingRateLimiter,
    pub(crate) cimd_bulkhead: Arc<CimdBulkhead>,
    pub(crate) resource_origin: String,
}

impl OAuthRuntime {
    pub(crate) fn new(journal_root: &Path, resource_origin: String) -> Self {
        Self {
            store: OAuthStore::open(journal_root),
            pairing_limiter: PairingRateLimiter::new(),
            cimd_bulkhead: CimdBulkhead::new(),
            resource_origin,
        }
    }
}

/// Refuse a request whose `Content-Encoding` is present and not `identity`.
pub(crate) fn reject_non_identity_encoding(request: &HttpRequest) -> Result<(), HttpResponse> {
    match request.header("content-encoding") {
        Ok(None) => Ok(()),
        Ok(Some(value)) if value.trim().eq_ignore_ascii_case("identity") => Ok(()),
        Ok(_) | Err(_) => Err(HttpResponse::error(
            400,
            "Bad Request",
            "request content encoding is unsupported",
        )),
    }
}

/// Parse `application/x-www-form-urlencoded` pairs. `+` decodes as space.
pub(crate) fn parse_urlencoded_pairs(input: &str) -> Option<Vec<(String, String)>> {
    if input.is_empty() {
        return Some(Vec::new());
    }
    let mut pairs = Vec::new();
    for piece in input.split('&') {
        if pairs.len() >= MAX_URLENCODED_PAIRS {
            return None;
        }
        let (key, value) = match piece.split_once('=') {
            Some((key, value)) => (key, value),
            None => (piece, ""),
        };
        let key = urlparse::percent_decode(&key.replace('+', " "))?;
        let value = urlparse::percent_decode(&value.replace('+', " "))?;
        pairs.push((key, value));
    }
    Some(pairs)
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use super::parse_urlencoded_pairs;
    use super::urlparse::query_value_encode;

    #[test]
    fn urlencoded_pairs_round_trip_and_form_rules() {
        let encoded = format!("state={}&empty=", query_value_encode("a&b=# %"));
        assert_eq!(
            parse_urlencoded_pairs(&encoded),
            Some(vec![
                ("state".to_owned(), "a&b=# %".to_owned()),
                ("empty".to_owned(), String::new()),
            ])
        );
        assert_eq!(
            parse_urlencoded_pairs("a+b=c+d"),
            Some(vec![("a b".to_owned(), "c d".to_owned())])
        );
        assert_eq!(parse_urlencoded_pairs(""), Some(Vec::new()));
        assert_eq!(
            parse_urlencoded_pairs("flag"),
            Some(vec![("flag".to_owned(), String::new())])
        );
        assert_eq!(parse_urlencoded_pairs("a=%ZZ"), None);
        assert_eq!(parse_urlencoded_pairs("a=%80"), None);
        let overflow = (0..33)
            .map(|index| format!("k{index}=v"))
            .collect::<Vec<_>>()
            .join("&");
        assert_eq!(parse_urlencoded_pairs(&overflow), None);
    }
}
