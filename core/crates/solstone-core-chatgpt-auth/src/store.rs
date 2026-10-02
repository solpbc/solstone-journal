// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Storage and invariant enforcement for `config/chatgpt-sign-in.json`.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{
    AtomicWriteError, FileLock, JsonWriteOptions, LockError, LockOptions, hold_lock, write_json,
};

pub const CREDENTIAL_FILE: &str = "config/chatgpt-sign-in.json";
pub const DIRECT_USE_SCOPE: &str = "chatgpt.tokens.use.direct";
pub const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(45);

#[cfg(test)]
thread_local! {
    static TEST_ACCESS_TOKEN_LOCK_TIMEOUT: std::cell::Cell<Option<Duration>> = const { std::cell::Cell::new(None) };
    static TEST_NOW_SECS: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub fn with_test_access_token_lock_timeout<T>(timeout: Duration, f: impl FnOnce() -> T) -> T {
    TEST_ACCESS_TOKEN_LOCK_TIMEOUT.with(|cell| {
        let old = cell.replace(Some(timeout));
        let result = f();
        cell.set(old);
        result
    })
}

#[cfg(test)]
pub fn with_test_now_secs<T>(now: u64, f: impl FnOnce() -> T) -> T {
    TEST_NOW_SECS.with(|cell| {
        let old = cell.replace(Some(now));
        let result = f();
        cell.set(old);
        result
    })
}

pub fn now_secs() -> u64 {
    #[cfg(test)]
    if let Some(now) = TEST_NOW_SECS.with(|cell| cell.get()) {
        return now;
    }
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn access_token_lock_options() -> LockOptions {
    #[cfg(test)]
    if let Some(timeout) = TEST_ACCESS_TOKEN_LOCK_TIMEOUT.with(|cell| cell.get()) {
        return LockOptions {
            timeout,
            ..Default::default()
        };
    }
    LockOptions {
        timeout: DEFAULT_LOCK_TIMEOUT,
        ..Default::default()
    }
}

pub(crate) fn credential_lock_options() -> LockOptions {
    LockOptions {
        timeout: DEFAULT_LOCK_TIMEOUT,
        ..Default::default()
    }
}

/// Dynamic registration state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registration {
    pub client_id: String,
    pub client_refused: bool,
}

/// Token payload.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: u64,
    pub scopes: Vec<String>,
}

impl fmt::Debug for Tokens {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Tokens")
            .field("expires_at", &self.expires_at)
            .field("scopes", &self.scopes)
            .finish_non_exhaustive()
    }
}

/// Root credential structure persisted in `config/chatgpt-sign-in.json`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatGptSignInDoc {
    pub version: u32,
    pub host_id: String,
    pub epoch: u64,
    pub sign_in_id: Option<String>,
    pub registration: Option<Registration>,
    pub subject: Option<String>,
    pub email: Option<String>,
    pub plan_usage_declined: bool,
    pub tokens: Option<Tokens>,
}

impl fmt::Debug for ChatGptSignInDoc {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChatGptSignInDoc")
            .field("version", &self.version)
            .field("epoch", &self.epoch)
            .field("has_registration", &self.registration.is_some())
            .field("has_tokens", &self.tokens.is_some())
            .field("plan_usage_declined", &self.plan_usage_declined)
            .finish_non_exhaustive()
    }
}

impl ChatGptSignInDoc {
    pub fn new_initial(host_id: String) -> Self {
        Self {
            version: 1,
            host_id,
            epoch: 1,
            sign_in_id: None,
            registration: None,
            subject: None,
            email: None,
            plan_usage_declined: false,
            tokens: None,
        }
    }

    pub fn validate_invariants(&self) -> bool {
        if self.version != 1 || self.host_id.is_empty() || self.epoch == 0 {
            return false;
        }
        if let Some(tokens) = &self.tokens
            && (self.registration.is_none()
                || tokens.access_token.is_empty()
                || tokens.refresh_token.is_empty()
                || tokens.scopes.is_empty()
                || !tokens.scopes.iter().any(|scope| scope == DIRECT_USE_SCOPE))
        {
            return false;
        }
        true
    }
}

pub fn credential_path(journal: &Path) -> PathBuf {
    journal.join(CREDENTIAL_FILE)
}

pub fn acquire_credential_lock(
    journal: &Path,
    options: LockOptions,
) -> Result<FileLock, LockError> {
    let path = credential_path(journal);
    hold_lock(
        &path,
        LockOptions {
            mode: Some(0o600),
            ..options
        },
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum LoadResult {
    Present(ChatGptSignInDoc),
    Absent,
    Unreadable,
}

pub fn load_credential_file(journal: &Path) -> LoadResult {
    let path = credential_path(journal);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return LoadResult::Absent,
        Err(_) => return LoadResult::Unreadable,
    };
    let Ok(doc) = serde_json::from_slice::<ChatGptSignInDoc>(&bytes) else {
        return LoadResult::Unreadable;
    };
    if !doc.validate_invariants() {
        return LoadResult::Unreadable;
    }
    LoadResult::Present(doc)
}

pub fn save_credential_file(
    journal: &Path,
    doc: &ChatGptSignInDoc,
) -> Result<(), AtomicWriteError> {
    let path = credential_path(journal);
    write_json(
        &path,
        doc,
        JsonWriteOptions {
            mode: Some(0o600),
            indent: Some(2),
            sort_keys: true,
        },
    )
}

pub fn rotate_unreadable_credential_file(
    journal: &Path,
) -> Result<Option<PathBuf>, std::io::Error> {
    let path = credential_path(journal);
    if !path.exists() {
        return Ok(None);
    }
    let now = now_secs();
    let dest = journal.join(format!("config/chatgpt-sign-in.json.unreadable-{now}"));
    fs::rename(&path, &dest)?;
    Ok(Some(dest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doc_invariants_valid() {
        let mut doc = ChatGptSignInDoc::new_initial("host-1".to_string());
        assert!(doc.validate_invariants());

        doc.tokens = Some(Tokens {
            access_token: "acc".to_string(),
            refresh_token: "ref".to_string(),
            expires_at: 1000,
            scopes: vec![DIRECT_USE_SCOPE.to_string()],
        });
        // Without registration, tokens are invalid
        assert!(!doc.validate_invariants());

        doc.registration = Some(Registration {
            client_id: "client-1".to_string(),
            client_refused: false,
        });
        assert!(doc.validate_invariants());

        // Missing direct use scope
        doc.tokens.as_mut().unwrap().scopes = vec!["openid".to_string()];
        assert!(!doc.validate_invariants());
    }

    #[test]
    fn load_and_save_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let journal = dir.path();
        std::fs::create_dir_all(journal.join("config")).expect("create config");

        assert!(matches!(load_credential_file(journal), LoadResult::Absent));

        let mut doc = ChatGptSignInDoc::new_initial("host-roundtrip".to_string());
        doc.registration = Some(Registration {
            client_id: "client-123".to_string(),
            client_refused: false,
        });
        doc.tokens = Some(Tokens {
            access_token: "token-1".to_string(),
            refresh_token: "refresh-1".to_string(),
            expires_at: 5000,
            scopes: vec![DIRECT_USE_SCOPE.to_string()],
        });

        save_credential_file(journal, &doc).expect("save");

        match load_credential_file(journal) {
            LoadResult::Present(loaded) => {
                assert_eq!(loaded.host_id, "host-roundtrip");
                assert_eq!(loaded.tokens.unwrap().access_token, "token-1");
            }
            _ => panic!("expected LoadResult::Present"),
        }
    }

    #[test]
    fn load_unreadable_and_rotate() {
        let dir = tempfile::tempdir().expect("tempdir");
        let journal = dir.path();
        std::fs::create_dir_all(journal.join("config")).expect("create config");

        let path = credential_path(journal);
        std::fs::write(&path, b"not json content").expect("write bad json");

        assert!(matches!(
            load_credential_file(journal),
            LoadResult::Unreadable
        ));

        let rotated = rotate_unreadable_credential_file(journal).expect("rotate");
        assert!(rotated.is_some());
        assert!(!path.exists());
        assert!(rotated.unwrap().exists());
    }
}
