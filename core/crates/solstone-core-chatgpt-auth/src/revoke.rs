// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Token revocation for ChatGPT OAuth tokens.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::transport::{ChatGptTransport, HttpResponse, TransportError};

pub const REVOKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Revocation endpoint path, as published by OpenAI's discovery document.
pub const REVOKE_PATH: &str = "/api/accounts/oauth/revoke";

pub fn revoke_refresh_token(
    transport: &dyn ChatGptTransport,
    auth_base: &str,
    refresh_token: &str,
    client_id: &str,
) -> bool {
    let revoke_url = format!("{auth_base}{REVOKE_PATH}");
    let mut form = BTreeMap::new();
    form.insert("token".to_string(), refresh_token.to_string());
    form.insert("token_type_hint".to_string(), "refresh_token".to_string());
    form.insert("client_id".to_string(), client_id.to_string());

    let mut attempts = 0;
    loop {
        attempts += 1;
        match transport.post_form(&revoke_url, &form, REVOKE_TIMEOUT) {
            Ok(HttpResponse { status, .. }) if (200..=299).contains(&status) => return true,
            Ok(HttpResponse { status, .. }) if (500..=599).contains(&status) && attempts < 2 => {
                continue;
            }
            Ok(_) => return false,
            Err(TransportError::Network | TransportError::Timeout) if attempts < 2 => {
                continue;
            }
            Err(_) => return false,
        }
    }
}
