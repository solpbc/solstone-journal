// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Test support and synchronization hooks for ChatGPT auth testing.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use solstone_core_auth_flow::random_token;
use solstone_core_journal_io::AtomicWriteError;

use crate::store::{
    ChatGptSignInDoc, DIRECT_USE_SCOPE, Registration, Tokens, save_credential_file,
};

pub const REFRESH_MARKERS_ENV: &str = "SOLSTONE_CHATGPT_AUTH_REFRESH_MARKERS";

thread_local! {
    static FAIL_NEXT_GRANT_WRITE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub fn set_fail_next_grant_write(fail: bool) {
    FAIL_NEXT_GRANT_WRITE.with(|c| c.set(fail));
}

pub fn take_fail_next_grant_write() -> bool {
    FAIL_NEXT_GRANT_WRITE.with(|c| c.replace(false))
}

pub fn record_refresh_marker_if_configured() {
    let dir = match std::env::var(REFRESH_MARKERS_ENV) {
        Ok(v) => PathBuf::from(v),
        Err(_) => return,
    };

    if !dir.is_dir() {
        return;
    }
    let pid = std::process::id();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let rand = random_token(4).unwrap_or_default();
    let marker_path = dir.join(format!("refresh-{pid}-{now}-{rand}.marker"));
    let _ = fs::write(marker_path, b"marker");
}

pub fn write_test_credential(journal: &Path, signed_in: bool) -> Result<(), AtomicWriteError> {
    let now = crate::store::now_secs();
    let doc = ChatGptSignInDoc {
        version: 1,
        host_id: "solstone-test-host".to_string(),
        epoch: 1,
        sign_in_id: if signed_in {
            Some("test-sign-in-id".to_string())
        } else {
            None
        },
        registration: Some(Registration {
            client_id: "oaiapp_test".to_string(),
            client_refused: false,
        }),
        subject: Some("user-sub-123".to_string()),
        email: Some("user@example.com".to_string()),
        plan_usage_declined: false,
        tokens: if signed_in {
            Some(Tokens {
                access_token: "test-access-token".to_string(),
                refresh_token: "test-refresh-token".to_string(),
                expires_at: now.saturating_add(86400),
                scopes: vec![
                    "openid".to_string(),
                    "profile".to_string(),
                    "email".to_string(),
                    DIRECT_USE_SCOPE.to_string(),
                ],
            })
        } else {
            None
        },
    };
    save_credential_file(journal, &doc)
}
