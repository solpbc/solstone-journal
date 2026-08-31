// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Journal-hosted OAuth helpers for the MCP endpoint.

pub(crate) mod bulkhead;
pub(crate) mod cimd;
pub(crate) mod dcr;
pub(crate) mod metadata;
pub(crate) mod pairing;
pub(crate) mod rate_limit;
pub(crate) mod redirect;
pub(crate) mod store;
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
