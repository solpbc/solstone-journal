// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Reusable `0o600` atomic OAuth credential storage (`config/<service>-sign-in.json`),
//! 45-second `flock`-serialized rotating refresh with crash-durable renewal sidecar
//! (`config/<service>-sign-in.json.renewal`), and lock-free RFC 7009 revocation.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use solstone_core_journal_io::{
    AtomicWriteError, AtomicWriteOptions, FileLock, JsonWriteOptions, LockError, LockOptions,
    atomic_replace, hold_lock, write_json,
};

use crate::atproto::{DpopKeyPair, DpopRequestError, send_with_dpop_nonce_retry};
use crate::registration::RegistrationMode;
use crate::ssrf::{
    HttpMethod, HttpRequest, HttpResponse, OAuthHttpClient, SsrfFetchError, validate_https_url,
};
use crate::{encode_form, extract_oauth_error_code};

pub const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(45);
pub const REFRESH_TIMEOUT: Duration = Duration::from_secs(30);
pub const REVOKE_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_EXPIRATION_MARGIN_SECS: u64 = 300;

#[cfg(test)]
thread_local! {
    static TEST_NOW_SECS: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
    static TEST_LOCK_TIMEOUT: std::cell::Cell<Option<Duration>> = const { std::cell::Cell::new(None) };
    static TEST_FAIL_NEXT_PRIMARY_SAVE_AFTER_RENEWAL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub fn with_test_now_secs<T>(now: u64, f: impl FnOnce() -> T) -> T {
    TEST_NOW_SECS.with(|cell| {
        let prev = cell.replace(Some(now));
        let out = f();
        cell.set(prev);
        out
    })
}

#[cfg(test)]
pub fn with_test_lock_timeout<T>(timeout: Duration, f: impl FnOnce() -> T) -> T {
    TEST_LOCK_TIMEOUT.with(|cell| {
        let prev = cell.replace(Some(timeout));
        let out = f();
        cell.set(prev);
        out
    })
}

#[cfg(test)]
pub fn arm_fail_next_primary_save_after_renewal() {
    TEST_FAIL_NEXT_PRIMARY_SAVE_AFTER_RENEWAL.with(|cell| cell.set(true));
}

pub fn now_unix_secs() -> u64 {
    #[cfg(test)]
    if let Some(now) = TEST_NOW_SECS.with(std::cell::Cell::get) {
        return now;
    }
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn effective_lock_options(timeout: Duration) -> LockOptions {
    #[cfg(test)]
    let timeout = TEST_LOCK_TIMEOUT
        .with(std::cell::Cell::get)
        .unwrap_or(timeout);
    LockOptions {
        mode: Some(0o600),
        timeout,
        ..Default::default()
    }
}

/// Validated lowercase service identifier used to construct safe `config/<service>-sign-in.json*` paths.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ServiceId(String);

impl ServiceId {
    /// Admit lowercase ASCII `[a-z0-9][a-z0-9_-]{0,63}` with no path separators or traversal.
    pub fn new(raw: impl Into<String>) -> Result<Self, CredentialError> {
        let value = raw.into();
        let bytes = value.as_bytes();
        if bytes.is_empty()
            || bytes.len() > 64
            || !(bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
            || !bytes
                .iter()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-' || *b == b'_')
        {
            return Err(CredentialError::InvalidServiceId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// OAuth client registration metadata stored in `config/<service>-sign-in.json`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientRegistration {
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    pub mode: RegistrationMode,
    #[serde(default)]
    pub client_refused: bool,
}

impl fmt::Debug for ClientRegistration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClientRegistration")
            .field("client_id", &self.client_id)
            .field("has_client_secret", &self.client_secret.is_some())
            .field("mode", &self.mode)
            .field("client_refused", &self.client_refused)
            .finish()
    }
}

/// Persisted OAuth token payload. Never logs token strings in `Debug`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OAuthTokens {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    pub token_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    #[serde(default)]
    pub scopes: Vec<String>,
}

impl fmt::Debug for OAuthTokens {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OAuthTokens")
            .field("token_type", &self.token_type)
            .field("has_refresh_token", &self.refresh_token.is_some())
            .field("expires_at", &self.expires_at)
            .field("scopes", &self.scopes)
            .finish_non_exhaustive()
    }
}

/// Root credential document stored atomically (`0o600`) at `config/<service>-sign-in.json`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OAuthCredentialDoc {
    pub version: u32,
    pub service: String,
    pub epoch: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    pub token_endpoint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revocation_endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registration: Option<ClientRegistration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dpop_key_pkcs8_b64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dpop_nonce: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<OAuthTokens>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, Value>,
}

impl fmt::Debug for OAuthCredentialDoc {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OAuthCredentialDoc")
            .field("version", &self.version)
            .field("service", &self.service)
            .field("epoch", &self.epoch)
            .field("has_registration", &self.registration.is_some())
            .field("has_dpop_key", &self.dpop_key_pkcs8_b64.is_some())
            .field("has_tokens", &self.tokens.is_some())
            .finish_non_exhaustive()
    }
}

impl OAuthCredentialDoc {
    pub fn new(service: &ServiceId, token_endpoint: impl Into<String>) -> Self {
        Self {
            version: 1,
            service: service.as_str().to_owned(),
            epoch: 1,
            issuer: None,
            resource: None,
            token_endpoint: token_endpoint.into(),
            revocation_endpoint: None,
            registration: None,
            dpop_key_pkcs8_b64: None,
            dpop_nonce: None,
            subject: None,
            account_label: None,
            tokens: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn validate_invariants(&self, expected_service: &ServiceId) -> bool {
        if self.version != 1
            || self.service != expected_service.as_str()
            || self.epoch == 0
            || self.token_endpoint.is_empty()
        {
            return false;
        }
        if let Some(tokens) = &self.tokens
            && (self.registration.is_none()
                || tokens.access_token.is_empty()
                || tokens.token_type.is_empty())
        {
            return false;
        }
        true
    }
}

/// Crash-durable renewal sidecar persisted at `config/<service>-sign-in.json.renewal` (`0o600`)
/// before overwriting `config/<service>-sign-in.json`, protecting single-use rotating refresh tokens.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HeldRenewal {
    version: u32,
    service: String,
    epoch: u64,
    spent_refresh_token: String,
    tokens: OAuthTokens,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dpop_nonce: Option<String>,
}

/// Result of reading a service's credential file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum LoadResult {
    Present(OAuthCredentialDoc),
    Absent,
    Unreadable,
}

/// Live token view returned to service callers.
#[derive(Clone, PartialEq, Eq)]
pub struct AccessTokenView {
    pub access_token: String,
    pub token_type: String,
    pub expires_at: Option<u64>,
    pub dpop_key_pkcs8_b64: Option<String>,
    pub dpop_nonce: Option<String>,
}

impl fmt::Debug for AccessTokenView {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AccessTokenView")
            .field("token_type", &self.token_type)
            .field("expires_at", &self.expires_at)
            .field("has_dpop_key", &self.dpop_key_pkcs8_b64.is_some())
            .finish_non_exhaustive()
    }
}

/// Outcome of signing out of a stored service credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignOutResult {
    /// `true` when remote revocation succeeded or no revocation endpoint / refresh token existed.
    pub revoked: bool,
}

/// Errors from credential storage, refresh, and sign-out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialError {
    InvalidServiceId,
    SignInRequired,
    Storage(PathBuf),
    Busy,
    Network,
    Unavailable,
    Refused(Option<String>),
    Malformed(&'static str),
    GrantNotSaved,
    Io(String),
}

impl fmt::Display for CredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidServiceId => formatter.write_str("invalid OAuth service identifier"),
            Self::SignInRequired => formatter.write_str("service sign-in is required"),
            Self::Storage(path) => {
                write!(
                    formatter,
                    "unreadable credential storage at {}",
                    path.display()
                )
            }
            Self::Busy => formatter.write_str("timed out waiting for credential lock"),
            Self::Network => formatter.write_str("network error while refreshing OAuth credential"),
            Self::Unavailable => {
                formatter.write_str("OAuth token endpoint temporarily unavailable")
            }
            Self::Refused(Some(code)) => write!(formatter, "OAuth token refresh refused: {code}"),
            Self::Refused(None) => formatter.write_str("OAuth token refresh refused"),
            Self::Malformed(field) => {
                write!(formatter, "malformed refresh response field: {field}")
            }
            Self::GrantNotSaved => {
                formatter.write_str("rotated OAuth refresh grant could not be persisted")
            }
            Self::Io(message) => write!(formatter, "credential I/O error: {message}"),
        }
    }
}

impl std::error::Error for CredentialError {}

/// Compute `config/<service>-sign-in.json` under `journal`.
pub fn credential_path(journal: &Path, service: &ServiceId) -> PathBuf {
    journal
        .join("config")
        .join(format!("{}-sign-in.json", service.as_str()))
}

/// Compute `config/<service>-sign-in.json.renewal` under `journal`.
pub fn renewal_sidecar_path(journal: &Path, service: &ServiceId) -> PathBuf {
    journal
        .join("config")
        .join(format!("{}-sign-in.json.renewal", service.as_str()))
}

/// Acquire the `0o600` sidecar `flock` protecting `config/<service>-sign-in.json`.
pub fn acquire_credential_lock(
    journal: &Path,
    service: &ServiceId,
    timeout: Duration,
) -> Result<FileLock, LockError> {
    let path = credential_path(journal, service);
    hold_lock(&path, effective_lock_options(timeout))
}

/// Load and validate `config/<service>-sign-in.json`.
pub fn load_credential_file(journal: &Path, service: &ServiceId) -> LoadResult {
    let path = credential_path(journal, service);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return LoadResult::Absent,
        Err(_) => return LoadResult::Unreadable,
    };
    let Ok(doc) = serde_json::from_slice::<OAuthCredentialDoc>(&bytes) else {
        return LoadResult::Unreadable;
    };
    if !doc.validate_invariants(service) {
        return LoadResult::Unreadable;
    }
    LoadResult::Present(doc)
}

/// Atomically persist `config/<service>-sign-in.json` with mode `0o600`.
pub fn save_credential_file(
    journal: &Path,
    service: &ServiceId,
    doc: &OAuthCredentialDoc,
) -> Result<(), AtomicWriteError> {
    let path = credential_path(journal, service);
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

/// Move aside an unreadable credential file to `config/<service>-sign-in.json.unreadable-<ts>`.
pub fn rotate_unreadable_credential_file(
    journal: &Path,
    service: &ServiceId,
) -> Result<Option<PathBuf>, std::io::Error> {
    let path = credential_path(journal, service);
    if !path.exists() {
        return Ok(None);
    }
    let now = now_unix_secs();
    let config_dir = journal.join("config");
    let mut dest = config_dir.join(format!(
        "{}-sign-in.json.unreadable-{now}",
        service.as_str()
    ));
    let mut suffix = 1u32;
    while dest.exists() {
        dest = config_dir.join(format!(
            "{}-sign-in.json.unreadable-{now}-{suffix}",
            service.as_str()
        ));
        suffix = suffix.saturating_add(1);
    }
    fs::rename(&path, &dest)?;
    Ok(Some(dest))
}

fn write_renewal_sidecar(journal: &Path, service: &ServiceId, held: &HeldRenewal) {
    let path = renewal_sidecar_path(journal, service);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec(held) {
        let _ = atomic_replace(&path, &bytes, AtomicWriteOptions { mode: Some(0o600) });
    }
}

fn clear_renewal_sidecar(journal: &Path, service: &ServiceId) {
    let _ = fs::remove_file(renewal_sidecar_path(journal, service));
}

/// Reconcile a crash-left renewal sidecar while holding the credential `flock`.
///
/// If the credential file still holds `spent_refresh_token` at the matching `epoch`,
/// promotes the held rotated tokens into the primary credential file and removes the sidecar.
/// Otherwise (the credential file moved on, signed out, or sidecar is corrupt), deletes the sidecar.
fn apply_held_renewal_locked(
    journal: &Path,
    service: &ServiceId,
    doc: &mut OAuthCredentialDoc,
) -> Result<bool, CredentialError> {
    let path = renewal_sidecar_path(journal, service);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => {
            clear_renewal_sidecar(journal, service);
            return Ok(false);
        }
    };
    let Ok(held) = serde_json::from_slice::<HeldRenewal>(&bytes) else {
        clear_renewal_sidecar(journal, service);
        return Ok(false);
    };
    if held.version != 1 || held.service != service.as_str() || held.epoch != doc.epoch {
        clear_renewal_sidecar(journal, service);
        return Ok(false);
    }
    let current_refresh = doc.tokens.as_ref().and_then(|t| t.refresh_token.as_deref());
    if current_refresh != Some(held.spent_refresh_token.as_str()) {
        clear_renewal_sidecar(journal, service);
        return Ok(false);
    }
    doc.tokens = Some(held.tokens);
    if held.dpop_nonce.is_some() {
        doc.dpop_nonce = held.dpop_nonce;
    }
    save_credential_file(journal, service, doc).map_err(|_| CredentialError::GrantNotSaved)?;
    clear_renewal_sidecar(journal, service);
    Ok(true)
}

/// Lock-free RFC 7009 token revocation (up to 2 attempts on 5xx / transport error).
pub fn revoke_refresh_token(
    http: &dyn OAuthHttpClient,
    revocation_endpoint: &str,
    refresh_token: &str,
    client_id: &str,
    client_secret: Option<&str>,
) -> bool {
    if validate_https_url(revocation_endpoint).is_err() {
        return false;
    }
    let mut form = BTreeMap::new();
    form.insert("token".to_owned(), refresh_token.to_owned());
    form.insert("token_type_hint".to_owned(), "refresh_token".to_owned());
    form.insert("client_id".to_owned(), client_id.to_owned());
    if let Some(secret) = client_secret.filter(|s| !s.is_empty()) {
        form.insert("client_secret".to_owned(), secret.to_owned());
    }
    let body = encode_form(&form).into_bytes();

    for attempt in 0..2 {
        let req = HttpRequest::post(
            revocation_endpoint,
            "application/x-www-form-urlencoded",
            body.clone(),
        )
        .with_timeout(REVOKE_TIMEOUT);
        match http.send(&req) {
            Ok(HttpResponse { status, .. }) if (200..=299).contains(&status) => return true,
            Ok(HttpResponse { status, .. }) if (500..=599).contains(&status) && attempt == 0 => {
                continue;
            }
            Ok(_) => return false,
            Err(SsrfFetchError::Connect | SsrfFetchError::Deadline | SsrfFetchError::Tls)
                if attempt == 0 =>
            {
                continue;
            }
            Err(_) => return false,
        }
    }
    false
}

/// Reusable manager providing lock-free valid-token reads, 45s `flock`-serialized
/// crash-durable rotating refresh, and lock-free sign-out/revocation for one service.
pub struct OAuthCredentialManager {
    journal: PathBuf,
    service: ServiceId,
    http: Arc<dyn OAuthHttpClient>,
    expiration_margin_secs: u64,
    lock_timeout: Duration,
}

impl OAuthCredentialManager {
    pub fn new(journal: PathBuf, service: ServiceId, http: Arc<dyn OAuthHttpClient>) -> Self {
        Self {
            journal,
            service,
            http,
            expiration_margin_secs: DEFAULT_EXPIRATION_MARGIN_SECS,
            lock_timeout: DEFAULT_LOCK_TIMEOUT,
        }
    }

    pub fn with_expiration_margin_secs(mut self, margin_secs: u64) -> Self {
        self.expiration_margin_secs = margin_secs;
        self
    }

    pub fn with_lock_timeout(mut self, timeout: Duration) -> Self {
        self.lock_timeout = timeout;
        self
    }

    pub fn journal(&self) -> &Path {
        &self.journal
    }

    pub fn service(&self) -> &ServiceId {
        &self.service
    }

    fn is_token_fresh(&self, tokens: &OAuthTokens, now: u64) -> bool {
        match tokens.expires_at {
            Some(exp) => exp > now.saturating_add(self.expiration_margin_secs),
            None => true,
        }
    }

    fn token_view(doc: &OAuthCredentialDoc, tokens: &OAuthTokens) -> AccessTokenView {
        AccessTokenView {
            access_token: tokens.access_token.clone(),
            token_type: tokens.token_type.clone(),
            expires_at: tokens.expires_at,
            dpop_key_pkcs8_b64: doc.dpop_key_pkcs8_b64.clone(),
            dpop_nonce: doc.dpop_nonce.clone(),
        }
    }

    /// Retrieve a valid access token.
    ///
    /// Returns lock-free when no crash-recovery sidecar exists and the stored token has at
    /// least `expiration_margin_secs` remaining. Otherwise acquires the 45s `flock`, reconciles
    /// any pending renewal sidecar, re-checks freshness, and refreshes if still needed.
    pub fn access_token(&self) -> Result<AccessTokenView, CredentialError> {
        let now = now_unix_secs();

        // 1. Lock-free fast path (only when no renewal sidecar is waiting to be promoted).
        if !renewal_sidecar_path(&self.journal, &self.service).exists() {
            match load_credential_file(&self.journal, &self.service) {
                LoadResult::Present(doc) => {
                    if let Some(tokens) = &doc.tokens {
                        if self.is_token_fresh(tokens, now) {
                            return Ok(Self::token_view(&doc, tokens));
                        }
                    } else {
                        return Err(CredentialError::SignInRequired);
                    }
                }
                LoadResult::Absent => return Err(CredentialError::SignInRequired),
                LoadResult::Unreadable => {
                    return Err(CredentialError::Storage(credential_path(
                        &self.journal,
                        &self.service,
                    )));
                }
            }
        }

        // 2. Acquire 45s flock.
        let _lock = acquire_credential_lock(&self.journal, &self.service, self.lock_timeout)
            .map_err(|err| match err {
                LockError::Timeout(_) => CredentialError::Busy,
                other => CredentialError::Io(other.to_string()),
            })?;

        // 3. Re-read under lock & reconcile any crash-left renewal sidecar first.
        let mut doc = match load_credential_file(&self.journal, &self.service) {
            LoadResult::Present(doc) => doc,
            LoadResult::Absent => {
                clear_renewal_sidecar(&self.journal, &self.service);
                return Err(CredentialError::SignInRequired);
            }
            LoadResult::Unreadable => {
                return Err(CredentialError::Storage(credential_path(
                    &self.journal,
                    &self.service,
                )));
            }
        };

        apply_held_renewal_locked(&self.journal, &self.service, &mut doc)?;

        let now = now_unix_secs();
        if let Some(tokens) = &doc.tokens {
            if self.is_token_fresh(tokens, now) {
                return Ok(Self::token_view(&doc, tokens));
            }
        } else {
            return Err(CredentialError::SignInRequired);
        }

        // 4. Perform refresh under lock.
        self.perform_refresh_locked(&mut doc)
    }

    /// Force a locked refresh after a resource server rejected `rejected_access_token` with 401.
    /// If another caller already rotated to a different fresh token, returns that token instead.
    pub fn access_token_after_rejection(
        &self,
        rejected_access_token: &str,
    ) -> Result<AccessTokenView, CredentialError> {
        let _lock = acquire_credential_lock(&self.journal, &self.service, self.lock_timeout)
            .map_err(|err| match err {
                LockError::Timeout(_) => CredentialError::Busy,
                other => CredentialError::Io(other.to_string()),
            })?;

        let mut doc = match load_credential_file(&self.journal, &self.service) {
            LoadResult::Present(doc) => doc,
            LoadResult::Absent => return Err(CredentialError::SignInRequired),
            LoadResult::Unreadable => {
                return Err(CredentialError::Storage(credential_path(
                    &self.journal,
                    &self.service,
                )));
            }
        };

        apply_held_renewal_locked(&self.journal, &self.service, &mut doc)?;

        let now = now_unix_secs();
        if let Some(tokens) = &doc.tokens {
            if tokens.access_token != rejected_access_token && self.is_token_fresh(tokens, now) {
                return Ok(Self::token_view(&doc, tokens));
            }
        } else {
            return Err(CredentialError::SignInRequired);
        }

        self.perform_refresh_locked(&mut doc)
    }

    /// Clear tokens (and optionally registration/DPoP key when `forget` is `true`) under lock,
    /// release the lock, and revoke any held refresh token lock-free.
    pub fn sign_out(&self, forget: bool) -> Result<SignOutResult, CredentialError> {
        let _lock = acquire_credential_lock(&self.journal, &self.service, self.lock_timeout)
            .map_err(|err| match err {
                LockError::Timeout(_) => CredentialError::Busy,
                other => CredentialError::Io(other.to_string()),
            })?;

        clear_renewal_sidecar(&self.journal, &self.service);

        let (mut doc, revoke_target) = match load_credential_file(&self.journal, &self.service) {
            LoadResult::Absent => return Ok(SignOutResult { revoked: true }),
            LoadResult::Unreadable => {
                if forget {
                    let _ = rotate_unreadable_credential_file(&self.journal, &self.service);
                    return Ok(SignOutResult { revoked: false });
                }
                return Err(CredentialError::Storage(credential_path(
                    &self.journal,
                    &self.service,
                )));
            }
            LoadResult::Present(doc) => {
                let target = doc
                    .revocation_endpoint
                    .clone()
                    .zip(doc.tokens.as_ref().and_then(|t| t.refresh_token.clone()))
                    .zip(
                        doc.registration
                            .as_ref()
                            .map(|r| (r.client_id.clone(), r.client_secret.clone())),
                    );
                (doc, target)
            }
        };

        if forget {
            doc.tokens = None;
            doc.registration = None;
            doc.dpop_key_pkcs8_b64 = None;
            doc.dpop_nonce = None;
            doc.subject = None;
            doc.account_label = None;
            doc.epoch = doc.epoch.saturating_add(1);
        } else {
            doc.tokens = None;
        }

        save_credential_file(&self.journal, &self.service, &doc)
            .map_err(|err| CredentialError::Io(err.to_string()))?;

        // Release the file lock before making any network call.
        drop(_lock);

        if let Some(((revocation_endpoint, refresh_token), (client_id, client_secret))) =
            revoke_target
        {
            let revoked = revoke_refresh_token(
                self.http.as_ref(),
                &revocation_endpoint,
                &refresh_token,
                &client_id,
                client_secret.as_deref(),
            );
            Ok(SignOutResult { revoked })
        } else {
            Ok(SignOutResult { revoked: true })
        }
    }

    fn perform_refresh_locked(
        &self,
        doc: &mut OAuthCredentialDoc,
    ) -> Result<AccessTokenView, CredentialError> {
        let Some(existing_tokens) = doc.tokens.clone() else {
            return Err(CredentialError::SignInRequired);
        };
        let Some(spent_refresh_token) = existing_tokens.refresh_token.clone() else {
            return Err(CredentialError::SignInRequired);
        };
        let Some(registration) = doc.registration.clone() else {
            return Err(CredentialError::Storage(credential_path(
                &self.journal,
                &self.service,
            )));
        };

        let mut form = BTreeMap::new();
        form.insert("grant_type".to_owned(), "refresh_token".to_owned());
        form.insert("refresh_token".to_owned(), spent_refresh_token.clone());
        form.insert("client_id".to_owned(), registration.client_id.clone());
        if let Some(secret) = registration
            .client_secret
            .as_deref()
            .filter(|s| !s.is_empty())
        {
            form.insert("client_secret".to_owned(), secret.to_owned());
        }
        if let Some(resource) = doc.resource.as_deref().filter(|r| !r.is_empty()) {
            form.insert("resource".to_owned(), resource.to_owned());
        }
        let body = encode_form(&form).into_bytes();

        let (response, latest_nonce) = if let Some(dpop_b64) = &doc.dpop_key_pkcs8_b64 {
            let dpop_key = DpopKeyPair::from_pkcs8_b64(dpop_b64).map_err(|_| {
                CredentialError::Storage(credential_path(&self.journal, &self.service))
            })?;
            let content_headers = vec![(
                "Content-Type".to_owned(),
                "application/x-www-form-urlencoded".to_owned(),
            )];
            send_with_dpop_nonce_retry(
                self.http.as_ref(),
                &dpop_key,
                doc.dpop_nonce.as_deref(),
                HttpMethod::Post,
                &doc.token_endpoint,
                &content_headers,
                Some(body),
                None,
                now_unix_secs(),
                REFRESH_TIMEOUT,
            )
            .map_err(|err| match err {
                DpopRequestError::Fetch(_) => CredentialError::Network,
                DpopRequestError::Dpop(_) | DpopRequestError::Random => {
                    CredentialError::Io(err.to_string())
                }
            })?
        } else {
            let req = HttpRequest::post(
                &doc.token_endpoint,
                "application/x-www-form-urlencoded",
                body,
            )
            .with_timeout(REFRESH_TIMEOUT);
            let resp = self.http.send(&req).map_err(|_| CredentialError::Network)?;
            (resp, None)
        };

        if let Some(nonce) = latest_nonce.clone() {
            doc.dpop_nonce = Some(nonce);
        }

        match response.status {
            200 => {
                let Ok(json) = serde_json::from_slice::<Value>(&response.body) else {
                    return Err(CredentialError::Malformed("refresh_response"));
                };
                let new_refresh = json
                    .get("refresh_token")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| spent_refresh_token.clone());

                let access_token_opt = json
                    .get("access_token")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty());
                let expires_in_opt = json
                    .get("expires_in")
                    .and_then(Value::as_u64)
                    .filter(|v| *v > 0);
                let token_type = json
                    .get("token_type")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| existing_tokens.token_type.clone());
                let scopes = json
                    .get("scope")
                    .and_then(Value::as_str)
                    .map(|s| s.split_whitespace().map(ToOwned::to_owned).collect())
                    .unwrap_or_else(|| existing_tokens.scopes.clone());

                let mut malformed_field = None;
                let (access_token, expires_at) = if let Some(at) = access_token_opt {
                    let exp = expires_in_opt.map(|ei| now_unix_secs().saturating_add(ei));
                    (at.to_owned(), exp)
                } else {
                    malformed_field = Some("access_token");
                    (existing_tokens.access_token.clone(), Some(0))
                };

                let updated_tokens = OAuthTokens {
                    access_token,
                    refresh_token: Some(new_refresh),
                    token_type,
                    expires_at,
                    scopes,
                };

                // Phase 1: write crash-durable renewal sidecar BEFORE touching the primary file.
                let held = HeldRenewal {
                    version: 1,
                    service: self.service.as_str().to_owned(),
                    epoch: doc.epoch,
                    spent_refresh_token,
                    tokens: updated_tokens.clone(),
                    dpop_nonce: latest_nonce,
                };
                write_renewal_sidecar(&self.journal, &self.service, &held);

                // Phase 2: update primary credential file atomically.
                #[cfg(test)]
                if TEST_FAIL_NEXT_PRIMARY_SAVE_AFTER_RENEWAL.with(|cell| cell.replace(false)) {
                    return Err(CredentialError::GrantNotSaved);
                }

                doc.tokens = Some(updated_tokens.clone());
                if save_credential_file(&self.journal, &self.service, doc).is_err() {
                    return Err(CredentialError::GrantNotSaved);
                }

                // Phase 3: clear sidecar only after primary write committed.
                clear_renewal_sidecar(&self.journal, &self.service);

                if let Some(field) = malformed_field {
                    return Err(CredentialError::Malformed(field));
                }

                Ok(Self::token_view(doc, &updated_tokens))
            }
            429 | 500..=599 => Err(CredentialError::Unavailable),
            400..=499 => {
                let err_code = extract_oauth_error_code(&response.body);
                let is_invalid_grant = matches!(
                    err_code.as_deref(),
                    Some(
                        "invalid_grant"
                            | "invalid_refresh_token"
                            | "token_expired"
                            | "refresh_token_expired"
                            | "refresh_token_invalidated"
                            | "refresh_token_reused"
                    )
                );
                if is_invalid_grant {
                    doc.tokens = None;
                    save_credential_file(&self.journal, &self.service, doc)
                        .map_err(|err| CredentialError::Io(err.to_string()))?;
                    return Err(CredentialError::SignInRequired);
                }
                if err_code.as_deref() == Some("invalid_client") {
                    doc.tokens = None;
                    if let Some(reg) = &mut doc.registration {
                        reg.client_refused = true;
                    }
                    save_credential_file(&self.journal, &self.service, doc)
                        .map_err(|err| CredentialError::Io(err.to_string()))?;
                    return Err(CredentialError::SignInRequired);
                }
                Err(CredentialError::Refused(err_code))
            }
            _ => Err(CredentialError::Refused(None)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;

    struct ScriptedHttp {
        responses: Mutex<VecDeque<HttpResponse>>,
        requests: Mutex<Vec<HttpRequest>>,
    }

    impl ScriptedHttp {
        fn new(responses: Vec<HttpResponse>) -> Self {
            Self {
                responses: Mutex::new(VecDeque::from(responses)),
                requests: Mutex::new(Vec::new()),
            }
        }
    }

    impl OAuthHttpClient for ScriptedHttp {
        fn send(&self, request: &HttpRequest) -> Result<HttpResponse, SsrfFetchError> {
            self.requests.lock().unwrap().push(request.clone());
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or(SsrfFetchError::Connect)
        }
    }

    fn seed_doc(service: &ServiceId, expires_at: u64) -> OAuthCredentialDoc {
        let mut doc = OAuthCredentialDoc::new(service, "https://mcp.plaud.ai/token");
        doc.revocation_endpoint = Some("https://mcp.plaud.ai/revoke".to_owned());
        doc.resource = Some("https://mcp.plaud.ai/mcp".to_owned());
        doc.registration = Some(ClientRegistration {
            client_id: "plaud-client-1".to_owned(),
            client_secret: None,
            mode: RegistrationMode::Dcr,
            client_refused: false,
        });
        doc.tokens = Some(OAuthTokens {
            access_token: "old-access".to_owned(),
            refresh_token: Some("old-refresh".to_owned()),
            token_type: "Bearer".to_owned(),
            expires_at: Some(expires_at),
            scopes: vec!["read".to_owned()],
        });
        doc
    }

    #[test]
    fn service_id_rejects_traversal_and_admits_slugs() {
        assert!(ServiceId::new("plaud").is_ok());
        assert!(ServiceId::new("atproto_pds-1").is_ok());
        for bad in ["", "../etc", "a/b", "Plaud", "-leading", "with space"] {
            assert_eq!(ServiceId::new(bad), Err(CredentialError::InvalidServiceId));
        }
    }

    #[test]
    fn crash_after_renewal_sidecar_recovers_rotated_token_without_resending_spent_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path().to_path_buf();
        let service = ServiceId::new("plaud").unwrap();
        let doc = seed_doc(&service, 100);
        save_credential_file(&journal, &service, &doc).unwrap();

        let http = Arc::new(ScriptedHttp::new(vec![HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: br#"{"access_token":"new-access-2","refresh_token":"new-refresh-2","token_type":"Bearer","expires_in":3600}"#.to_vec(),
        }]));
        let manager = OAuthCredentialManager::new(journal.clone(), service.clone(), http.clone());

        // Simulate crash right after writing config/plaud-sign-in.json.renewal, before saving primary file.
        with_test_now_secs(1000, || {
            arm_fail_next_primary_save_after_renewal();
            assert_eq!(manager.access_token(), Err(CredentialError::GrantNotSaved));
            assert!(
                renewal_sidecar_path(&journal, &service).exists(),
                "renewal sidecar must survive primary write failure"
            );

            // Next call recovers rotated tokens directly from the sidecar with ZERO extra HTTP requests!
            let recovered = manager
                .access_token()
                .expect("recovers rotated token from renewal sidecar");
            assert_eq!(recovered.access_token, "new-access-2");
            assert!(
                !renewal_sidecar_path(&journal, &service).exists(),
                "renewal sidecar must be removed after promotion"
            );
        });

        assert_eq!(
            http.requests.lock().unwrap().len(),
            1,
            "spent refresh token must never be re-sent after sidecar recovery"
        );
        match load_credential_file(&journal, &service) {
            LoadResult::Present(loaded) => {
                assert_eq!(
                    loaded.tokens.unwrap().refresh_token.as_deref(),
                    Some("new-refresh-2")
                );
            }
            other => panic!("expected Present, got {other:?}"),
        }
    }

    #[test]
    fn sign_out_clears_tokens_and_revokes_lock_free() {
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path().to_path_buf();
        let service = ServiceId::new("plaud").unwrap();
        let doc = seed_doc(&service, 5000);
        save_credential_file(&journal, &service, &doc).unwrap();

        // Transport asserts the credential lock is NOT held when revocation runs!
        struct LockFreeCheckHttp {
            journal: PathBuf,
            service: ServiceId,
        }
        impl OAuthHttpClient for LockFreeCheckHttp {
            fn send(&self, _request: &HttpRequest) -> Result<HttpResponse, SsrfFetchError> {
                let lock = acquire_credential_lock(
                    &self.journal,
                    &self.service,
                    Duration::from_millis(50),
                );
                assert!(
                    lock.is_ok(),
                    "credential flock must be released before calling revocation endpoint"
                );
                Ok(HttpResponse {
                    status: 200,
                    headers: Vec::new(),
                    body: Vec::new(),
                })
            }
        }

        let manager = OAuthCredentialManager::new(
            journal.clone(),
            service.clone(),
            Arc::new(LockFreeCheckHttp {
                journal: journal.clone(),
                service: service.clone(),
            }),
        );
        let out = manager.sign_out(true).expect("sign out succeeds");
        assert!(out.revoked);

        match load_credential_file(&journal, &service) {
            LoadResult::Present(loaded) => {
                assert!(loaded.tokens.is_none());
                assert!(loaded.registration.is_none());
                assert_eq!(loaded.epoch, 2);
            }
            other => panic!("expected Present, got {other:?}"),
        }
    }

    #[test]
    fn terminal_invalid_grant_clears_tokens_while_unlisted_400_preserves_file() {
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path().to_path_buf();
        let service = ServiceId::new("plaud").unwrap();
        let doc = seed_doc(&service, 100);
        save_credential_file(&journal, &service, &doc).unwrap();
        let before = fs::read(credential_path(&journal, &service)).unwrap();

        let http = Arc::new(ScriptedHttp::new(vec![
            HttpResponse {
                status: 400,
                headers: Vec::new(),
                body: br#"{"error":"temporarily_refused_custom"}"#.to_vec(),
            },
            HttpResponse {
                status: 400,
                headers: Vec::new(),
                body: br#"{"error":"invalid_grant"}"#.to_vec(),
            },
        ]));
        let manager = OAuthCredentialManager::new(journal.clone(), service.clone(), http);

        with_test_now_secs(1000, || {
            assert_eq!(
                manager.access_token(),
                Err(CredentialError::Refused(Some(
                    "temporarily_refused_custom".to_owned()
                )))
            );
            assert_eq!(
                fs::read(credential_path(&journal, &service)).unwrap(),
                before
            );

            assert_eq!(manager.access_token(), Err(CredentialError::SignInRequired));
            match load_credential_file(&journal, &service) {
                LoadResult::Present(after) => assert!(after.tokens.is_none()),
                other => panic!("expected Present, got {other:?}"),
            }
        });
    }
}
