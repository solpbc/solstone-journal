// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable journal-local OAuth ledger.

use std::cell::{Cell, RefCell};
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, Utc};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solstone_core_journal_io::{
    AtomicWriteError, JsonWriteOptions, LockError, LockOptions, PathError,
    create_directory_with_mode, hold_lock, write_json,
};
use subtle::ConstantTimeEq;

use super::pairing::{canonicalize_pairing_code, encode_pairing_code};
use crate::permissions::ReadPermission;
use crate::tokens::{
    RandomSource, RandomSourceError, SystemRandomSource, TokenStore, VerifiedToken,
};

const OAUTH_DIRECTORY: &str = "mcp-endpoint";
const OAUTH_FILE: &str = "oauth.json";
const OAUTH_SCHEMA: u32 = 2;
const MAX_OAUTH_STATE_BYTES: usize = 16 * 1024 * 1024;
const MAX_OAUTH_ENTRY_BYTES: usize = 16 * 1024;
const MAX_CLIENTS: usize = 1024;
const MAX_CLIENTS_PER_SOURCE: usize = 16;
const CLIENT_UNUSED_TTL_SECS: i64 = 24 * 3600;
const MAX_GRANTS: usize = 1024;
const MAX_PENDING: usize = 256;
const MAX_PENDING_PER_SOURCE: usize = 8;
const PENDING_TRANSACTION_TTL_SECS: i64 = 600;
const AUTH_CODE_TTL_SECS: i64 = 300;
const PAIRING_TTL_SECS: i64 = 600;
const MAX_TRANSACTION_FAILURES: u8 = 5;
const ACCESS_TTL_SECS: i64 = 3600;
const REFRESH_TTL_SECS: i64 = 30 * 24 * 3600;
const TOKEN_BYTES: usize = 32;
const PAIRING_CODE_BYTES: usize = 5;
const REFRESH_HISTORY_LIMIT: usize = 16;
const REFRESH_GRACE_SECS: i64 = 30;
const MAX_REPLAY_NOTICES: usize = 16;

// Fault-injection hooks for the store's own takeover tests. Every other test in
// the crate redeems codes concurrently, so neither hook may reach them: the stop
// step is per thread (set and read on the test's own thread), and the hold fires
// only for the journal it was armed for.
#[cfg(test)]
thread_local! {
    pub(crate) static TAKEOVER_STOP_AFTER: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
type I4Hold = (PathBuf, Box<dyn Fn() + Send + Sync>);

#[cfg(test)]
pub(crate) static I4_TEST_HOLD: std::sync::Mutex<Option<I4Hold>> = std::sync::Mutex::new(None);

#[cfg(test)]
fn check_takeover_stop(step: usize) -> Result<(), OAuthStoreError> {
    let stop_after = TAKEOVER_STOP_AFTER.with(std::cell::Cell::get);
    if stop_after > 0 && stop_after == step {
        return Err(OAuthStoreError::InvalidGrant);
    }
    Ok(())
}
#[cfg(not(test))]
fn check_takeover_stop(_step: usize) -> Result<(), OAuthStoreError> {
    Ok(())
}

#[cfg(test)]
fn trigger_i4_hold(root: &Path) {
    let cb = match I4_TEST_HOLD.lock() {
        Ok(mut guard) if guard.as_ref().is_some_and(|(armed, _)| armed == root) => guard.take(),
        _ => None,
    };
    if let Some((_, cb)) = cb {
        cb();
    }
}
#[cfg(not(test))]
fn trigger_i4_hold(_root: &Path) {}

/// A journal-root-bound OAuth ledger.
pub struct OAuthStore {
    root: PathBuf,
    #[cfg(test)]
    pub(crate) lock_timeout: Option<std::time::Duration>,
}

/// A newly generated pairing code. The plaintext is returned exactly once.
pub struct CreatedPairingCode {
    pub code: String,
    pub expires_at: DateTime<Utc>,
    pub generation: u64,
    pub door: String,
}

/// Non-secret state for the owner's currently active pairing window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingCodeSummary {
    pub expires_at: DateTime<Utc>,
    pub generation: u64,
    pub locked: bool,
    pub door: Option<String>,
}

pub(crate) fn stored_door_to_symbol(door: Option<&str>) -> Option<String> {
    match door {
        None => None,
        Some(solstone_core_journal_config::MCP_LAN_DOOR_RESOURCE) => Some("lan".to_string()),
        Some("local") => Some("local".to_string()),
        Some("relay") => Some("relay".to_string()),
        Some(d) if d.starts_with("https://") => Some("byo".to_string()),
        Some(_) => None,
    }
}

/// Authorization code plus the GET-bound redirect fields.
pub(crate) struct IssuedAuthorization {
    pub(crate) code: String,
    pub(crate) redirect_uri: String,
    pub(crate) state: Option<String>,
    pub(crate) issuer: String,
}

/// Access and refresh tokens issued after a successful exchange.
pub(crate) struct IssuedTokens {
    pub(crate) access_token: String,
    pub(crate) refresh_token: String,
    #[allow(dead_code)]
    pub(crate) token_id: String,
    pub(crate) expires_in: i64,
    /// Present only when this exchange is the first grant the store has ever
    /// held for the client id, in any cohort: no client record with that id
    /// had redeemed a code before and no grant for it was held. A refresh
    /// never sets it.
    pub(crate) first_grant: Option<FirstGrant>,
}

/// The owner-visible name of a client receiving its first grant.
pub(crate) struct FirstGrant {
    pub(crate) client_name: Option<String>,
}

/// Public view of a registered OAuth client.
pub(crate) struct RegisteredClient {
    pub(crate) id: String,
    pub(crate) client_id: String,
    pub(crate) redirect_uris: Vec<String>,
    pub(crate) client_name: Option<String>,
    pub(crate) created_at: DateTime<Utc>,
}

/// Classification of a presented pairing code for an OAuth authorization request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CodeClassification {
    Matched { reconnect_to: Option<String> },
    NoMatch,
}

/// Stored context needed to faithfully re-render a pending consent request.
pub(crate) struct PendingAuthorization {
    pub(crate) client: RegisteredClient,
    pub(crate) redirect_uri: String,
    pub(crate) pairing_verified: bool,
    #[allow(dead_code)]
    pub(crate) reconnect_to: Option<String>,
}

/// Non-secret metadata suitable for listing registered OAuth clients.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthClientSummary {
    pub client_id: String,
    pub client_name: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Non-secret grant metadata suitable for registry tracking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthGrantSummary {
    pub id: String,
    pub client_id: String,
    pub client_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub access_expires_at: DateTime<Utc>,
    pub resource: Option<String>,
}

/// An owner-facing record of a refresh-token reuse revocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReplayNotice {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) door: String,
    pub(crate) dismissed: bool,
}

/// Failure while operating the OAuth ledger.
#[derive(Debug)]
pub enum OAuthStoreError {
    Randomness,
    InvalidToken,
    InvalidGrant,
    RefreshReused { grant_id: String },
    NoActivePairing,
    Quota,
    ClientNotFound,
    TransactionNotFound,
    TransactionExpired,
    TransactionExhausted,
    CodeExpired,
    BindingMismatch,
    PairingMismatch,
    PairingLocked,
    Directory(PathError),
    Lock(LockError),
    Read { path: PathBuf, source: io::Error },
    Malformed { path: PathBuf },
    UnsupportedSchema { path: PathBuf, found: u32 },
    Write(AtomicWriteError),
    EntryTooLarge,
    StateTooLarge,
    Permission,
}

impl fmt::Display for OAuthStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Randomness => "could not obtain complete OAuth randomness",
            Self::InvalidToken => "OAuth token is invalid",
            Self::InvalidGrant => "OAuth grant is invalid",
            Self::RefreshReused { .. } => "OAuth refresh token was reused",
            Self::NoActivePairing => "no active pairing code",
            Self::Quota => "OAuth store quota reached",
            Self::ClientNotFound => "OAuth client was not found",
            Self::TransactionNotFound => "OAuth authorization transaction was not found",
            Self::TransactionExpired => "OAuth authorization transaction expired",
            Self::TransactionExhausted => "OAuth authorization transaction is exhausted",
            Self::CodeExpired => "OAuth authorization code expired",
            Self::BindingMismatch => "OAuth request does not match the bound transaction",
            Self::PairingMismatch => "pairing code is invalid",
            Self::PairingLocked => "pairing code is locked",
            Self::Directory(_) => "could not prepare MCP OAuth directory",
            Self::Lock(_) => "could not lock MCP OAuth store",
            Self::Read { .. } => "could not read MCP OAuth store",
            Self::Malformed { .. } => "MCP OAuth store is malformed",
            Self::UnsupportedSchema { .. } => "MCP OAuth store has an unsupported schema",
            Self::Write(_) => "could not write MCP OAuth store",
            Self::EntryTooLarge => "OAuth store entry exceeds its size limit",
            Self::StateTooLarge => "OAuth store exceeds its size limit",
            Self::Permission => "the chosen permission could not be stored",
        })
    }
}

impl Error for OAuthStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Directory(error) => Some(error),
            Self::Lock(error) => Some(error),
            Self::Read { source, .. } => Some(source),
            Self::Write(error) => Some(error),
            _ => None,
        }
    }
}

impl From<crate::agent_state::AgentStateError> for OAuthStoreError {
    fn from(err: crate::agent_state::AgentStateError) -> Self {
        match err {
            crate::agent_state::AgentStateError::Lock(l) => Self::Lock(l),
            crate::agent_state::AgentStateError::Write(w) => Self::Write(w),
            crate::agent_state::AgentStateError::Directory(d) => Self::Directory(d),
            crate::agent_state::AgentStateError::Unreadable => Self::StateTooLarge,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OAuthStoreFile {
    schema: u32,
    #[serde(default)]
    pairing_generation: u64,
    clients: Vec<StoredClient>,
    grants: Vec<StoredGrant>,
    pending: Vec<StoredPending>,
    pairing: Option<StoredPairing>,
}

impl Default for OAuthStoreFile {
    fn default() -> Self {
        Self {
            schema: OAUTH_SCHEMA,
            pairing_generation: 0,
            clients: Vec::new(),
            grants: Vec::new(),
            pending: Vec::new(),
            pairing: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredClient {
    id: String,
    client_id: String,
    redirect_uris: Vec<String>,
    client_name: Option<String>,
    source: String,
    created_at: DateTime<Utc>,
    last_used_at: Option<DateTime<Utc>>,
    revocation_generation: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredGrant {
    id: String,
    client_record_id: String,
    client_id: String,
    access_verifier: String,
    refresh_verifier: String,
    refresh_generation: u64,
    revocation_generation: u64,
    access_expires_at: DateTime<Utc>,
    refresh_expires_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
    /// Once any grant carries a resource, an older binary rejects the whole oauth file as malformed, so every OAuth call on every listener fails and the agents app's view of connections fails until a binary that knows the field reads that journal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resource: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    generation: Option<u64>,
}

#[derive(Debug, Clone)]
struct RefreshHistoryEntry {
    verifier: [u8; TOKEN_BYTES],
    verifier_b64: String,
    rotated_at: i64,
    derived: bool,
}

#[derive(Debug, Clone)]
struct ParsedRefreshVerifier {
    current_verifier: [u8; TOKEN_BYTES],
    current_verifier_b64: String,
    secret: Option<[u8; TOKEN_BYTES]>,
    history: Vec<RefreshHistoryEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditPersistence {
    Persist,
    Drop,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayNoticesFile {
    notices: Vec<ReplayNotice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredPending {
    transaction_id: String,
    client_record_id: String,
    redirect_uri: String,
    resource: String,
    issuer: String,
    pkce_s256: String,
    pkce_method: String,
    state: Option<String>,
    source: String,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    failure_count: u8,
    authorization_code_verifier: Option<String>,
    code_expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    permission: Option<ReadPermission>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    generation: Option<u64>,
    /// Set while a transaction's pairing code has been checked and its
    /// requester is choosing facets. An older binary rejects the whole oauth
    /// file as malformed while any transaction is in that step, which lasts
    /// at most one transaction lifetime.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pairing_verified: bool,
    /// The generation of the pairing code that was open when this
    /// transaction started. Only that code can finish it, so a transaction
    /// never outlives the window the owner opened. Cleared once the code is
    /// used, and the row is dropped once its code is replaced, revoked or
    /// expired. An older binary rejects the whole oauth file as malformed
    /// while any row carries it; making and revoking a code clears them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pairing_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reconnect_to: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredPairing {
    verifier: String,
    expires_at: DateTime<Utc>,
    generation: u64,
    locked: bool,
    /// Once a pairing code carries a door, an older binary rejects the whole oauth file as malformed, so every OAuth call on every listener fails and the agents app's view of connections fails until a binary that knows the field reads that journal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    door: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    config_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reconnect_to: Option<String>,
}

enum TakeoverFirstEditResult {
    ExistingGrant {
        grant_id: String,
        is_paused: bool,
        pending_permission: Option<ReadPermission>,
    },
    EarlierSource {
        grant_id: String,
        permission: Option<ReadPermission>,
    },
}

impl OAuthStore {
    /// Bind an OAuth store to one journal root.
    #[must_use]
    pub fn open(journal_root: &Path) -> Self {
        Self {
            root: journal_root.to_path_buf(),
            #[cfg(test)]
            lock_timeout: None,
        }
    }

    #[cfg(test)]
    #[must_use]
    pub fn with_lock_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.lock_timeout = Some(timeout);
        self
    }

    pub fn generate_pairing_code_with_door(
        &self,
        door: &str,
    ) -> Result<CreatedPairingCode, OAuthStoreError> {
        self.generate_pairing_code_with_door_and_reconnect(door, None)
    }

    pub fn generate_pairing_code_with_door_and_reconnect(
        &self,
        door: &str,
        reconnect_to: Option<String>,
    ) -> Result<CreatedPairingCode, OAuthStoreError> {
        self.generate_pairing_code_with_random_door_and_reconnect(
            &SystemRandomSource,
            door,
            reconnect_to,
        )
    }

    #[allow(dead_code)]
    pub(crate) fn generate_pairing_code_with_random_and_door(
        &self,
        random: &dyn RandomSource,
        door: &str,
    ) -> Result<CreatedPairingCode, OAuthStoreError> {
        self.generate_pairing_code_with_random_door_and_reconnect(random, door, None)
    }

    pub(crate) fn generate_pairing_code_with_random_door_and_reconnect(
        &self,
        random: &dyn RandomSource,
        door: &str,
        reconnect_to: Option<String>,
    ) -> Result<CreatedPairingCode, OAuthStoreError> {
        let (door_stored, config_generation) = match door {
            "local" => (Some("local".to_string()), None),
            "relay" => (Some("relay".to_string()), None),
            "lan" => (
                Some(solstone_core_journal_config::MCP_LAN_DOOR_RESOURCE.to_string()),
                None,
            ),
            "byo" => {
                let config = solstone_core_journal_config::read_journal_config(&self.root)
                    .map_err(|_| OAuthStoreError::BindingMismatch)?;
                let byo = solstone_core_journal_config::byo_hostname_config(&config);
                let solstone_core_journal_config::ByoHostnameConfigStatus::Configured(byo_cfg) =
                    byo
                else {
                    return Err(OAuthStoreError::BindingMismatch);
                };
                let Some(hostname) = byo_cfg.hostname else {
                    return Err(OAuthStoreError::BindingMismatch);
                };
                (
                    Some(format!("https://{hostname}/mcp")),
                    Some(byo_cfg.generation),
                )
            }
            _ => return Err(OAuthStoreError::BindingMismatch),
        };
        let door_symbol = stored_door_to_symbol(door_stored.as_deref())
            .ok_or(OAuthStoreError::BindingMismatch)?;
        let mut code_bytes = [0_u8; PAIRING_CODE_BYTES];
        fill_exact(random, &mut code_bytes)?;
        let code = encode_pairing_code(&code_bytes);
        let verifier = sha256_b64(code.as_bytes());
        self.mutate(|store, now| {
            store.pairing_generation = store.pairing_generation.saturating_add(1).max(1);
            let expires_at = now + Duration::seconds(PAIRING_TTL_SECS);
            let generation = store.pairing_generation;
            store.pairing = Some(StoredPairing {
                verifier,
                expires_at,
                generation,
                locked: false,
                door: door_stored,
                config_generation,
                reconnect_to,
            });
            Ok(CreatedPairingCode {
                code,
                expires_at,
                generation,
                door: door_symbol,
            })
        })
    }

    /// Invalidate the active pairing code and advance generation.
    pub fn revoke_pairing_code(&self) -> Result<(), OAuthStoreError> {
        self.mutate(|store, _now| {
            if store.pairing.is_none() {
                return Err(OAuthStoreError::NoActivePairing);
            }
            store.pairing_generation = store.pairing_generation.saturating_add(1);
            store.pairing = None;
            Ok(())
        })
    }

    /// Return the current live pairing window without exposing its one-time code.
    pub fn current_pairing_code(&self) -> Result<Option<PairingCodeSummary>, OAuthStoreError> {
        let now = current_time();
        Ok(self.read_store()?.pairing.and_then(|pairing| {
            (pairing.expires_at > now).then_some(PairingCodeSummary {
                expires_at: pairing.expires_at,
                generation: pairing.generation,
                locked: pairing.locked,
                door: stored_door_to_symbol(pairing.door.as_deref()),
            })
        }))
    }

    /// Lock the current pairing code without advancing generation.
    pub(crate) fn lock_pairing_code(&self) -> Result<(), OAuthStoreError> {
        self.mutate(|store, _now| {
            let pairing = store
                .pairing
                .as_mut()
                .ok_or(OAuthStoreError::NoActivePairing)?;
            pairing.locked = true;
            Ok(())
        })
    }

    /// Whether the owner has a pairing code open for this door right now.
    ///
    /// Registration and authorization answer only while this is true, so a
    /// journal whose owner is not connecting an agent offers nothing to start.
    pub(crate) fn pairing_window_open(
        &self,
        binding: &super::RuntimeBinding,
    ) -> Result<bool, OAuthStoreError> {
        let now = current_time();
        Ok(self.read_store()?.pairing.is_some_and(|pairing| {
            pairing.expires_at > now && !pairing.locked && pairing_door_matches(&pairing, binding)
        }))
    }

    /// Whether a transaction that has not taken its pairing code yet still can:
    /// its row is live and unused, and the code it started under is still the
    /// open, unlocked code for this door. It only reads, so nothing is spent,
    /// counted or pruned.
    pub(crate) fn code_step_open(
        &self,
        transaction_id: &str,
        binding: &super::RuntimeBinding,
    ) -> Result<bool, OAuthStoreError> {
        let now = current_time();
        let store = self.read_store()?;
        let Some(pairing) = store.pairing.as_ref() else {
            return Ok(false);
        };
        if pairing.expires_at <= now || pairing.locked || !pairing_door_matches(pairing, binding) {
            return Ok(false);
        }
        Ok(store.pending.iter().any(|pending| {
            pending.transaction_id == transaction_id
                && pending.resource == binding.canonical()
                && pending.generation == binding.stored_grant_generation()
                && pending.expires_at > now
                && pending.authorization_code_verifier.is_none()
                && !pending.pairing_verified
                && pending.failure_count < MAX_TRANSACTION_FAILURES
                && pending.pairing_window == Some(pairing.generation)
        }))
    }

    /// Return the live pairing generation, or 0 when none is active.
    pub(crate) fn pairing_generation(&self) -> Result<u64, OAuthStoreError> {
        Ok(self
            .read_store()?
            .pairing
            .map(|pairing| pairing.generation)
            .unwrap_or(0))
    }

    pub(crate) fn grant_refresh_expires_at(
        &self,
        grant_id: &str,
    ) -> Result<Option<DateTime<Utc>>, OAuthStoreError> {
        let store = self.read_store()?;
        Ok(store
            .grants
            .iter()
            .find(|g| g.id == grant_id)
            .map(|g| g.refresh_expires_at))
    }

    pub(crate) fn resume_refresh_expiry(
        &self,
        grant_id: &str,
    ) -> Result<DateTime<Utc>, OAuthStoreError> {
        self.edit(
            |store, now| {
                let Some(grant) = store.grants.iter_mut().find(|g| g.id == grant_id) else {
                    return (EditPersistence::Drop, Err(OAuthStoreError::InvalidGrant));
                };
                let expiry = now + Duration::seconds(REFRESH_TTL_SECS);
                grant.refresh_expires_at = expiry;
                (EditPersistence::Persist, Ok(expiry))
            },
            || Ok(()),
        )
    }

    /// Verify a presented access token against freshly loaded durable state.
    pub(crate) fn verify_access_token(
        &self,
        presented: &str,
        binding: &super::RuntimeBinding,
    ) -> Result<VerifiedToken, OAuthStoreError> {
        let store = self.read_store()?;
        let now = current_time();
        let digest = decode_sha256(presented).ok_or(OAuthStoreError::InvalidToken)?;
        let mut verified = None;
        for grant in &store.grants {
            let verifier = decode_b64_32(&grant.access_verifier).ok_or_else(|| {
                OAuthStoreError::Malformed {
                    path: self.oauth_path(),
                }
            })?;
            if bool::from(digest.ct_eq(&verifier)) && verified.is_none() {
                if grant.access_expires_at <= now {
                    continue;
                }
                let Some(client) = store
                    .clients
                    .iter()
                    .find(|client| client.id == grant.client_record_id)
                else {
                    continue;
                };
                if grant.revocation_generation != client.revocation_generation {
                    continue;
                }
                if !binding.grant_matches(&grant.resource, grant.generation) {
                    continue;
                }
                verified = Some(VerifiedToken {
                    id: format!("oauth:{}", grant.id),
                    agent_identity: grant.client_id.clone(),
                });
            }
        }
        verified.ok_or(OAuthStoreError::InvalidToken)
    }

    /// Persist one GET /authorize transaction bound to a registered client.
    #[allow(clippy::too_many_arguments, dead_code)]
    pub(crate) fn create_transaction(
        &self,
        client_record_id: &str,
        redirect_uri: &str,
        resource: &str,
        issuer: &str,
        pkce_s256: &str,
        pkce_method: &str,
        state: Option<&str>,
        source: &str,
    ) -> Result<String, OAuthStoreError> {
        self.create_transaction_with_random(
            client_record_id,
            redirect_uri,
            resource,
            issuer,
            pkce_s256,
            pkce_method,
            state,
            source,
            None,
            &SystemRandomSource,
        )
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn create_transaction_with_generation(
        &self,
        client_record_id: &str,
        redirect_uri: &str,
        resource: &str,
        issuer: &str,
        pkce_s256: &str,
        pkce_method: &str,
        state: Option<&str>,
        source: &str,
        generation: Option<u64>,
    ) -> Result<String, OAuthStoreError> {
        self.create_transaction_with_random(
            client_record_id,
            redirect_uri,
            resource,
            issuer,
            pkce_s256,
            pkce_method,
            state,
            source,
            generation,
            &SystemRandomSource,
        )
    }

    /// Persist a GET /authorize transaction for the door `binding` serves.
    /// The open pairing code must be live, unlocked and made for this door,
    /// checked under the same write that stores the transaction.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn create_transaction_at_door(
        &self,
        binding: &super::RuntimeBinding,
        client_record_id: &str,
        redirect_uri: &str,
        resource: &str,
        issuer: &str,
        pkce_s256: &str,
        pkce_method: &str,
        state: Option<&str>,
        source: &str,
    ) -> Result<String, OAuthStoreError> {
        self.create_transaction_inner(
            client_record_id,
            redirect_uri,
            resource,
            issuer,
            pkce_s256,
            pkce_method,
            state,
            source,
            binding.stored_grant_generation(),
            Some(binding),
            &SystemRandomSource,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn create_transaction_with_random(
        &self,
        client_record_id: &str,
        redirect_uri: &str,
        resource: &str,
        issuer: &str,
        pkce_s256: &str,
        pkce_method: &str,
        state: Option<&str>,
        source: &str,
        generation: Option<u64>,
        random: &dyn RandomSource,
    ) -> Result<String, OAuthStoreError> {
        self.create_transaction_inner(
            client_record_id,
            redirect_uri,
            resource,
            issuer,
            pkce_s256,
            pkce_method,
            state,
            source,
            generation,
            None,
            random,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn create_transaction_inner(
        &self,
        client_record_id: &str,
        redirect_uri: &str,
        resource: &str,
        issuer: &str,
        pkce_s256: &str,
        pkce_method: &str,
        state: Option<&str>,
        source: &str,
        generation: Option<u64>,
        door: Option<&super::RuntimeBinding>,
        random: &dyn RandomSource,
    ) -> Result<String, OAuthStoreError> {
        let transaction_id = random_b64(random)?;
        let client_record_id = client_record_id.to_owned();
        let redirect_uri = redirect_uri.to_owned();
        let resource = resource.to_owned();
        let issuer = issuer.to_owned();
        let pkce_s256 = pkce_s256.to_owned();
        let pkce_method = pkce_method.to_owned();
        let state = state.map(str::to_owned);
        let source = source.to_owned();
        self.mutate(|store, now| {
            if !store
                .clients
                .iter()
                .any(|client| client.id == client_record_id)
            {
                return Err(OAuthStoreError::TransactionNotFound);
            }
            let (pairing_window, reconnect_to) = store
                .pairing
                .as_ref()
                .filter(|pairing| {
                    pairing.expires_at > now
                        && !pairing.locked
                        && door.is_none_or(|binding| pairing_door_matches(pairing, binding))
                })
                .map(|pairing| (pairing.generation, pairing.reconnect_to.clone()))
                .ok_or(OAuthStoreError::NoActivePairing)?;
            if store.pending.len() >= MAX_PENDING
                || store
                    .pending
                    .iter()
                    .filter(|pending| pending.source == source)
                    .count()
                    >= MAX_PENDING_PER_SOURCE
            {
                return Err(OAuthStoreError::Quota);
            }
            store.pending.push(StoredPending {
                transaction_id: transaction_id.clone(),
                client_record_id,
                redirect_uri,
                resource,
                issuer,
                pkce_s256,
                pkce_method,
                state,
                source,
                created_at: now,
                expires_at: now + Duration::seconds(PENDING_TRANSACTION_TTL_SECS),
                failure_count: 0,
                authorization_code_verifier: None,
                code_expires_at: None,
                permission: None,
                generation,
                pairing_verified: false,
                pairing_window: Some(pairing_window),
                reconnect_to,
            });
            Ok(transaction_id)
        })
    }

    /// Consume the pairing code and issue a bound authorization code.
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn complete_pairing(
        &self,
        transaction_id: &str,
        pairing_code: &str,
        binding: &super::RuntimeBinding,
    ) -> Result<IssuedAuthorization, OAuthStoreError> {
        self.complete_pairing_with_permission(transaction_id, pairing_code, None, binding)
    }

    pub(crate) fn complete_pairing_with_permission(
        &self,
        transaction_id: &str,
        pairing_code: &str,
        permission: Option<ReadPermission>,
        binding: &super::RuntimeBinding,
    ) -> Result<IssuedAuthorization, OAuthStoreError> {
        self.complete_pairing_with_random_and_permission(
            transaction_id,
            pairing_code,
            permission,
            binding,
            &SystemRandomSource,
        )
    }

    fn complete_pairing_with_random_and_permission(
        &self,
        transaction_id: &str,
        pairing_code: &str,
        permission: Option<ReadPermission>,
        binding: &super::RuntimeBinding,
        random: &dyn RandomSource,
    ) -> Result<IssuedAuthorization, OAuthStoreError> {
        let presented = presented_pairing_digest(pairing_code);
        let code_bytes = random_bytes(random)?;
        self.mutate(|store, now| {
            let index = open_pending_index(store, transaction_id, binding, now)?;
            if store.pending[index].pairing_verified {
                return Err(OAuthStoreError::TransactionNotFound);
            }
            if store.pending[index].reconnect_to.is_some()
                || store
                    .pairing
                    .as_ref()
                    .is_some_and(|p| p.reconnect_to.is_some())
            {
                return Err(OAuthStoreError::BindingMismatch);
            }
            consume_pairing_code(store, index, presented, binding)?;
            Ok(issue_authorization(
                &mut store.pending[index],
                permission,
                &code_bytes,
                now,
            ))
        })
    }

    /// Consume the pairing code without issuing anything yet, so the requester
    /// can be shown the journal's facets only after proving they hold the code.
    ///
    /// The transaction gets a fresh identifier: the one on the page served
    /// before the code was checked can no longer finish it.
    pub(crate) fn verify_pairing(
        &self,
        transaction_id: &str,
        pairing_code: &str,
        binding: &super::RuntimeBinding,
    ) -> Result<String, OAuthStoreError> {
        self.verify_pairing_with_random(transaction_id, pairing_code, binding, &SystemRandomSource)
    }

    fn verify_pairing_with_random(
        &self,
        transaction_id: &str,
        pairing_code: &str,
        binding: &super::RuntimeBinding,
        random: &dyn RandomSource,
    ) -> Result<String, OAuthStoreError> {
        let presented = presented_pairing_digest(pairing_code);
        let verified_transaction_id = random_b64(random)?;
        self.mutate(|store, now| {
            let index = open_pending_index(store, transaction_id, binding, now)?;
            if store.pending[index].pairing_verified {
                return Err(OAuthStoreError::TransactionNotFound);
            }
            consume_pairing_code(store, index, presented, binding)?;
            let pending = &mut store.pending[index];
            pending.transaction_id = verified_transaction_id.clone();
            pending.pairing_verified = true;
            pending.expires_at = now + Duration::seconds(PENDING_TRANSACTION_TTL_SECS);
            Ok(verified_transaction_id)
        })
    }

    /// Issue the authorization code for a transaction whose pairing code was
    /// already verified.
    pub(crate) fn complete_verified_pairing(
        &self,
        transaction_id: &str,
        permission: ReadPermission,
        binding: &super::RuntimeBinding,
    ) -> Result<IssuedAuthorization, OAuthStoreError> {
        let code_bytes = random_bytes(&SystemRandomSource)?;
        self.mutate(|store, now| {
            let index = open_pending_index(store, transaction_id, binding, now)?;
            if !store.pending[index].pairing_verified {
                return Err(OAuthStoreError::TransactionNotFound);
            }
            if store.pending[index].reconnect_to.is_some() {
                return Err(OAuthStoreError::BindingMismatch);
            }
            Ok(issue_authorization(
                &mut store.pending[index],
                Some(permission),
                &code_bytes,
                now,
            ))
        })
    }

    /// Re-check target and complete a verified transaction that carries a reconnect target.
    pub(crate) fn complete_confirm_pairing(
        &self,
        transaction_id: &str,
        permission: Option<ReadPermission>,
        binding: &super::RuntimeBinding,
    ) -> Result<Option<IssuedAuthorization>, OAuthStoreError> {
        let code_bytes = random_bytes(&SystemRandomSource)?;
        self.edit(
            |store, now| {
                let index = match open_pending_index(store, transaction_id, binding, now) {
                    Ok(i) => i,
                    Err(e) => return (EditPersistence::Drop, Err(e)),
                };
                if !store.pending[index].pairing_verified {
                    return (
                        EditPersistence::Drop,
                        Err(OAuthStoreError::TransactionNotFound),
                    );
                }
                let Some(reconnect_target) = store.pending[index].reconnect_to.clone() else {
                    return (EditPersistence::Drop, Err(OAuthStoreError::BindingMismatch));
                };

                let target_gone = if let Some(oauth_id) = reconnect_target.strip_prefix("oauth:") {
                    !store.grants.iter().any(|g| g.id == oauth_id)
                } else if let Some(hex) = reconnect_target.strip_prefix("source:") {
                    match is_source_held_by_live_connection_locked(&self.root, store, hex) {
                        Ok(held) => held,
                        Err(e) => return (EditPersistence::Drop, Err(e)),
                    }
                } else {
                    true
                };

                if target_gone {
                    // End the transaction: the refusal page says this code won't work
                    // again, so posting the form again must not fall through to an
                    // untargeted pairing.
                    store.pending.remove(index);
                    return (EditPersistence::Persist, Err(OAuthStoreError::InvalidGrant));
                }

                let Some(permission) = permission else {
                    return (EditPersistence::Drop, Ok(None));
                };

                let issued = issue_authorization(
                    &mut store.pending[index],
                    Some(permission),
                    &code_bytes,
                    now,
                );
                (EditPersistence::Persist, Ok(Some(issued)))
            },
            || Ok(()),
        )
    }

    /// Read the reconnect target on an open pending transaction, if any.
    pub(crate) fn pending_reconnect_to(
        &self,
        transaction_id: &str,
        binding: &super::RuntimeBinding,
    ) -> Result<Option<String>, OAuthStoreError> {
        let store = self.read_store()?;
        let now = current_time();
        let Some(pending) = store.pending.iter().find(|pending| {
            pending.transaction_id == transaction_id
                && pending.resource == binding.canonical()
                && pending.generation == binding.stored_grant_generation()
                && pending.expires_at > now
        }) else {
            return Ok(None);
        };
        Ok(pending.reconnect_to.clone())
    }

    /// Whether the pairing code open for this door was made for an existing agent.
    pub(crate) fn open_pairing_is_targeted(
        &self,
        binding: &super::RuntimeBinding,
    ) -> Result<bool, OAuthStoreError> {
        let store = self.read_store()?;
        let now = current_time();
        Ok(store.pairing.as_ref().is_some_and(|pairing| {
            pairing.expires_at > now
                && !pairing.locked
                && pairing_door_matches(pairing, binding)
                && pairing.reconnect_to.is_some()
        }))
    }

    /// Classify a presented pairing code without consuming it or incrementing failures.
    pub(crate) fn classify_presented_code(
        &self,
        pairing_code: &str,
        binding: &super::RuntimeBinding,
    ) -> Result<CodeClassification, OAuthStoreError> {
        let store = self.read_store()?;
        let now = current_time();
        let Some(pairing) = store.pairing.as_ref() else {
            return Ok(CodeClassification::NoMatch);
        };
        if pairing.expires_at <= now || pairing.locked || !pairing_door_matches(pairing, binding) {
            return Ok(CodeClassification::NoMatch);
        }
        let presented = presented_pairing_digest(pairing_code);
        let Some(verifier) = decode_b64_32(&pairing.verifier) else {
            return Err(OAuthStoreError::Malformed {
                path: PathBuf::from(OAUTH_FILE),
            });
        };
        let matches = presented.is_some_and(|digest| bool::from(digest.ct_eq(&verifier)));
        if matches {
            Ok(CodeClassification::Matched {
                reconnect_to: pairing.reconnect_to.clone(),
            })
        } else {
            Ok(CodeClassification::NoMatch)
        }
    }

    /// Exchange a single-use authorization code for access and refresh tokens.
    pub(crate) fn redeem_authorization_code(
        &self,
        code: &str,
        client_id: &str,
        redirect_uri: &str,
        resource: &str,
        pkce_verifier: &str,
        binding: &super::RuntimeBinding,
    ) -> Result<IssuedTokens, OAuthStoreError> {
        self.redeem_authorization_code_with_random(
            code,
            client_id,
            redirect_uri,
            resource,
            pkce_verifier,
            binding,
            &SystemRandomSource,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn redeem_authorization_code_with_random(
        &self,
        code: &str,
        client_id: &str,
        redirect_uri: &str,
        resource: &str,
        pkce_verifier: &str,
        binding: &super::RuntimeBinding,
        random: &dyn RandomSource,
    ) -> Result<IssuedTokens, OAuthStoreError> {
        let presented = decode_sha256(code).ok_or(OAuthStoreError::InvalidToken)?;
        let has_reconnect = {
            let store = self.read_store()?;
            store.pending.iter().any(|pending| {
                pending.reconnect_to.is_some()
                    && pending
                        .authorization_code_verifier
                        .as_ref()
                        .is_some_and(|verifier| {
                            decode_b64_32(verifier).is_some_and(|d| bool::from(presented.ct_eq(&d)))
                        })
            })
        };
        if has_reconnect {
            return self.redeem_authorization_code_takeover(
                code,
                client_id,
                redirect_uri,
                resource,
                pkce_verifier,
                binding,
                random,
            );
        }
        let access_bytes = random_bytes(random)?;
        let refresh_bytes = random_bytes(random)?;
        let id_bytes = random_bytes(random)?;
        let pkce_digest = sha256_digest(pkce_verifier.as_bytes());
        let mut granted_permission = None;
        let issued = self.mutate(|store, now| {
            let mut matched = None;
            for (index, pending) in store.pending.iter().enumerate() {
                let Some(verifier) = pending.authorization_code_verifier.as_ref() else {
                    continue;
                };
                let verifier =
                    decode_b64_32(verifier).ok_or_else(|| OAuthStoreError::Malformed {
                        path: PathBuf::from(OAUTH_FILE),
                    })?;
                if bool::from(presented.ct_eq(&verifier)) && matched.is_none() {
                    matched = Some(index);
                }
            }
            let index = matched.ok_or(OAuthStoreError::InvalidToken)?;
            if store.pending[index].reconnect_to.is_some() {
                return Err(OAuthStoreError::InvalidGrant);
            }
            let pending = store.pending.remove(index);
            granted_permission = pending.permission.clone();
            let code_expires_at = pending
                .code_expires_at
                .ok_or(OAuthStoreError::CodeExpired)?;
            if code_expires_at <= now {
                return Err(OAuthStoreError::CodeExpired);
            }
            let client = store
                .clients
                .iter()
                .find(|client| client.id == pending.client_record_id)
                .ok_or(OAuthStoreError::BindingMismatch)?;
            let stored_challenge = decode_b64_32(&pending.pkce_s256);
            if client.client_id != client_id
                || pending.redirect_uri != redirect_uri
                || pending.resource != resource
                || pending.resource != binding.canonical()
                || pending.generation != binding.stored_grant_generation()
                || pending.pkce_method != "S256"
                || stored_challenge
                    .is_none_or(|challenge| !bool::from(pkce_digest.ct_eq(&challenge)))
            {
                return Err(OAuthStoreError::BindingMismatch);
            }
            if store.grants.len() >= MAX_GRANTS {
                return Err(OAuthStoreError::Quota);
            }
            let grant_id = URL_SAFE_NO_PAD.encode(id_bytes);
            let client_record_id = client.id.clone();
            let stored_client_id = client.client_id.clone();
            let revocation_generation = client.revocation_generation;
            // A redemption sets `last_used_at`, and a client record that has
            // one is never pruned, so its absence across every record with
            // this id is the store's own fact that none was granted before.
            let first_grant =
                (!store.clients.iter().any(|known| {
                    known.client_id == stored_client_id && known.last_used_at.is_some()
                }) && !store
                    .grants
                    .iter()
                    .any(|grant| grant.client_id == stored_client_id))
                .then(|| FirstGrant {
                    client_name: client.client_name.clone(),
                });
            store.grants.push(StoredGrant {
                id: grant_id.clone(),
                client_record_id,
                client_id: stored_client_id,
                access_verifier: sha256_b64(&access_bytes),
                refresh_verifier: sha256_b64(&refresh_bytes),
                refresh_generation: 0,
                revocation_generation,
                access_expires_at: now + Duration::seconds(ACCESS_TTL_SECS),
                refresh_expires_at: now + Duration::seconds(REFRESH_TTL_SECS),
                created_at: now,
                resource: binding.stored_grant_resource(),
                generation: pending.generation,
            });
            if let Some(client) = store
                .clients
                .iter_mut()
                .find(|client| client.id == pending.client_record_id)
            {
                client.last_used_at = Some(now);
            }
            Ok(IssuedTokens {
                access_token: URL_SAFE_NO_PAD.encode(access_bytes),
                refresh_token: URL_SAFE_NO_PAD.encode(refresh_bytes),
                token_id: grant_id,
                expires_in: ACCESS_TTL_SECS,
                first_grant,
            })
        })?;
        if let Some(permission) = granted_permission
            && crate::permissions::PermissionStore::open(&self.root)
                .set_permission(&format!("oauth:{}", issued.token_id), permission)
                .is_err()
        {
            let _ = self.revoke_grant_by_id(&issued.token_id);
            return Err(OAuthStoreError::Permission);
        }
        Ok(issued)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn redeem_authorization_code_takeover(
        &self,
        code: &str,
        client_id: &str,
        redirect_uri: &str,
        resource: &str,
        pkce_verifier: &str,
        binding: &super::RuntimeBinding,
        random: &dyn RandomSource,
    ) -> Result<IssuedTokens, OAuthStoreError> {
        let presented = decode_sha256(code).ok_or(OAuthStoreError::InvalidToken)?;
        let access_bytes = random_bytes(random)?;
        let refresh_bytes = random_bytes(random)?;
        let id_bytes = random_bytes(random)?;
        let pkce_digest = sha256_digest(pkce_verifier.as_bytes());

        let edit_res = self.edit(
            |store, now| {
                let mut matched = None;
                for (index, pending) in store.pending.iter().enumerate() {
                    let Some(verifier) = pending.authorization_code_verifier.as_ref() else {
                        continue;
                    };
                    let verifier = match decode_b64_32(verifier) {
                        Some(v) => v,
                        None => {
                            return (
                                EditPersistence::Drop,
                                Err(OAuthStoreError::Malformed {
                                    path: PathBuf::from(OAUTH_FILE),
                                }),
                            );
                        }
                    };
                    if bool::from(presented.ct_eq(&verifier)) && matched.is_none() {
                        matched = Some(index);
                    }
                }
                let index = match matched {
                    Some(idx) => idx,
                    None => return (EditPersistence::Drop, Err(OAuthStoreError::InvalidToken)),
                };

                let pending = &store.pending[index];
                let code_expires_at = match pending.code_expires_at {
                    Some(exp) => exp,
                    None => return (EditPersistence::Drop, Err(OAuthStoreError::CodeExpired)),
                };
                if code_expires_at <= now {
                    return (EditPersistence::Drop, Err(OAuthStoreError::CodeExpired));
                }
                let client = match store
                    .clients
                    .iter()
                    .find(|client| client.id == pending.client_record_id)
                {
                    Some(c) => c,
                    None => return (EditPersistence::Drop, Err(OAuthStoreError::BindingMismatch)),
                };
                let stored_challenge = decode_b64_32(&pending.pkce_s256);
                if client.client_id != client_id
                    || pending.redirect_uri != redirect_uri
                    || pending.resource != resource
                    || pending.resource != binding.canonical()
                    || pending.generation != binding.stored_grant_generation()
                    || pending.pkce_method != "S256"
                    || stored_challenge
                        .is_none_or(|challenge| !bool::from(pkce_digest.ct_eq(&challenge)))
                {
                    return (EditPersistence::Drop, Err(OAuthStoreError::BindingMismatch));
                }

                let target = match &pending.reconnect_to {
                    Some(t) => t.clone(),
                    None => return (EditPersistence::Drop, Err(OAuthStoreError::InvalidGrant)),
                };

                if let Some(target_grant_id) = target.strip_prefix("oauth:") {
                    let target_grant_id = target_grant_id.to_string();
                    let grant_exists = store.grants.iter().any(|g| g.id == target_grant_id);
                    if !grant_exists {
                        store.pending.remove(index);
                        return (EditPersistence::Persist, Err(OAuthStoreError::InvalidGrant));
                    }

                    // Existing grant Step 1:
                    let conn_key = format!("oauth:{target_grant_id}");
                    let current_source =
                        match crate::agent_state::memory_source(&self.root, &conn_key) {
                            Ok(src) => src,
                            Err(e) => {
                                return (EditPersistence::Drop, Err(OAuthStoreError::from(e)));
                            }
                        };
                    if let Err(e) = crate::agent_state::set_memory_source(
                        &self.root,
                        &conn_key,
                        current_source.component(),
                    ) {
                        return (EditPersistence::Drop, Err(OAuthStoreError::from(e)));
                    }
                    trigger_i4_hold(&self.root);
                    if check_takeover_stop(1).is_err() {
                        return (EditPersistence::Drop, Err(OAuthStoreError::InvalidGrant));
                    }

                    // Existing grant Step 2:
                    let is_paused =
                        match crate::agent_state::is_owner_paused_locked(&self.root, &conn_key) {
                            Ok(paused) => paused,
                            Err(e) => {
                                return (EditPersistence::Drop, Err(OAuthStoreError::from(e)));
                            }
                        };

                    let grant = match store.grants.iter_mut().find(|g| g.id == target_grant_id) {
                        Some(g) => g,
                        None => return (EditPersistence::Drop, Err(OAuthStoreError::InvalidGrant)),
                    };
                    grant.access_verifier = sha256_b64(&access_bytes);
                    grant.refresh_verifier = sha256_b64(&refresh_bytes);
                    grant.refresh_generation = 0;
                    grant.revocation_generation = client.revocation_generation;
                    grant.client_record_id = client.id.clone();
                    grant.client_id = client.client_id.clone();
                    grant.created_at = now;
                    grant.access_expires_at = now + Duration::seconds(ACCESS_TTL_SECS);
                    grant.resource = binding.stored_grant_resource();
                    grant.generation = pending.generation;
                    if !is_paused && grant.refresh_expires_at > now {
                        grant.refresh_expires_at = now;
                    }

                    let pending_client_record_id = pending.client_record_id.clone();
                    if let Some(c) = store
                        .clients
                        .iter_mut()
                        .find(|c| c.id == pending_client_record_id)
                    {
                        c.last_used_at = Some(now);
                    }
                    let pending_permission = pending.permission.clone();

                    (
                        EditPersistence::Persist,
                        Ok(TakeoverFirstEditResult::ExistingGrant {
                            grant_id: target_grant_id,
                            is_paused,
                            pending_permission,
                        }),
                    )
                } else if let Some(hex) = target.strip_prefix("source:") {
                    let hex = hex.to_string();
                    let held =
                        match is_source_held_by_live_connection_locked(&self.root, store, &hex) {
                            Ok(held) => held,
                            Err(e) => return (EditPersistence::Drop, Err(e)),
                        };
                    if held {
                        store.pending.remove(index);
                        return (EditPersistence::Persist, Err(OAuthStoreError::InvalidGrant));
                    }
                    if store.grants.len() >= MAX_GRANTS {
                        return (EditPersistence::Drop, Err(OAuthStoreError::Quota));
                    }

                    // Earlier source Step 1:
                    let grant_id = URL_SAFE_NO_PAD.encode(id_bytes);
                    let conn_key = format!("oauth:{grant_id}");
                    if let Err(e) =
                        crate::agent_state::set_memory_source(&self.root, &conn_key, &hex)
                    {
                        return (EditPersistence::Drop, Err(OAuthStoreError::from(e)));
                    }
                    trigger_i4_hold(&self.root);
                    if check_takeover_stop(1).is_err() {
                        return (EditPersistence::Drop, Err(OAuthStoreError::InvalidGrant));
                    }

                    // Earlier source Step 2:
                    let pending = store.pending.remove(index);
                    let client_record_id = client.id.clone();
                    let stored_client_id = client.client_id.clone();
                    let revocation_generation = client.revocation_generation;
                    store.grants.push(StoredGrant {
                        id: grant_id.clone(),
                        client_record_id,
                        client_id: stored_client_id,
                        access_verifier: sha256_b64(&access_bytes),
                        refresh_verifier: sha256_b64(&refresh_bytes),
                        refresh_generation: 0,
                        revocation_generation,
                        access_expires_at: now + Duration::seconds(ACCESS_TTL_SECS),
                        refresh_expires_at: now + Duration::seconds(REFRESH_TTL_SECS),
                        created_at: now,
                        resource: binding.stored_grant_resource(),
                        generation: pending.generation,
                    });
                    if let Some(c) = store
                        .clients
                        .iter_mut()
                        .find(|c| c.id == pending.client_record_id)
                    {
                        c.last_used_at = Some(now);
                    }

                    (
                        EditPersistence::Persist,
                        Ok(TakeoverFirstEditResult::EarlierSource {
                            grant_id,
                            permission: pending.permission,
                        }),
                    )
                } else {
                    (EditPersistence::Drop, Err(OAuthStoreError::InvalidGrant))
                }
            },
            || Ok(()),
        )?;

        check_takeover_stop(2)?;

        match edit_res {
            TakeoverFirstEditResult::ExistingGrant {
                grant_id,
                is_paused,
                pending_permission,
            } => {
                let conn_key = format!("oauth:{grant_id}");
                // Step 3: Permission row, same oauth:<id> key.
                let is_paused_now =
                    crate::agent_state::is_owner_paused_locked(&self.root, &conn_key)
                        .map_err(OAuthStoreError::from)?;
                let eval = if is_paused_now { "hold" } else { "enforce" };
                // A confirmed takeover always carries the owner's choice. Without one,
                // stop here, in an allowed state: taken over and still paused.
                let Some(perm) = pending_permission else {
                    return Err(OAuthStoreError::InvalidGrant);
                };
                crate::permissions::PermissionStore::open(&self.root)
                    .update_permission_preserving_generation(&conn_key, perm, eval)
                    .map_err(|_| OAuthStoreError::Permission)?;
                check_takeover_stop(3)?;

                // Step 4: A second edit sets refresh_expires_at = now + REFRESH_TTL_SECS only.
                self.edit(
                    |store, now| {
                        let Some(grant) = store.grants.iter_mut().find(|g| g.id == grant_id) else {
                            return (EditPersistence::Drop, Err(OAuthStoreError::InvalidGrant));
                        };
                        grant.refresh_expires_at = now + Duration::seconds(REFRESH_TTL_SECS);
                        (EditPersistence::Persist, Ok(()))
                    },
                    || Ok(()),
                )?;
                check_takeover_stop(4)?;

                // Step 5 & 6: Owner pause only
                if is_paused || is_paused_now {
                    crate::agent_state::clear_owner_pause(&self.root, &conn_key)
                        .map_err(OAuthStoreError::from)?;
                    check_takeover_stop(5)?;
                    crate::permissions::PermissionStore::open(&self.root)
                        .set_evaluation_only(&conn_key, "enforce")
                        .map_err(|_| OAuthStoreError::Permission)?;
                    check_takeover_stop(6)?;
                }

                // Remove the pending row
                self.edit(
                    |store, _now| {
                        if let Some(pos) = store.pending.iter().position(|p| {
                            p.authorization_code_verifier.as_ref().is_some_and(|v| {
                                decode_b64_32(v).is_some_and(|d| bool::from(presented.ct_eq(&d)))
                            })
                        }) {
                            store.pending.remove(pos);
                            (EditPersistence::Persist, Ok(()))
                        } else {
                            (EditPersistence::Drop, Ok(()))
                        }
                    },
                    || Ok(()),
                )?;

                Ok(IssuedTokens {
                    access_token: URL_SAFE_NO_PAD.encode(access_bytes),
                    refresh_token: URL_SAFE_NO_PAD.encode(refresh_bytes),
                    token_id: grant_id,
                    expires_in: ACCESS_TTL_SECS,
                    first_grant: None,
                })
            }
            TakeoverFirstEditResult::EarlierSource {
                grant_id,
                permission,
            } => {
                let conn_key = format!("oauth:{grant_id}");
                if let Some(perm) = permission
                    && crate::permissions::PermissionStore::open(&self.root)
                        .set_permission(&conn_key, perm)
                        .is_err()
                {
                    let _ = self.revoke_grant_by_id(&grant_id);
                    return Err(OAuthStoreError::Permission);
                }
                Ok(IssuedTokens {
                    access_token: URL_SAFE_NO_PAD.encode(access_bytes),
                    refresh_token: URL_SAFE_NO_PAD.encode(refresh_bytes),
                    token_id: grant_id,
                    expires_in: ACCESS_TTL_SECS,
                    first_grant: None,
                })
            }
        }
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn redeem_takeover_without_oauth_lock(
        &self,
        code: &str,
        client_id: &str,
        redirect_uri: &str,
        resource: &str,
        pkce_verifier: &str,
        binding: &super::RuntimeBinding,
    ) -> Result<IssuedTokens, OAuthStoreError> {
        self.redeem_takeover_without_oauth_lock_with_random(
            code,
            client_id,
            redirect_uri,
            resource,
            pkce_verifier,
            binding,
            &SystemRandomSource,
        )
    }

    #[cfg(test)]
    #[allow(dead_code, clippy::too_many_arguments)]
    pub(crate) fn redeem_takeover_without_oauth_lock_with_random(
        &self,
        code: &str,
        client_id: &str,
        redirect_uri: &str,
        resource: &str,
        pkce_verifier: &str,
        binding: &super::RuntimeBinding,
        random: &dyn RandomSource,
    ) -> Result<IssuedTokens, OAuthStoreError> {
        let presented = decode_sha256(code).ok_or(OAuthStoreError::InvalidToken)?;
        let access_bytes = random_bytes(random)?;
        let refresh_bytes = random_bytes(random)?;
        let id_bytes = random_bytes(random)?;
        let pkce_digest = sha256_digest(pkce_verifier.as_bytes());

        let mut store = self.read_store()?;
        let now = current_time();

        let mut matched = None;
        for (index, pending) in store.pending.iter().enumerate() {
            let Some(verifier) = pending.authorization_code_verifier.as_ref() else {
                continue;
            };
            let verifier = decode_b64_32(verifier).ok_or_else(|| OAuthStoreError::Malformed {
                path: PathBuf::from(OAUTH_FILE),
            })?;
            if bool::from(presented.ct_eq(&verifier)) && matched.is_none() {
                matched = Some(index);
            }
        }
        let index = matched.ok_or(OAuthStoreError::InvalidToken)?;
        let pending = &store.pending[index];
        let code_expires_at = pending
            .code_expires_at
            .ok_or(OAuthStoreError::CodeExpired)?;
        if code_expires_at <= now {
            return Err(OAuthStoreError::CodeExpired);
        }
        let client = store
            .clients
            .iter()
            .find(|client| client.id == pending.client_record_id)
            .ok_or(OAuthStoreError::BindingMismatch)?;
        let stored_challenge = decode_b64_32(&pending.pkce_s256);
        if client.client_id != client_id
            || pending.redirect_uri != redirect_uri
            || pending.resource != resource
            || pending.resource != binding.canonical()
            || pending.generation != binding.stored_grant_generation()
            || pending.pkce_method != "S256"
            || stored_challenge.is_none_or(|challenge| !bool::from(pkce_digest.ct_eq(&challenge)))
        {
            return Err(OAuthStoreError::BindingMismatch);
        }

        let target = pending
            .reconnect_to
            .clone()
            .ok_or(OAuthStoreError::InvalidGrant)?;

        if let Some(target_grant_id) = target.strip_prefix("oauth:") {
            let target_grant_id = target_grant_id.to_string();
            let grant_exists = store.grants.iter().any(|g| g.id == target_grant_id);
            if !grant_exists {
                store.pending.remove(index);
                self.write_store(&self.oauth_path(), &mut store)?;
                return Err(OAuthStoreError::InvalidGrant);
            }

            let conn_key = format!("oauth:{target_grant_id}");
            let current_source = crate::agent_state::memory_source(&self.root, &conn_key)
                .map_err(OAuthStoreError::from)?;
            crate::agent_state::set_memory_source(
                &self.root,
                &conn_key,
                current_source.component(),
            )
            .map_err(OAuthStoreError::from)?;
            trigger_i4_hold(&self.root);

            let is_paused = crate::agent_state::is_owner_paused_locked(&self.root, &conn_key)
                .map_err(OAuthStoreError::from)?;
            let grant = store
                .grants
                .iter_mut()
                .find(|g| g.id == target_grant_id)
                .ok_or(OAuthStoreError::InvalidGrant)?;
            grant.access_verifier = sha256_b64(&access_bytes);
            grant.refresh_verifier = sha256_b64(&refresh_bytes);
            grant.refresh_generation = 0;
            grant.revocation_generation = client.revocation_generation;
            grant.client_record_id = client.id.clone();
            grant.client_id = client.client_id.clone();
            grant.created_at = now;
            grant.access_expires_at = now + Duration::seconds(ACCESS_TTL_SECS);
            grant.resource = binding.stored_grant_resource();
            grant.generation = pending.generation;
            if !is_paused && grant.refresh_expires_at > now {
                grant.refresh_expires_at = now;
            }
            if let Some(c) = store
                .clients
                .iter_mut()
                .find(|c| c.id == pending.client_record_id)
            {
                c.last_used_at = Some(now);
            }
            self.write_store(&self.oauth_path(), &mut store)?;

            Ok(IssuedTokens {
                access_token: URL_SAFE_NO_PAD.encode(access_bytes),
                refresh_token: URL_SAFE_NO_PAD.encode(refresh_bytes),
                token_id: target_grant_id,
                expires_in: ACCESS_TTL_SECS,
                first_grant: None,
            })
        } else if let Some(hex) = target.strip_prefix("source:") {
            let hex = hex.to_string();
            let held = is_source_held_by_live_connection_locked(&self.root, &store, &hex)?;
            if held {
                store.pending.remove(index);
                self.write_store(&self.oauth_path(), &mut store)?;
                return Err(OAuthStoreError::InvalidGrant);
            }

            let grant_id = URL_SAFE_NO_PAD.encode(id_bytes);
            let conn_key = format!("oauth:{grant_id}");
            crate::agent_state::set_memory_source(&self.root, &conn_key, &hex)
                .map_err(OAuthStoreError::from)?;
            trigger_i4_hold(&self.root);

            let pending = store.pending.remove(index);
            let client_record_id = client.id.clone();
            let stored_client_id = client.client_id.clone();
            let revocation_generation = client.revocation_generation;
            store.grants.push(StoredGrant {
                id: grant_id.clone(),
                client_record_id,
                client_id: stored_client_id,
                access_verifier: sha256_b64(&access_bytes),
                refresh_verifier: sha256_b64(&refresh_bytes),
                refresh_generation: 0,
                revocation_generation,
                access_expires_at: now + Duration::seconds(ACCESS_TTL_SECS),
                refresh_expires_at: now + Duration::seconds(REFRESH_TTL_SECS),
                created_at: now,
                resource: binding.stored_grant_resource(),
                generation: pending.generation,
            });
            if let Some(c) = store
                .clients
                .iter_mut()
                .find(|c| c.id == pending.client_record_id)
            {
                c.last_used_at = Some(now);
            }
            if let Ok(on_disk) = self.read_store() {
                for g in on_disk.grants {
                    if !store.grants.iter().any(|existing| existing.id == g.id) {
                        store.grants.push(g);
                    }
                }
            }
            self.write_store(&self.oauth_path(), &mut store)?;
            if let Some(perm) = pending.permission {
                let _ = crate::permissions::PermissionStore::open(&self.root)
                    .set_permission(&conn_key, perm);
            }
            Ok(IssuedTokens {
                access_token: URL_SAFE_NO_PAD.encode(access_bytes),
                refresh_token: URL_SAFE_NO_PAD.encode(refresh_bytes),
                token_id: grant_id,
                expires_in: ACCESS_TTL_SECS,
                first_grant: None,
            })
        } else {
            Err(OAuthStoreError::InvalidGrant)
        }
    }

    /// Rotate a refresh token and issue a new access/refresh pair.
    pub(crate) fn refresh_grant(
        &self,
        refresh_token: &str,
        client_id: &str,
        binding: &super::RuntimeBinding,
    ) -> Result<IssuedTokens, OAuthStoreError> {
        self.refresh_grant_with_random(refresh_token, client_id, binding, &SystemRandomSource)
    }

    pub(crate) fn refresh_grant_with_random(
        &self,
        refresh_token: &str,
        client_id: &str,
        binding: &super::RuntimeBinding,
        random: &dyn RandomSource,
    ) -> Result<IssuedTokens, OAuthStoreError> {
        let presented_raw = URL_SAFE_NO_PAD
            .decode(refresh_token)
            .map_err(|_| OAuthStoreError::InvalidToken)?;
        let presented = sha256_digest(&presented_raw);
        let replay_notice = RefCell::new(None);
        let revoked_grant_id = RefCell::new(None);
        let oauth_written = Cell::new(false);
        let result = self.edit(
            |store, now| {
                let mut matched = None;
                for (index, grant) in store.grants.iter().enumerate() {
                    let Some(parsed) = parse_refresh_verifier(&grant.refresh_verifier) else {
                        return (
                            EditPersistence::Drop,
                            Err(OAuthStoreError::Malformed {
                                path: PathBuf::from(OAUTH_FILE),
                            }),
                        );
                    };
                    if matched.is_none() {
                        if bool::from(presented.ct_eq(&parsed.current_verifier)) {
                            matched = Some((index, None, parsed));
                        } else if let Some(history_index) = parsed
                            .history
                            .iter()
                            .position(|entry| bool::from(presented.ct_eq(&entry.verifier)))
                        {
                            matched = Some((index, Some(history_index), parsed));
                        }
                    }
                }
                let Some((index, history_index, parsed)) = matched else {
                    return (EditPersistence::Drop, Err(OAuthStoreError::InvalidToken));
                };
                let client = store
                    .clients
                    .iter()
                    .find(|client| client.id == store.grants[index].client_record_id);
                let Some(client) = client else {
                    return (EditPersistence::Drop, Err(OAuthStoreError::InvalidToken));
                };
                if store.grants[index].client_id != client_id
                    || store.grants[index].revocation_generation != client.revocation_generation
                    || !binding.grant_matches(
                        &store.grants[index].resource,
                        store.grants[index].generation,
                    )
                {
                    return (EditPersistence::Drop, Err(OAuthStoreError::InvalidToken));
                }

                if let Some(history_index) = history_index {
                    let history = &parsed.history[history_index];
                    let is_grace = history_index == 0
                        && now.timestamp() <= history.rotated_at.saturating_add(REFRESH_GRACE_SECS);
                    // The rotation that retired this token derived the current pair
                    // from it, so re-deriving reproduces that pair exactly. Anything
                    // that does not reproduce it is treated as a replay.
                    let grace_pair = parsed.secret.filter(|_| is_grace).and_then(|secret| {
                        let generation = store.grants[index].refresh_generation;
                        let pair = derive_token_pair(&secret, generation, &presented_raw);
                        bool::from(sha256_digest(&pair.1).ct_eq(&parsed.current_verifier))
                            .then_some(pair)
                    });
                    if let Some((access_bytes, refresh_bytes)) = grace_pair {
                        return (
                            EditPersistence::Drop,
                            Ok(IssuedTokens {
                                access_token: URL_SAFE_NO_PAD.encode(access_bytes),
                                refresh_token: URL_SAFE_NO_PAD.encode(refresh_bytes),
                                token_id: store.grants[index].id.clone(),
                                expires_in: ACCESS_TTL_SECS,
                                first_grant: None,
                            }),
                        );
                    }

                    let grant_client_id = store.grants[index].client_id.clone();
                    let notice_name = client.client_name.clone().unwrap_or(grant_client_id);
                    let grant = store.grants.remove(index);
                    let notice = ReplayNotice {
                        id: grant.id.clone(),
                        name: notice_name,
                        door: replay_door_label(grant.resource.as_deref()),
                        dismissed: false,
                    };
                    *replay_notice.borrow_mut() = Some(notice);
                    *revoked_grant_id.borrow_mut() = Some(grant.id.clone());
                    return (
                        EditPersistence::Persist,
                        Err(OAuthStoreError::RefreshReused { grant_id: grant.id }),
                    );
                }

                let old_secret = parsed.secret;
                let secret = match old_secret {
                    Some(secret) => secret,
                    None => match random_bytes(random) {
                        Ok(secret) => secret,
                        Err(error) => return (EditPersistence::Drop, Err(error)),
                    },
                };
                let grant = &mut store.grants[index];
                let generation = grant.refresh_generation.saturating_add(1);
                let (access_bytes, refresh_bytes) =
                    derive_token_pair(&secret, generation, &presented_raw);
                let mut history = parsed.history;
                history.insert(
                    0,
                    RefreshHistoryEntry {
                        verifier: parsed.current_verifier,
                        verifier_b64: parsed.current_verifier_b64,
                        rotated_at: now.timestamp(),
                        derived: old_secret.is_some(),
                    },
                );
                grant.access_verifier = sha256_b64(&access_bytes);
                grant.refresh_verifier =
                    write_refresh_verifier(&sha256_b64(&refresh_bytes), &secret, &history);
                grant.refresh_generation = generation;
                grant.access_expires_at = now + Duration::seconds(ACCESS_TTL_SECS);
                let connection = format!("oauth:{}", grant.id);
                let is_owner_paused =
                    crate::agent_state::is_owner_paused_locked(&self.root, &connection)
                        .unwrap_or(true);
                if !is_owner_paused && grant.refresh_expires_at > now {
                    grant.refresh_expires_at = now + Duration::seconds(REFRESH_TTL_SECS);
                }
                (
                    EditPersistence::Persist,
                    Ok(IssuedTokens {
                        access_token: URL_SAFE_NO_PAD.encode(access_bytes),
                        refresh_token: URL_SAFE_NO_PAD.encode(refresh_bytes),
                        token_id: grant.id.clone(),
                        expires_in: ACCESS_TTL_SECS,
                        first_grant: None,
                    }),
                )
            },
            || {
                oauth_written.set(true);
                if let Some(notice) = replay_notice.borrow_mut().take() {
                    // The revocation is already on disk; losing the notice must not
                    // turn the client's refusal into a server error.
                    if self.append_replay_notice_locked(notice).is_err() {
                        log::warn!("oauth: replay notice could not be saved");
                    }
                }
                Ok(())
            },
        );
        if oauth_written.get()
            && let Some(grant_id) = revoked_grant_id.borrow_mut().take()
        {
            let _ = crate::permissions::PermissionStore::open(&self.root)
                .remove_connection(&format!("oauth:{grant_id}"));
        }
        result
    }

    /// Register a CIMD client, returning the existing record when the URL matches.
    #[cfg(test)]
    pub(crate) fn register_client(
        &self,
        client_id: &str,
        redirect_uris: Vec<String>,
        client_name: Option<String>,
        source: &str,
    ) -> Result<RegisteredClient, OAuthStoreError> {
        self.register_client_with_random(
            client_id,
            redirect_uris,
            client_name,
            source,
            &SystemRandomSource,
        )
    }

    pub(crate) fn register_client_with_random(
        &self,
        client_id: &str,
        redirect_uris: Vec<String>,
        client_name: Option<String>,
        source: &str,
        random: &dyn RandomSource,
    ) -> Result<RegisteredClient, OAuthStoreError> {
        let id = random_b64(random)?;
        let client_id_owned = client_id.to_owned();
        let source = source.to_owned();
        let mut evicted_grant_ids = Vec::new();
        let registered = self.mutate(|store, now| {
            let is_byo = source.starts_with("byo:");
            if let Some(existing) = store.clients.iter().find(|client| {
                client.client_id == client_id_owned && (client.source.starts_with("byo:") == is_byo)
            }) {
                return Ok(registered_from(existing));
            }

            let is_idle = |client: &StoredClient, store: &OAuthStoreFile| -> bool {
                let has_live_grant = store.grants.iter().any(|g| {
                    g.client_record_id == client.id
                        && g.revocation_generation == client.revocation_generation
                });
                if has_live_grant {
                    return false;
                }
                let has_live_pending = store.pending.iter().any(|p| {
                    p.client_record_id == client.id
                        && if p.authorization_code_verifier.is_some() {
                            p.code_expires_at.is_some_and(|exp| exp > now)
                        } else {
                            p.expires_at > now
                        }
                });
                if has_live_pending {
                    return false;
                }
                true
            };

            let source_count = store.clients.iter().filter(|c| c.source == source).count();
            let mut evicted_target = None;

            if is_byo {
                let byo_count = store
                    .clients
                    .iter()
                    .filter(|c| c.source.starts_with("byo:"))
                    .count();
                if byo_count >= MAX_CLIENTS {
                    let oldest_byo_idle = store
                        .clients
                        .iter()
                        .enumerate()
                        .filter(|(_, c)| c.source.starts_with("byo:") && is_idle(c, store))
                        .min_by_key(|(index, c)| (c.created_at, *index))
                        .map(|(_, c)| c.id.clone());
                    let Some(target_id) = oldest_byo_idle else {
                        return Err(OAuthStoreError::Quota);
                    };
                    evicted_target = Some(target_id);
                }
            } else if source_count >= MAX_CLIENTS_PER_SOURCE {
                let oldest_source_idle = store
                    .clients
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.source == source && is_idle(c, store))
                    .min_by_key(|(index, c)| (c.created_at, *index))
                    .map(|(_, c)| c.id.clone());
                let Some(target_id) = oldest_source_idle else {
                    return Err(OAuthStoreError::Quota);
                };
                evicted_target = Some(target_id);
            } else {
                let local_count = store
                    .clients
                    .iter()
                    .filter(|c| !c.source.starts_with("byo:"))
                    .count();
                if local_count >= MAX_CLIENTS {
                    let oldest_local_idle = store
                        .clients
                        .iter()
                        .enumerate()
                        .filter(|(_, c)| !c.source.starts_with("byo:") && is_idle(c, store))
                        .min_by_key(|(index, c)| (c.created_at, *index))
                        .map(|(_, c)| c.id.clone());
                    let Some(target_id) = oldest_local_idle else {
                        return Err(OAuthStoreError::Quota);
                    };
                    evicted_target = Some(target_id);
                }
            }

            if let Some(target_id) = evicted_target {
                evicted_grant_ids = store
                    .grants
                    .iter()
                    .filter(|g| g.client_record_id == target_id)
                    .map(|g| g.id.clone())
                    .collect();
                store.grants.retain(|g| g.client_record_id != target_id);
                store.clients.retain(|c| c.id != target_id);
            }

            store.clients.push(StoredClient {
                id: id.clone(),
                client_id: client_id_owned,
                redirect_uris,
                client_name,
                source,
                created_at: now,
                last_used_at: None,
                revocation_generation: 0,
            });
            Ok(registered_from(
                store.clients.last().expect("client inserted"),
            ))
        })?;

        let perm_store = crate::permissions::PermissionStore::open(&self.root);
        for gid in evicted_grant_ids {
            let _ = perm_store.remove_connection(&format!("oauth:{gid}"));
        }

        Ok(registered)
    }

    /// Resolve the client and return target bound to a pending authorization.
    pub(crate) fn pending_authorization(
        &self,
        transaction_id: &str,
        binding: &super::RuntimeBinding,
    ) -> Result<Option<PendingAuthorization>, OAuthStoreError> {
        let store = self.read_store()?;
        let Some(pending) = store.pending.iter().find(|pending| {
            pending.transaction_id == transaction_id
                && pending.resource == binding.canonical()
                && pending.generation == binding.stored_grant_generation()
        }) else {
            return Ok(None);
        };
        let client = store
            .clients
            .iter()
            .find(|client| client.id == pending.client_record_id)
            .ok_or(OAuthStoreError::ClientNotFound)?;
        Ok(Some(PendingAuthorization {
            client: registered_from(client),
            redirect_uri: pending.redirect_uri.clone(),
            pairing_verified: pending.pairing_verified,
            reconnect_to: pending.reconnect_to.clone(),
        }))
    }

    /// Look up a registered client by CIMD URL.
    #[allow(dead_code)]
    pub(crate) fn lookup_client_by_cimd_url(
        &self,
        client_id: &str,
    ) -> Result<Option<RegisteredClient>, OAuthStoreError> {
        self.lookup_client_by_cimd_url_in_cohort(client_id, false)
    }

    pub(crate) fn lookup_client_by_cimd_url_in_cohort(
        &self,
        client_id: &str,
        is_byo: bool,
    ) -> Result<Option<RegisteredClient>, OAuthStoreError> {
        Ok(self
            .read_store()?
            .clients
            .into_iter()
            .find(|client| {
                client.client_id == client_id && (client.source.starts_with("byo:") == is_byo)
            })
            .map(|client| registered_from(&client)))
    }

    /// Invalidate outstanding tokens for one client without deleting its record.
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn revoke_client(&self, client_record_id: &str) -> Result<(), OAuthStoreError> {
        let mut grant_ids = Vec::new();
        self.mutate(|store, _now| {
            let client = store
                .clients
                .iter_mut()
                .find(|client| client.id == client_record_id)
                .ok_or(OAuthStoreError::ClientNotFound)?;
            client.revocation_generation = client.revocation_generation.saturating_add(1);
            grant_ids = store
                .grants
                .iter()
                .filter(|g| g.client_record_id == client_record_id)
                .map(|g| g.id.clone())
                .collect();
            // Its grants can never verify again; keeping them would list a dead
            // connection as connected until its deadline.
            store
                .grants
                .retain(|g| g.client_record_id != client_record_id);
            Ok(())
        })?;
        let perm_store = crate::permissions::PermissionStore::open(&self.root);
        for gid in grant_ids {
            let _ = perm_store.remove_connection(&format!("oauth:{gid}"));
        }
        Ok(())
    }

    /// List only non-secret OAuth client metadata from the current store.
    pub fn list_clients(&self) -> Result<Vec<OAuthClientSummary>, OAuthStoreError> {
        Ok(self
            .read_store()?
            .clients
            .into_iter()
            .map(|client| OAuthClientSummary {
                client_id: client.client_id,
                client_name: client.client_name,
                created_at: client.created_at,
            })
            .collect())
    }

    /// Invalidate outstanding tokens for the client identified by `client_id`.
    pub fn revoke_client_by_client_id(&self, client_id: &str) -> Result<(), OAuthStoreError> {
        let client_id = client_id.to_owned();
        let mut grant_ids = Vec::new();
        self.mutate(|store, _now| {
            // One client id can hold a record for an owner hostname and one for
            // the other doors; revoking the client revokes both.
            let mut record_ids = Vec::new();
            for client in store
                .clients
                .iter_mut()
                .filter(|client| client.client_id == client_id)
            {
                client.revocation_generation = client.revocation_generation.saturating_add(1);
                record_ids.push(client.id.clone());
            }
            if record_ids.is_empty() {
                return Err(OAuthStoreError::ClientNotFound);
            }
            grant_ids = store
                .grants
                .iter()
                .filter(|g| record_ids.contains(&g.client_record_id))
                .map(|g| g.id.clone())
                .collect();
            // Its grants can never verify again; keeping them would list a dead
            // connection as connected until its deadline.
            store
                .grants
                .retain(|g| !record_ids.contains(&g.client_record_id));
            Ok(())
        })?;
        let perm_store = crate::permissions::PermissionStore::open(&self.root);
        for gid in grant_ids {
            let _ = perm_store.remove_connection(&format!("oauth:{gid}"));
        }
        Ok(())
    }

    /// List all active grants with their client metadata.
    pub fn list_grants(&self) -> Result<Vec<OAuthGrantSummary>, OAuthStoreError> {
        let store = self.read_store()?;
        let mut summaries = Vec::new();
        for grant in &store.grants {
            let client = store
                .clients
                .iter()
                .find(|c| c.id == grant.client_record_id);
            let client_name = client.and_then(|c| c.client_name.clone());
            summaries.push(OAuthGrantSummary {
                id: grant.id.clone(),
                client_id: grant.client_id.clone(),
                client_name,
                created_at: grant.created_at,
                access_expires_at: grant.access_expires_at,
                resource: grant.resource.clone(),
            });
        }
        Ok(summaries)
    }

    /// Read the bounded owner-facing history of refresh-token reuse revocations.
    pub(crate) fn list_replay_notices(&self) -> Result<Vec<ReplayNotice>, OAuthStoreError> {
        Ok(self.read_replay_notices()?.notices)
    }

    /// Dismiss one owner-facing refresh-token reuse notice.
    pub(crate) fn dismiss_replay_notice(&self, notice_id: &str) -> Result<bool, OAuthStoreError> {
        self.ensure_directory()?;
        let path = self.oauth_path();
        let _lock = hold_lock(
            &path,
            LockOptions {
                mode: Some(0o600),
                ..LockOptions::default()
            },
        )
        .map_err(OAuthStoreError::Lock)?;
        let mut file = self.read_replay_notices()?;
        let Some(notice) = file
            .notices
            .iter_mut()
            .find(|notice| notice.id == notice_id)
        else {
            return Ok(false);
        };
        if !notice.dismissed {
            notice.dismissed = true;
            self.write_replay_notices(&file)?;
        }
        Ok(true)
    }

    /// Remove every grant issued at an owner hostname generation other than
    /// `current`. A changed or removed hostname ends those grants for good
    /// (`RuntimeBinding::grant_matches`), so none may stay listed as connected.
    pub(crate) fn retire_byo_grants_outside(
        &self,
        current: Option<u64>,
    ) -> Result<usize, OAuthStoreError> {
        let mut retired = Vec::new();
        self.mutate(|store, _now| {
            retired = store
                .grants
                .iter()
                .filter(|grant| grant.generation.is_some() && grant.generation != current)
                .map(|grant| grant.id.clone())
                .collect();
            store
                .grants
                .retain(|grant| grant.generation.is_none() || grant.generation == current);
            Ok(())
        })?;
        let permissions = crate::permissions::PermissionStore::open(&self.root);
        for grant_id in &retired {
            let _ = permissions.remove_connection(&format!("oauth:{grant_id}"));
        }
        Ok(retired.len())
    }

    /// Revoke a specific grant by its connection ID.
    pub fn revoke_grant_by_id(&self, grant_id: &str) -> Result<bool, OAuthStoreError> {
        let grant_id = grant_id.to_owned();
        let mut removed = false;
        self.mutate(|store, _now| {
            let Some(index) = store.grants.iter().position(|g| g.id == grant_id) else {
                return Ok(false);
            };
            store.grants.remove(index);
            removed = true;
            Ok(true)
        })?;
        if removed {
            let _ = crate::permissions::PermissionStore::open(&self.root)
                .remove_connection(&format!("oauth:{grant_id}"));
        }
        Ok(removed)
    }

    fn mutate<T>(
        &self,
        operation: impl FnOnce(&mut OAuthStoreFile, DateTime<Utc>) -> Result<T, OAuthStoreError>,
    ) -> Result<T, OAuthStoreError> {
        self.edit(
            |store, now| {
                let result = operation(store, now);
                let persistence = match &result {
                    Ok(_) => EditPersistence::Persist,
                    Err(OAuthStoreError::PairingMismatch)
                    | Err(OAuthStoreError::TransactionExpired)
                    | Err(OAuthStoreError::TransactionExhausted)
                    | Err(OAuthStoreError::CodeExpired)
                    | Err(OAuthStoreError::BindingMismatch)
                    | Err(OAuthStoreError::Quota) => EditPersistence::Persist,
                    Err(_) => EditPersistence::Drop,
                };
                (persistence, result)
            },
            || Ok(()),
        )
    }

    fn edit<T>(
        &self,
        operation: impl FnOnce(
            &mut OAuthStoreFile,
            DateTime<Utc>,
        ) -> (EditPersistence, Result<T, OAuthStoreError>),
        after_write: impl FnOnce() -> Result<(), OAuthStoreError>,
    ) -> Result<T, OAuthStoreError> {
        self.ensure_directory()?;
        let path = self.oauth_path();
        let lock_options = LockOptions {
            mode: Some(0o600),
            #[cfg(test)]
            timeout: self
                .lock_timeout
                .unwrap_or_else(|| LockOptions::default().timeout),
            ..LockOptions::default()
        };
        let (result, lapsed) = {
            let _lock = hold_lock(&path, lock_options).map_err(OAuthStoreError::Lock)?;
            let mut store = self.read_store()?;
            let now = current_time();
            let lapsed = prune(&mut store, now);
            let (persistence, result) = operation(&mut store, now);
            if persistence != EditPersistence::Persist {
                return result;
            }
            self.write_store(&path, &mut store)?;
            after_write()?;
            (result, lapsed)
        };
        // A lapsed grant's read permission goes with it, as on every other way a
        // grant ends, so no row outlives the connection it described.
        if !lapsed.is_empty() {
            let permissions = crate::permissions::PermissionStore::open(&self.root);
            for grant_id in lapsed {
                let _ = permissions.remove_connection(&format!("oauth:{grant_id}"));
            }
        }
        result
    }

    fn endpoint_directory(&self) -> PathBuf {
        self.root.join(OAUTH_DIRECTORY)
    }

    fn oauth_path(&self) -> PathBuf {
        self.endpoint_directory().join(OAUTH_FILE)
    }

    fn replay_notices_path(&self) -> PathBuf {
        self.endpoint_directory().join("replay-notices.json")
    }

    fn ensure_directory(&self) -> Result<(), OAuthStoreError> {
        create_directory_with_mode(&self.endpoint_directory(), 0o700)
            .map_err(OAuthStoreError::Directory)
    }

    fn read_store(&self) -> Result<OAuthStoreFile, OAuthStoreError> {
        let path = self.oauth_path();
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(OAuthStoreFile::default());
            }
            Err(source) => return Err(OAuthStoreError::Read { path, source }),
        };
        let store = serde_json::from_slice::<OAuthStoreFile>(&bytes)
            .map_err(|_| OAuthStoreError::Malformed { path: path.clone() })?;
        if !matches!(store.schema, 1 | OAUTH_SCHEMA) {
            return Err(OAuthStoreError::UnsupportedSchema {
                path,
                found: store.schema,
            });
        }
        if store
            .grants
            .iter()
            .any(|grant| parse_refresh_verifier(&grant.refresh_verifier).is_none())
        {
            return Err(OAuthStoreError::Malformed { path });
        }
        Ok(store)
    }

    fn write_store(&self, path: &Path, store: &mut OAuthStoreFile) -> Result<(), OAuthStoreError> {
        store.schema = OAUTH_SCHEMA;
        enforce_entry_sizes(store)?;
        let encoded = serde_json::to_vec(store).map_err(|_| OAuthStoreError::Malformed {
            path: path.to_path_buf(),
        })?;
        if encoded.len() > max_state_bytes() {
            return Err(OAuthStoreError::StateTooLarge);
        }
        write_json(
            path,
            store,
            JsonWriteOptions {
                mode: Some(0o600),
                ..JsonWriteOptions::default()
            },
        )
        .map_err(OAuthStoreError::Write)
    }

    fn read_replay_notices(&self) -> Result<ReplayNoticesFile, OAuthStoreError> {
        let path = self.replay_notices_path();
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(ReplayNoticesFile::default());
            }
            Err(source) => return Err(OAuthStoreError::Read { path, source }),
        };
        serde_json::from_slice(&bytes).map_err(|_| OAuthStoreError::Malformed { path })
    }

    fn append_replay_notice_locked(&self, notice: ReplayNotice) -> Result<(), OAuthStoreError> {
        let mut file = self.read_replay_notices()?;
        if file.notices.iter().any(|existing| existing.id == notice.id) {
            return Ok(());
        }
        file.notices.push(notice);
        if file.notices.len() > MAX_REPLAY_NOTICES {
            let excess = file.notices.len() - MAX_REPLAY_NOTICES;
            file.notices.drain(..excess);
        }
        self.write_replay_notices(&file)
    }

    fn write_replay_notices(&self, file: &ReplayNoticesFile) -> Result<(), OAuthStoreError> {
        let path = self.replay_notices_path();
        write_json(
            &path,
            file,
            JsonWriteOptions {
                mode: Some(0o600),
                ..JsonWriteOptions::default()
            },
        )
        .map_err(OAuthStoreError::Write)
    }
}

fn presented_pairing_digest(pairing_code: &str) -> Option<[u8; TOKEN_BYTES]> {
    canonicalize_pairing_code(pairing_code).map(|code| sha256_digest(code.as_bytes()))
}

/// Find a transaction this runtime may finish that has not issued a code yet.
fn open_pending_index(
    store: &mut OAuthStoreFile,
    transaction_id: &str,
    binding: &super::RuntimeBinding,
    now: DateTime<Utc>,
) -> Result<usize, OAuthStoreError> {
    let index = store
        .pending
        .iter()
        .position(|pending| pending.transaction_id == transaction_id)
        .ok_or(OAuthStoreError::TransactionNotFound)?;
    if store.pending[index].resource != binding.canonical()
        || store.pending[index].generation != binding.stored_grant_generation()
    {
        return Err(OAuthStoreError::TransactionNotFound);
    }
    if store.pending[index].authorization_code_verifier.is_none()
        && store.pending[index].expires_at <= now
    {
        store.pending.remove(index);
        return Err(OAuthStoreError::TransactionExpired);
    }
    if store.pending[index].authorization_code_verifier.is_some() {
        return Err(OAuthStoreError::InvalidToken);
    }
    if store.pending[index].failure_count >= MAX_TRANSACTION_FAILURES {
        store.pending.remove(index);
        return Err(OAuthStoreError::TransactionExhausted);
    }
    Ok(index)
}

/// Check the presented code against the live pairing code and the door it was
/// made for, then consume it. A mismatch counts against the transaction.
fn consume_pairing_code(
    store: &mut OAuthStoreFile,
    index: usize,
    presented_digest: Option<[u8; TOKEN_BYTES]>,
    binding: &super::RuntimeBinding,
) -> Result<(), OAuthStoreError> {
    let Some(pairing) = store.pairing.as_ref() else {
        return Err(OAuthStoreError::NoActivePairing);
    };
    if pairing.locked {
        return Err(OAuthStoreError::PairingLocked);
    }
    // A transaction belongs to the code that was open when it started. Once
    // that code is used, replaced or revoked, the transaction is over.
    if store.pending[index].pairing_window != Some(pairing.generation) {
        store.pending.remove(index);
        return Err(OAuthStoreError::TransactionExpired);
    }
    let verifier = decode_b64_32(&pairing.verifier).ok_or_else(|| OAuthStoreError::Malformed {
        path: PathBuf::from(OAUTH_FILE),
    })?;
    let door_matches = pairing_door_matches(pairing, binding);
    let matches =
        presented_digest.is_some_and(|digest| bool::from(digest.ct_eq(&verifier))) && door_matches;
    if !matches {
        store.pending[index].failure_count += 1;
        if store.pending[index].failure_count >= MAX_TRANSACTION_FAILURES {
            store.pending.remove(index);
        }
        return Err(OAuthStoreError::PairingMismatch);
    }
    if let Some(pairing) = store.pairing.as_ref()
        && pairing.reconnect_to.is_some()
    {
        store.pending[index].reconnect_to = pairing.reconnect_to.clone();
    }
    store.pairing_generation = store.pairing_generation.saturating_add(1);
    store.pairing = None;
    // The code is spent; this transaction no longer needs its window, and an
    // older binary can read the file again once no row carries one.
    store.pending[index].pairing_window = None;
    Ok(())
}

/// Whether a pairing code was made for the door this runtime serves.
fn pairing_door_matches(pairing: &StoredPairing, binding: &super::RuntimeBinding) -> bool {
    match pairing.door.as_deref() {
        None => false,
        Some("relay") => matches!(binding, super::RuntimeBinding::Unbound { .. }),
        Some("local") => match binding {
            super::RuntimeBinding::Bound { canonical } => {
                canonical != solstone_core_journal_config::MCP_LAN_DOOR_RESOURCE
            }
            _ => false,
        },
        Some(solstone_core_journal_config::MCP_LAN_DOOR_RESOURCE) => match binding {
            super::RuntimeBinding::Bound { canonical } => {
                canonical == solstone_core_journal_config::MCP_LAN_DOOR_RESOURCE
            }
            _ => false,
        },
        Some(required) if required.starts_with("https://") => match binding {
            super::RuntimeBinding::Byo {
                canonical,
                generation,
            } => canonical == required && pairing.config_generation == Some(*generation),
            _ => false,
        },
        Some(_) => false,
    }
}

fn issue_authorization(
    pending: &mut StoredPending,
    permission: Option<ReadPermission>,
    code_bytes: &[u8; TOKEN_BYTES],
    now: DateTime<Utc>,
) -> IssuedAuthorization {
    pending.authorization_code_verifier = Some(sha256_b64(code_bytes));
    pending.code_expires_at = Some(now + Duration::seconds(AUTH_CODE_TTL_SECS));
    pending.permission = permission;
    IssuedAuthorization {
        code: URL_SAFE_NO_PAD.encode(code_bytes),
        redirect_uri: pending.redirect_uri.clone(),
        state: pending.state.clone(),
        issuer: pending.issuer.clone(),
    }
}

fn is_source_held_by_live_connection_locked(
    root: &Path,
    store: &OAuthStoreFile,
    hex: &str,
) -> Result<bool, OAuthStoreError> {
    let path = crate::agent_state::agent_state_path(root);
    let _side_lock = if path.exists() {
        Some(
            hold_lock(
                &path,
                LockOptions {
                    mode: Some(0o600),
                    ..LockOptions::default()
                },
            )
            .map_err(OAuthStoreError::Lock)?,
        )
    } else {
        None
    };
    let tokens = TokenStore::open(root)
        .list()
        .map_err(|_| OAuthStoreError::StateTooLarge)?;
    for token in tokens {
        let conn = format!("bearer:{}", token.id);
        match crate::agent_state::memory_source(root, &conn) {
            Ok(src) => {
                if src.component() == hex {
                    return Ok(true);
                }
            }
            Err(_) => return Err(OAuthStoreError::StateTooLarge),
        }
    }
    for grant in &store.grants {
        let conn = format!("oauth:{}", grant.id);
        match crate::agent_state::memory_source(root, &conn) {
            Ok(src) => {
                if src.component() == hex {
                    return Ok(true);
                }
            }
            Err(_) => return Err(OAuthStoreError::StateTooLarge),
        }
    }
    Ok(false)
}

/// Drops what has lapsed and returns the ids of the grants it dropped, so the
/// caller can remove their permission rows once the drop is on disk.
fn prune(store: &mut OAuthStoreFile, now: DateTime<Utc>) -> Vec<String> {
    store.pending.retain(|pending| {
        if pending.authorization_code_verifier.is_some() {
            pending.code_expires_at.is_some_and(|expires| expires > now)
        } else {
            pending.expires_at > now
        }
    });
    let lapsed = Vec::new();
    store.clients.retain(|client| {
        client.last_used_at.is_some()
            || now - client.created_at < Duration::seconds(CLIENT_UNUSED_TTL_SECS)
    });
    if let Some(pairing) = &store.pairing
        && pairing.expires_at <= now
    {
        store.pairing_generation = store.pairing_generation.saturating_add(1);
        store.pairing = None;
    }
    // A transaction still waiting on a code ends with that code: once it is
    // used, replaced, revoked or expired, the transaction goes too.
    let live = store.pairing.as_ref().map(|pairing| pairing.generation);
    store
        .pending
        .retain(|pending| pending.pairing_window.is_none() || pending.pairing_window == live);
    lapsed
}

fn enforce_entry_sizes(store: &OAuthStoreFile) -> Result<(), OAuthStoreError> {
    for client in &store.clients {
        entry_size(client)?;
    }
    for grant in &store.grants {
        entry_size(grant)?;
    }
    for pending in &store.pending {
        entry_size(pending)?;
    }
    if let Some(pairing) = &store.pairing {
        entry_size(pairing)?;
    }
    Ok(())
}

fn entry_size<T: Serialize>(value: &T) -> Result<(), OAuthStoreError> {
    let encoded = serde_json::to_vec(value).map_err(|_| OAuthStoreError::EntryTooLarge)?;
    if encoded.len() > MAX_OAUTH_ENTRY_BYTES {
        return Err(OAuthStoreError::EntryTooLarge);
    }
    Ok(())
}

fn registered_from(client: &StoredClient) -> RegisteredClient {
    RegisteredClient {
        id: client.id.clone(),
        client_id: client.client_id.clone(),
        redirect_uris: client.redirect_uris.clone(),
        client_name: client.client_name.clone(),
        created_at: client.created_at,
    }
}

fn fill_exact(random: &dyn RandomSource, bytes: &mut [u8]) -> Result<(), OAuthStoreError> {
    let written = random
        .fill(bytes)
        .map_err(|RandomSourceError| OAuthStoreError::Randomness)?;
    if written != bytes.len() {
        return Err(OAuthStoreError::Randomness);
    }
    Ok(())
}

fn random_bytes(random: &dyn RandomSource) -> Result<[u8; TOKEN_BYTES], OAuthStoreError> {
    let mut bytes = [0_u8; TOKEN_BYTES];
    fill_exact(random, &mut bytes)?;
    Ok(bytes)
}

fn random_b64(random: &dyn RandomSource) -> Result<String, OAuthStoreError> {
    Ok(URL_SAFE_NO_PAD.encode(random_bytes(random)?))
}

fn sha256_digest(bytes: &[u8]) -> [u8; TOKEN_BYTES] {
    Sha256::digest(bytes).into()
}

fn sha256_b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(sha256_digest(bytes))
}

fn parse_refresh_verifier(value: &str) -> Option<ParsedRefreshVerifier> {
    let Some((current, packed)) = value.split_once('|') else {
        let current_verifier = decode_canonical_b64_32(value)?;
        return Some(ParsedRefreshVerifier {
            current_verifier,
            current_verifier_b64: value.to_owned(),
            secret: None,
            history: Vec::new(),
        });
    };
    let (secret_b64, history_text) = packed.split_once('|')?;
    if history_text.is_empty() || history_text.contains('|') {
        return None;
    }
    let current_verifier = decode_canonical_b64_32(current)?;
    let secret = decode_canonical_b64_32(secret_b64)?;
    let mut history = Vec::new();
    for entry in history_text.split(';') {
        let mut fields = entry.split(',');
        let verifier_b64 = fields.next()?;
        let timestamp = fields.next()?;
        let flag = fields.next()?;
        if fields.next().is_some() || verifier_b64.is_empty() {
            return None;
        }
        let verifier = decode_canonical_b64_32(verifier_b64)?;
        let rotated_at = timestamp.parse::<i64>().ok()?;
        if rotated_at.to_string() != timestamp {
            return None;
        }
        let derived = match flag {
            "0" => false,
            "1" => true,
            _ => return None,
        };
        history.push(RefreshHistoryEntry {
            verifier,
            verifier_b64: verifier_b64.to_owned(),
            rotated_at,
            derived,
        });
        if history.len() > REFRESH_HISTORY_LIMIT {
            return None;
        }
    }
    Some(ParsedRefreshVerifier {
        current_verifier,
        current_verifier_b64: current.to_owned(),
        secret: Some(secret),
        history,
    })
}

fn write_refresh_verifier(
    current_verifier: &str,
    secret: &[u8; TOKEN_BYTES],
    history: &[RefreshHistoryEntry],
) -> String {
    let history = history
        .iter()
        .take(REFRESH_HISTORY_LIMIT)
        .map(|entry| {
            format!(
                "{},{},{}",
                entry.verifier_b64,
                entry.rotated_at,
                u8::from(entry.derived)
            )
        })
        .collect::<Vec<_>>()
        .join(";");
    format!(
        "{current_verifier}|{}|{history}",
        URL_SAFE_NO_PAD.encode(secret)
    )
}

fn decode_canonical_b64_32(value: &str) -> Option<[u8; TOKEN_BYTES]> {
    let decoded: [u8; TOKEN_BYTES] = URL_SAFE_NO_PAD.decode(value).ok()?.try_into().ok()?;
    (URL_SAFE_NO_PAD.encode(decoded) == value).then_some(decoded)
}

/// Derive a rotation's token pair from the grant's secret and the refresh token
/// presented for it. The stored file holds the secret but only a digest of each
/// refresh token, so it cannot reproduce a pair without a live token.
fn derive_token_pair(
    secret: &[u8; TOKEN_BYTES],
    generation: u64,
    presented_refresh: &[u8],
) -> ([u8; 32], [u8; 32]) {
    (
        derive_token(secret, b"access", generation, presented_refresh),
        derive_token(secret, b"refresh", generation, presented_refresh),
    )
}

fn derive_token(
    secret: &[u8; TOKEN_BYTES],
    domain: &[u8],
    generation: u64,
    presented_refresh: &[u8],
) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(domain);
    mac.update(&generation.to_be_bytes());
    mac.update(presented_refresh);
    mac.finalize().into_bytes().into()
}

fn replay_door_label(resource: Option<&str>) -> String {
    match resource {
        None => "solstone.me".to_owned(),
        Some(solstone_core_journal_config::MCP_LAN_DOOR_RESOURCE) => "your network".to_owned(),
        Some(solstone_core_journal_config::MCP_LOCAL_DOOR_RESOURCE) => "this computer".to_owned(),
        Some(value) if value.starts_with("https://") => "your own domain".to_owned(),
        Some(_) => "solstone.me".to_owned(),
    }
}

fn decode_b64_32(value: &str) -> Option<[u8; TOKEN_BYTES]> {
    URL_SAFE_NO_PAD.decode(value).ok()?.try_into().ok()
}

fn decode_sha256(presented: &str) -> Option<[u8; TOKEN_BYTES]> {
    let raw = URL_SAFE_NO_PAD.decode(presented).ok()?;
    Some(sha256_digest(&raw))
}

pub(crate) fn current_time() -> DateTime<Utc> {
    #[cfg(test)]
    {
        if let Some(now) = TEST_NOW.with(std::cell::Cell::get) {
            return DateTime::<Utc>::from_timestamp(now, 0).unwrap_or_else(Utc::now);
        }
    }
    Utc::now()
}

fn max_state_bytes() -> usize {
    #[cfg(test)]
    {
        if let Some(limit) = TEST_MAX_STATE_BYTES.with(std::cell::Cell::get) {
            return limit;
        }
    }
    MAX_OAUTH_STATE_BYTES
}

#[cfg(test)]
thread_local! {
    static TEST_NOW: std::cell::Cell<Option<i64>> = const { std::cell::Cell::new(None) };
    static TEST_MAX_STATE_BYTES: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

#[cfg(all(test, feature = "full-tests"))]
pub(crate) fn set_test_now(timestamp: Option<i64>) {
    TEST_NOW.with(|now| now.set(timestamp));
}

#[cfg(all(test, not(feature = "full-tests")))]
pub(crate) fn set_routine_now(timestamp: Option<i64>) {
    TEST_NOW.with(|now| now.set(timestamp));
}

#[cfg(all(test, feature = "full-tests"))]
pub(crate) fn set_routine_now(timestamp: Option<i64>) {
    set_test_now(timestamp);
}

#[cfg(all(test, not(feature = "full-tests")))]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::collections::HashSet;
    use std::fs;
    use std::net::{IpAddr, Ipv4Addr};

    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use chrono::{Duration, TimeZone, Utc};
    use serde::Deserialize;
    use sha2::Digest;

    use super::{
        AUTH_CODE_TTL_SECS, CLIENT_UNUSED_TTL_SECS, MAX_CLIENTS, MAX_CLIENTS_PER_SOURCE,
        MAX_GRANTS, MAX_OAUTH_ENTRY_BYTES, MAX_PENDING_PER_SOURCE, MAX_REPLAY_NOTICES,
        OAUTH_SCHEMA, OAuthStore, OAuthStoreError, OAuthStoreFile, PENDING_TRANSACTION_TTL_SECS,
        REFRESH_HISTORY_LIMIT, StoredClient, StoredGrant, TEST_MAX_STATE_BYTES, TEST_NOW,
        current_time, sha256_b64,
    };
    use crate::http1::{HttpMethod, HttpRequest};
    use crate::oauth::{OAuthRuntime, RuntimeBinding};
    use crate::permissions::{PermissionStore, PermissionsFile, ReadPermission};
    use crate::tokens::{RandomSource, RandomSourceError};

    fn test_binding() -> RuntimeBinding {
        RuntimeBinding::Unbound {
            canonical: "https://mcp.test/mcp".to_owned(),
        }
    }

    fn journal_root() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("solstone-mcp-oauth-")
            .tempdir_in(crate::test_scratch())
            .unwrap()
    }

    fn store_in(journal: &tempfile::TempDir) -> OAuthStore {
        OAuthStore::open(journal.path())
    }

    fn issue_tokens(
        store: &OAuthStore,
        client_record_id: &str,
        client_id: &str,
        binding: &RuntimeBinding,
        door: &str,
        permission: Option<ReadPermission>,
    ) -> super::IssuedTokens {
        issue_tokens_from_source(
            store,
            client_record_id,
            client_id,
            binding,
            door,
            permission,
            "192.0.2.1",
        )
    }

    fn issue_tokens_from_source(
        store: &OAuthStore,
        client_record_id: &str,
        client_id: &str,
        binding: &RuntimeBinding,
        door: &str,
        permission: Option<ReadPermission>,
        source: &str,
    ) -> super::IssuedTokens {
        let pairing = store.generate_pairing_code_with_door(door).unwrap();
        let transaction = open_transaction_for_binding(store, client_record_id, binding, source);
        let authorization = store
            .complete_pairing_with_permission(&transaction, &pairing.code, permission, binding)
            .unwrap();
        store
            .redeem_authorization_code(
                &authorization.code,
                client_id,
                "http://127.0.0.1/callback",
                binding.canonical(),
                "pkce-verifier",
                binding,
            )
            .unwrap()
    }

    fn refresh_request(
        oauth: &OAuthRuntime,
        refresh: &str,
        client_id: &str,
    ) -> crate::http1::HttpResponse {
        let body =
            format!("grant_type=refresh_token&refresh_token={refresh}&client_id={client_id}");
        let request = HttpRequest::from_test_parts(HttpMethod::Post, Vec::new(), body.into_bytes());
        crate::oauth::token::token(&request, oauth)
    }

    fn response_json(response: &crate::http1::HttpResponse) -> serde_json::Value {
        serde_json::from_slice(&response.body).unwrap()
    }

    fn seed_client(store: &OAuthStore, source: &str) -> String {
        store
            .register_client(
                "https://client.example/cimd.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                Some("fixture".to_owned()),
                source,
            )
            .unwrap()
            .id
    }

    fn open_transaction(store: &OAuthStore, client_id: &str, source: &str) -> String {
        let challenge = sha256_b64(b"pkce-verifier");
        store
            .create_transaction(
                client_id,
                "http://127.0.0.1/callback",
                "https://mcp.test/mcp",
                "https://mcp.test",
                &challenge,
                "S256",
                Some("state-1"),
                source,
            )
            .unwrap()
    }

    fn open_transaction_for_binding(
        store: &OAuthStore,
        client_id: &str,
        binding: &RuntimeBinding,
        source: &str,
    ) -> String {
        let challenge = sha256_b64(b"pkce-verifier");
        store
            .create_transaction_with_generation(
                client_id,
                "http://127.0.0.1/callback",
                binding.canonical(),
                "https://mcp.test",
                &challenge,
                "S256",
                Some("state-1"),
                source,
                binding.stored_grant_generation(),
            )
            .unwrap()
    }

    struct ShortRandom;

    impl RandomSource for ShortRandom {
        fn fill(&self, bytes: &mut [u8]) -> Result<usize, RandomSourceError> {
            let count = bytes.len().saturating_sub(1);
            bytes[..count].fill(0x5a);
            Ok(count)
        }
    }

    fn set_now(timestamp: i64) {
        TEST_NOW.with(|now| now.set(Some(timestamp)));
    }

    fn clear_now() {
        TEST_NOW.with(|now| now.set(None));
    }

    struct NowGuard;

    impl Drop for NowGuard {
        fn drop(&mut self) {
            clear_now();
            TEST_MAX_STATE_BYTES.with(|limit| limit.set(None));
        }
    }

    fn whole_journal_grant(store: &OAuthStore, journal: &tempfile::TempDir) -> String {
        let client = seed_client(store, "192.0.2.1");
        let tokens = issue_tokens(
            store,
            &client,
            "https://client.example/cimd.json",
            &test_binding(),
            "relay",
            Some(ReadPermission::default_whole_journal()),
        );
        assert!(
            PermissionStore::open(journal.path())
                .get_permission(&format!("oauth:{}", tokens.token_id))
                .unwrap()
                .is_some()
        );
        tokens.token_id
    }

    fn permission_row(journal: &tempfile::TempDir, grant_id: &str) -> bool {
        PermissionStore::open(journal.path())
            .get_permission(&format!("oauth:{grant_id}"))
            .unwrap()
            .is_some()
    }

    #[test]
    fn a_new_pairing_connects_while_the_agent_state_is_unreadable() {
        let journal = journal_root();
        let store = store_in(&journal);
        std::fs::create_dir_all(journal.path().join("mcp-endpoint")).unwrap();
        std::fs::write(
            journal.path().join("mcp-endpoint/agent-state.json"),
            b"not json",
        )
        .unwrap();
        let grant = whole_journal_grant(&store, &journal);
        assert!(store.list_grants().unwrap().iter().any(|g| g.id == grant));
        let record = PermissionStore::open(journal.path())
            .get_permission(&format!("oauth:{grant}"))
            .unwrap()
            .unwrap();
        assert_eq!(record.evaluation, "hold");
    }

    #[test]
    fn a_revoked_client_leaves_no_connection_listed() {
        let journal = journal_root();
        let store = store_in(&journal);
        let grant = whole_journal_grant(&store, &journal);

        store
            .revoke_client_by_client_id("https://client.example/cimd.json")
            .unwrap();

        assert!(!store.list_grants().unwrap().iter().any(|g| g.id == grant));
        assert!(!permission_row(&journal, &grant));
    }

    #[test]
    fn revoking_a_client_revokes_its_hostname_record_too() {
        let journal = journal_root();
        let store = store_in(&journal);
        let other_doors = whole_journal_grant(&store, &journal);
        let hostname_record = store
            .register_client(
                "https://client.example/cimd.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                Some("fixture".to_owned()),
                "byo:192.0.2.1",
            )
            .unwrap()
            .id;
        let hostname = issue_tokens(
            &store,
            &hostname_record,
            "https://client.example/cimd.json",
            &test_binding(),
            "relay",
            Some(ReadPermission::default_whole_journal()),
        )
        .token_id;
        assert_ne!(other_doors, hostname);

        store
            .revoke_client_by_client_id("https://client.example/cimd.json")
            .unwrap();

        assert!(store.list_grants().unwrap().is_empty());
        assert!(!permission_row(&journal, &other_doors));
        assert!(!permission_row(&journal, &hostname));
    }

    #[test]
    fn a_grant_from_an_earlier_hostname_leaves_the_list() {
        let journal = journal_root();
        let store = store_in(&journal);
        let earlier = whole_journal_grant(&store, &journal);
        let current = whole_journal_grant(&store, &journal);
        let elsewhere = whole_journal_grant(&store, &journal);
        store
            .mutate(|file, _| {
                for grant in &mut file.grants {
                    if grant.id == earlier {
                        grant.generation = Some(1);
                    } else if grant.id == current {
                        grant.generation = Some(2);
                    }
                }
                Ok(())
            })
            .unwrap();

        assert_eq!(store.retire_byo_grants_outside(Some(2)).unwrap(), 1);

        let listed: Vec<String> = store
            .list_grants()
            .unwrap()
            .into_iter()
            .map(|g| g.id)
            .collect();
        assert!(!listed.contains(&earlier));
        assert!(listed.contains(&current));
        assert!(
            listed.contains(&elsewhere),
            "a grant on another door is untouched"
        );
        assert!(!permission_row(&journal, &earlier));
        assert!(permission_row(&journal, &current));
    }

    #[test]
    fn an_idle_grant_stays_listed_and_its_permission_bytes_stay() {
        let _guard = NowGuard;
        let start = Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap();
        set_now(start.timestamp());
        let journal = journal_root();
        let store = store_in(&journal);
        let grant = whole_journal_grant(&store, &journal);
        let perm_path = journal.path().join("mcp-endpoint/permissions.json");
        let bytes_before = fs::read(&perm_path).unwrap();

        set_now(start.timestamp() + super::REFRESH_TTL_SECS + 1);
        // Any write prunes; making a pairing code is the owner's usual one.
        store.generate_pairing_code_with_door("relay").unwrap();

        let bytes_after = fs::read(&perm_path).unwrap();
        assert_eq!(bytes_before, bytes_after);
        assert!(store.list_grants().unwrap().iter().any(|g| g.id == grant));
        assert!(permission_row(&journal, &grant));
    }

    #[test]
    fn generate_and_reread_round_trip() {
        let journal = journal_root();
        let store = store_in(&journal);
        let created = store.generate_pairing_code_with_door("local").unwrap();
        assert_eq!(created.generation, 1);
        assert_eq!(store.pairing_generation().unwrap(), 1);
        assert_eq!(created.code.len(), 8);
        assert_eq!(created.door, "local");
    }

    #[test]
    fn pairing_is_single_use() {
        let journal = journal_root();
        let store = store_in(&journal);
        let client = seed_client(&store, "192.0.2.1");
        let pairing = store.generate_pairing_code_with_door("relay").unwrap();
        let transaction = open_transaction(&store, &client, "192.0.2.1");
        let issued = store
            .complete_pairing(&transaction, &pairing.code, &test_binding())
            .unwrap();
        assert_eq!(issued.redirect_uri, "http://127.0.0.1/callback");
        assert_eq!(issued.state.as_deref(), Some("state-1"));
        assert_eq!(issued.issuer, "https://mcp.test");
        assert_eq!(store.pairing_generation().unwrap(), 0);
        let challenge = sha256_b64(b"pkce-verifier");
        assert!(matches!(
            store.create_transaction(
                &client,
                "http://127.0.0.1/callback",
                "https://mcp.test/mcp",
                "https://mcp.test",
                &challenge,
                "S256",
                None,
                "192.0.2.1",
            ),
            Err(OAuthStoreError::NoActivePairing)
        ));
    }

    #[test]
    fn a_transaction_ends_with_the_code_it_started_under() {
        let journal = journal_root();
        let store = store_in(&journal);
        let client = seed_client(&store, "192.0.2.1");
        store.generate_pairing_code_with_door("relay").unwrap();
        let transaction = open_transaction(&store, &client, "192.0.2.1");
        let replacement = store.generate_pairing_code_with_door("relay").unwrap();
        assert!(matches!(
            store.complete_pairing(&transaction, &replacement.code, &test_binding()),
            Err(OAuthStoreError::TransactionExpired | OAuthStoreError::TransactionNotFound)
        ));
        let fresh = open_transaction(&store, &client, "192.0.2.1");
        store
            .complete_pairing(&fresh, &replacement.code, &test_binding())
            .expect("a transaction started under the open code finishes with it");
    }

    #[test]
    fn revoking_the_code_ends_every_transaction_waiting_on_it() {
        let journal = journal_root();
        let store = store_in(&journal);
        let client = seed_client(&store, "192.0.2.1");
        store.generate_pairing_code_with_door("relay").unwrap();
        let transaction = open_transaction(&store, &client, "192.0.2.1");
        store.revoke_pairing_code().unwrap();
        let fresh = store.generate_pairing_code_with_door("relay").unwrap();
        assert!(matches!(
            store.complete_pairing(&transaction, &fresh.code, &test_binding()),
            Err(OAuthStoreError::TransactionExpired | OAuthStoreError::TransactionNotFound)
        ));
    }

    #[test]
    fn a_spent_or_revoked_code_leaves_no_row_an_older_binary_cannot_read() {
        let journal = journal_root();
        let store = store_in(&journal);
        let client = seed_client(&store, "192.0.2.1");
        let pairing = store.generate_pairing_code_with_door("relay").unwrap();
        let used = open_transaction(&store, &client, "192.0.2.1");
        let _waiting = open_transaction(&store, &client, "192.0.2.2");
        store
            .complete_pairing(&used, &pairing.code, &test_binding())
            .unwrap();
        // The downgrade recipe: make a code with this version, then revoke it.
        store.generate_pairing_code_with_door("relay").unwrap();
        open_transaction(&store, &client, "192.0.2.3");
        store.revoke_pairing_code().unwrap();
        store.generate_pairing_code_with_door("relay").unwrap();
        store.revoke_pairing_code().unwrap();
        let raw = fs::read_to_string(journal.path().join("mcp-endpoint/oauth.json")).unwrap();
        assert!(!raw.contains("pairing_window"), "{raw}");
        assert_eq!(
            store.read_store().unwrap().pending.len(),
            1,
            "only the issued code's row stays"
        );
    }

    #[test]
    fn a_transaction_is_stored_only_when_the_open_code_is_for_this_door() {
        let journal = journal_root();
        let store = store_in(&journal);
        let client = seed_client(&store, "192.0.2.1");
        let challenge = sha256_b64(b"pkce-verifier");
        let open_at = |binding: &RuntimeBinding| {
            store.create_transaction_at_door(
                binding,
                &client,
                "http://127.0.0.1/callback",
                binding.canonical(),
                "https://mcp.test",
                &challenge,
                "S256",
                None,
                "192.0.2.1",
            )
        };
        store.generate_pairing_code_with_door("local").unwrap();
        assert!(matches!(
            open_at(&test_binding()),
            Err(OAuthStoreError::NoActivePairing)
        ));
        assert!(store.read_store().unwrap().pending.is_empty());
        store.generate_pairing_code_with_door("relay").unwrap();
        open_at(&test_binding()).expect("the open code is for this door");
    }

    #[test]
    fn only_a_client_ids_first_grant_ever_is_marked_first() {
        let _guard = NowGuard;
        let start = Utc.with_ymd_and_hms(2026, 9, 27, 12, 0, 0).unwrap();
        set_now(start.timestamp());
        let journal = journal_root();
        let store = store_in(&journal);
        let binding = test_binding();
        let agent = "https://agent.example/cimd.json";
        let register = |client_id: &str, source: &str| {
            store
                .register_client(
                    client_id,
                    vec!["http://127.0.0.1/callback".to_owned()],
                    Some("Agent".to_owned()),
                    source,
                )
                .unwrap()
        };
        let client = register(agent, "192.0.2.1");
        let first = issue_tokens(&store, &client.id, agent, &binding, "relay", None);
        let marked = first.first_grant.as_ref().expect("first grant is marked");
        assert_eq!(marked.client_name.as_deref(), Some("Agent"));

        let refreshed = store
            .refresh_grant(&first.refresh_token, agent, &binding)
            .unwrap();
        assert!(refreshed.first_grant.is_none(), "a refresh is not a grant");
        let again = issue_tokens(&store, &client.id, agent, &binding, "relay", None);
        assert!(again.first_grant.is_none(), "pairing again while connected");

        for grant in [&first.token_id, &again.token_id] {
            store.revoke_grant_by_id(grant).unwrap();
        }
        let after_disconnect = issue_tokens(&store, &client.id, agent, &binding, "relay", None);
        assert!(
            after_disconnect.first_grant.is_none(),
            "after disconnecting"
        );

        // Refresh expiry no longer prunes grants. `first` and `again` were
        // revoked before the lapse; `after_disconnect` stays.
        set_now(start.timestamp() + super::REFRESH_TTL_SECS + 1);
        let after_lapse = issue_tokens(&store, &client.id, agent, &binding, "relay", None);
        assert!(after_lapse.first_grant.is_none(), "after its access lapsed");
        let held = store.read_store().unwrap().grants;
        assert_eq!(
            held.iter()
                .map(|grant| grant.id.as_str())
                .collect::<Vec<_>>(),
            [
                after_disconnect.token_id.as_str(),
                after_lapse.token_id.as_str()
            ],
            "after_disconnect stays and after_lapse is added"
        );

        let other_cohort = register(agent, "byo:example");
        assert_ne!(other_cohort.id, client.id);
        let elsewhere = issue_tokens(&store, &other_cohort.id, agent, &binding, "relay", None);
        assert!(
            elsewhere.first_grant.is_none(),
            "the same client id elsewhere"
        );

        let other = "https://other.example/cimd.json";
        let other_client = register(other, "192.0.2.1");
        let first_other = issue_tokens(&store, &other_client.id, other, &binding, "relay", None);
        assert!(first_other.first_grant.is_some(), "a different client");
    }

    #[test]
    fn the_window_is_open_only_while_a_live_code_for_this_door_is() {
        let _guard = NowGuard;
        let start = Utc.with_ymd_and_hms(2026, 9, 27, 12, 0, 0).unwrap();
        set_now(start.timestamp());
        let journal = journal_root();
        let store = store_in(&journal);
        let loopback = RuntimeBinding::Bound {
            canonical: "http://127.0.0.1:7659/mcp".to_owned(),
        };
        assert!(!store.pairing_window_open(&test_binding()).unwrap());
        store.generate_pairing_code_with_door("relay").unwrap();
        assert!(store.pairing_window_open(&test_binding()).unwrap());
        assert!(!store.pairing_window_open(&loopback).unwrap());
        set_now(start.timestamp() + super::PAIRING_TTL_SECS);
        assert!(!store.pairing_window_open(&test_binding()).unwrap());
        set_now(start.timestamp());
        store.generate_pairing_code_with_door("relay").unwrap();
        store.revoke_pairing_code().unwrap();
        assert!(!store.pairing_window_open(&test_binding()).unwrap());
    }

    #[test]
    fn five_wrong_guesses_delete_only_that_transaction() {
        let journal = journal_root();
        let store = store_in(&journal);
        let client = seed_client(&store, "192.0.2.1");
        let pairing = store.generate_pairing_code_with_door("relay").unwrap();
        let transaction = open_transaction(&store, &client, "192.0.2.1");
        for _ in 0..5 {
            assert!(matches!(
                store.complete_pairing(&transaction, "00000000", &test_binding()),
                Err(OAuthStoreError::PairingMismatch)
            ));
        }
        assert!(matches!(
            store.complete_pairing(&transaction, &pairing.code, &test_binding()),
            Err(OAuthStoreError::TransactionNotFound)
        ));
        let retry = open_transaction(&store, &client, "192.0.2.1");
        store
            .complete_pairing(&retry, &pairing.code, &test_binding())
            .expect("pairing remains usable on a new transaction");
    }

    #[test]
    fn lock_pairing_blocks_every_source_without_changing_generation() {
        let journal = journal_root();
        let store = store_in(&journal);
        let client = seed_client(&store, "192.0.2.1");
        let pairing = store.generate_pairing_code_with_door("relay").unwrap();
        let generation = pairing.generation;
        let transaction = open_transaction(&store, &client, "198.51.100.9");
        store.lock_pairing_code().unwrap();
        assert_eq!(store.pairing_generation().unwrap(), generation);
        assert!(matches!(
            store.complete_pairing(&transaction, &pairing.code, &test_binding()),
            Err(OAuthStoreError::PairingLocked)
        ));
        assert!(
            !store.pairing_window_open(&test_binding()).unwrap(),
            "a locked code opens nothing"
        );
        let challenge = sha256_b64(b"pkce-verifier");
        assert!(matches!(
            store.create_transaction(
                &client,
                "http://127.0.0.1/callback",
                "https://mcp.test/mcp",
                "https://mcp.test",
                &challenge,
                "S256",
                None,
                "198.51.100.9",
            ),
            Err(OAuthStoreError::NoActivePairing)
        ));
        assert_eq!(store.pairing_generation().unwrap(), generation);
    }

    #[test]
    fn generate_and_revoke_bump_generation_and_clear_lock() {
        let journal = journal_root();
        let store = store_in(&journal);
        let first = store.generate_pairing_code_with_door("relay").unwrap();
        store.lock_pairing_code().unwrap();
        store.revoke_pairing_code().unwrap();
        assert_eq!(store.pairing_generation().unwrap(), 0);
        let second = store.generate_pairing_code_with_door("relay").unwrap();
        assert_eq!(second.generation, first.generation + 2);
        let client = seed_client(&store, "192.0.2.1");
        let transaction = open_transaction(&store, &client, "192.0.2.1");
        store
            .complete_pairing(&transaction, &second.code, &test_binding())
            .expect("fresh code is not locked");
    }

    #[test]
    fn a_code_made_for_the_local_door_is_refused_at_solstone_me() {
        let journal = journal_root();
        let store = store_in(&journal);
        let client = seed_client(&store, "192.0.2.1");
        let pairing = store.generate_pairing_code_with_door("local").unwrap();
        let transaction = open_transaction(&store, &client, "192.0.2.1");
        assert!(matches!(
            store.complete_pairing(&transaction, &pairing.code, &test_binding()),
            Err(OAuthStoreError::PairingMismatch)
        ));

        let local = RuntimeBinding::Bound {
            canonical: "https://mcp.test/mcp".to_owned(),
        };
        let transaction = open_transaction(&store, &client, "192.0.2.1");
        store
            .complete_pairing(&transaction, &pairing.code, &local)
            .expect("the local door accepts its own code");
    }

    #[test]
    fn complete_pairing_returns_stored_bindings_only() {
        let journal = journal_root();
        let store = store_in(&journal);
        let client = seed_client(&store, "192.0.2.1");
        let pairing = store.generate_pairing_code_with_door("relay").unwrap();
        let transaction = open_transaction(&store, &client, "192.0.2.1");
        let issued = store
            .complete_pairing(&transaction, &pairing.code, &test_binding())
            .unwrap();
        assert_eq!(issued.redirect_uri, "http://127.0.0.1/callback");
        assert_eq!(issued.issuer, "https://mcp.test");
        assert_eq!(issued.state.as_deref(), Some("state-1"));
    }

    #[test]
    fn redeem_is_single_use_and_split_ttls_are_independent() {
        let _guard = NowGuard;
        let start = Utc.with_ymd_and_hms(2026, 8, 31, 12, 0, 0).unwrap();
        set_now(start.timestamp());
        let journal = journal_root();
        let store = store_in(&journal);
        let client_id = seed_client(&store, "192.0.2.1");
        let pairing = store.generate_pairing_code_with_door("relay").unwrap();
        let transaction = open_transaction(&store, &client_id, "192.0.2.1");
        let issued = store
            .complete_pairing(&transaction, &pairing.code, &test_binding())
            .unwrap();
        let tokens = store
            .redeem_authorization_code(
                &issued.code,
                "https://client.example/cimd.json",
                "http://127.0.0.1/callback",
                "https://mcp.test/mcp",
                "pkce-verifier",
                &test_binding(),
            )
            .unwrap();
        assert_eq!(tokens.expires_in, 3600);
        assert!(matches!(
            store.redeem_authorization_code(
                &issued.code,
                "https://client.example/cimd.json",
                "http://127.0.0.1/callback",
                "https://mcp.test/mcp",
                "pkce-verifier",
                &test_binding(),
            ),
            Err(OAuthStoreError::InvalidToken)
        ));

        set_now(start.timestamp());
        let pairing = store.generate_pairing_code_with_door("relay").unwrap();
        let expiring = open_transaction(&store, &client_id, "192.0.2.9");
        let issued = store
            .complete_pairing(&expiring, &pairing.code, &test_binding())
            .unwrap();
        set_now(start.timestamp() + AUTH_CODE_TTL_SECS + 1);
        const { assert!(AUTH_CODE_TTL_SECS + 1 < PENDING_TRANSACTION_TTL_SECS) };
        assert!(matches!(
            store.redeem_authorization_code(
                &issued.code,
                "https://client.example/cimd.json",
                "http://127.0.0.1/callback",
                "https://mcp.test/mcp",
                "pkce-verifier",
                &test_binding(),
            ),
            Err(OAuthStoreError::CodeExpired | OAuthStoreError::InvalidToken)
        ));
        set_now(start.timestamp());
        let pairing = store.generate_pairing_code_with_door("relay").unwrap();
        let still_open = open_transaction(&store, &client_id, "192.0.2.8");
        let late = open_transaction(&store, &client_id, "192.0.2.10");
        set_now(start.timestamp() + AUTH_CODE_TTL_SECS + 1);
        store
            .complete_pairing(&still_open, &pairing.code, &test_binding())
            .expect("an unpaired transaction stays valid while its code is open");
        set_now(start.timestamp() + PENDING_TRANSACTION_TTL_SECS + 1);
        let pairing = store.generate_pairing_code_with_door("relay").unwrap();
        assert!(matches!(
            store.complete_pairing(&late, &pairing.code, &test_binding()),
            Err(OAuthStoreError::TransactionExpired | OAuthStoreError::TransactionNotFound)
        ));
    }

    #[test]
    fn redeem_rejects_mutated_bindings() {
        let journal = journal_root();
        let store = store_in(&journal);
        let client = seed_client(&store, "192.0.2.1");
        let pairing = store.generate_pairing_code_with_door("relay").unwrap();
        let transaction = open_transaction(&store, &client, "192.0.2.1");
        let issued = store
            .complete_pairing(&transaction, &pairing.code, &test_binding())
            .unwrap();
        assert!(matches!(
            store.redeem_authorization_code(
                &issued.code,
                "https://client.example/cimd.json",
                "http://127.0.0.1/other",
                "https://mcp.test/mcp",
                "pkce-verifier",
                &test_binding(),
            ),
            Err(OAuthStoreError::BindingMismatch)
        ));
        assert!(matches!(
            store.redeem_authorization_code(
                &issued.code,
                "https://client.example/cimd.json",
                "http://127.0.0.1/callback",
                "https://mcp.test/mcp",
                "pkce-verifier",
                &test_binding(),
            ),
            Err(OAuthStoreError::InvalidToken)
        ));
    }

    #[test]
    fn pending_and_client_per_source_caps() {
        let journal = journal_root();
        let store = store_in(&journal);
        let client = seed_client(&store, "192.0.2.1");
        store.generate_pairing_code_with_door("relay").unwrap();
        for _ in 0..MAX_PENDING_PER_SOURCE {
            open_transaction(&store, &client, "192.0.2.1");
        }
        let challenge = sha256_b64(b"pkce-verifier");
        assert!(matches!(
            store.create_transaction(
                &client,
                "http://127.0.0.1/callback",
                "https://mcp.test/mcp",
                "https://mcp.test",
                &challenge,
                "S256",
                None,
                "192.0.2.1",
            ),
            Err(OAuthStoreError::Quota)
        ));
        open_transaction(&store, &client, "198.51.100.2");

        let _guard = NowGuard;
        let base_now = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        set_now(base_now.timestamp());

        let path = journal.path().join("mcp-endpoint/oauth.json");
        let mut file = store.read_store().unwrap();
        file.clients.clear();
        for index in 0..MAX_CLIENTS_PER_SOURCE {
            file.clients.push(StoredClient {
                id: format!("client-{index}"),
                client_id: format!("https://client.example/{index}.json"),
                redirect_uris: vec!["http://127.0.0.1/callback".to_owned()],
                client_name: None,
                source: "203.0.113.1".to_owned(),
                created_at: base_now + Duration::seconds(index as i64),
                last_used_at: None,
                revocation_generation: 0,
            });
        }
        fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();

        let over = store
            .register_client(
                "https://client.example/over.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "203.0.113.1",
            )
            .unwrap();
        assert_eq!(over.client_id, "https://client.example/over.json");
        assert!(
            store
                .lookup_client_by_cimd_url("https://client.example/0.json")
                .unwrap()
                .is_none(),
            "oldest client of source was evicted"
        );
        assert!(
            store
                .lookup_client_by_cimd_url("https://client.example/1.json")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn entry_and_state_size_caps() {
        let _guard = NowGuard;
        let journal = journal_root();
        let store = store_in(&journal);
        let huge = "x".repeat(MAX_OAUTH_ENTRY_BYTES);
        assert!(matches!(
            store.register_client(
                "https://client.example/huge.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                Some(huge),
                "192.0.2.1",
            ),
            Err(OAuthStoreError::EntryTooLarge)
        ));

        TEST_MAX_STATE_BYTES.with(|limit| limit.set(Some(64)));
        assert!(matches!(
            store.generate_pairing_code_with_door("relay"),
            Err(OAuthStoreError::StateTooLarge)
        ));
    }

    #[test]
    fn corrupt_store_fails_closed() {
        let journal = journal_root();
        let store = store_in(&journal);
        let directory = journal.path().join("mcp-endpoint");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("oauth.json");
        fs::write(&path, b"not json").unwrap();
        assert!(store.generate_pairing_code_with_door("relay").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"not json");
    }

    #[test]
    fn incomplete_randomness_writes_nothing() {
        let journal = journal_root();
        let store = store_in(&journal);
        assert!(matches!(
            store.generate_pairing_code_with_random_and_door(&ShortRandom, "relay"),
            Err(OAuthStoreError::Randomness)
        ));
        assert!(!journal.path().join("mcp-endpoint/oauth.json").exists());
    }

    #[test]
    fn refresh_rotation_is_constant_size_and_replays_fail() {
        let _guard = NowGuard;
        let base_time = 1_700_000_000_i64;
        set_now(base_time);
        let journal = journal_root();
        let store = store_in(&journal);
        let client = seed_client(&store, "192.0.2.1");
        let pairing = store.generate_pairing_code_with_door("relay").unwrap();
        let transaction = open_transaction(&store, &client, "192.0.2.1");
        let issued = store
            .complete_pairing(&transaction, &pairing.code, &test_binding())
            .unwrap();
        let mut tokens = store
            .redeem_authorization_code(
                &issued.code,
                "https://client.example/cimd.json",
                "http://127.0.0.1/callback",
                "https://mcp.test/mcp",
                "pkce-verifier",
                &test_binding(),
            )
            .unwrap();
        let path = journal.path().join("mcp-endpoint/oauth.json");
        for rotation in 1..=REFRESH_HISTORY_LIMIT {
            set_now(base_time + rotation as i64);
            tokens = store
                .refresh_grant(
                    &tokens.refresh_token,
                    "https://client.example/cimd.json",
                    &test_binding(),
                )
                .unwrap();
        }
        let full_history_size = fs::read(&path).unwrap().len();
        set_now(base_time + REFRESH_HISTORY_LIMIT as i64 + 1);
        let previous = tokens.refresh_token.clone();
        let _after_full_history = store
            .refresh_grant(
                &tokens.refresh_token,
                "https://client.example/cimd.json",
                &test_binding(),
            )
            .unwrap();
        assert_eq!(fs::read(&path).unwrap().len(), full_history_size);

        set_now(base_time + REFRESH_HISTORY_LIMIT as i64 + 1 + 31);
        assert!(matches!(
            store.refresh_grant(
                &previous,
                "https://client.example/cimd.json",
                &test_binding()
            ),
            Err(OAuthStoreError::RefreshReused { .. })
        ));
    }

    #[test]
    fn replay_after_grace_revokes_the_grant_on_disk() {
        let _guard = NowGuard;
        let base_time = 1_800_000_000_i64;
        set_now(base_time);
        let journal = journal_root();
        let oauth = OAuthRuntime::new(journal.path(), "https://mcp.test".to_owned());
        let source = oauth.source_cohort(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)));
        let client = oauth
            .store
            .register_client(
                "https://client.example/replay.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                Some("replay fixture".to_owned()),
                &source,
            )
            .unwrap();
        let mut tokens = issue_tokens(
            &oauth.store,
            &client.id,
            &client.client_id,
            &oauth.binding(),
            "relay",
            Some(ReadPermission::default_whole_journal()),
        );
        let grant_id = tokens.token_id.clone();
        let original_refresh = tokens.refresh_token.clone();
        let path = journal.path().join("mcp-endpoint/oauth.json");
        assert!(
            PermissionStore::open(journal.path())
                .read()
                .unwrap()
                .permissions
                .iter()
                .any(|record| record.connection == format!("oauth:{grant_id}"))
        );

        let mut immediate_previous = String::new();
        for rotation in 1..=REFRESH_HISTORY_LIMIT + 1 {
            set_now(base_time + rotation as i64);
            immediate_previous = tokens.refresh_token.clone();
            tokens = oauth
                .store
                .refresh_grant(&tokens.refresh_token, &client.client_id, &oauth.binding())
                .unwrap();
        }
        let before_rejections = fs::read(&path).unwrap();
        let evicted = refresh_request(&oauth, &original_refresh, &client.client_id);
        assert_eq!(evicted.status, 400);
        assert_eq!(response_json(&evicted)["error"], "invalid_grant");
        assert_eq!(fs::read(&path).unwrap(), before_rejections);
        assert_eq!(oauth.store.list_grants().unwrap().len(), 1);

        let wrong_client =
            refresh_request(&oauth, &immediate_previous, "https://wrong.example/client");
        assert_eq!(wrong_client.status, 400);
        assert_eq!(fs::read(&path).unwrap(), before_rejections);
        let wrong_binding_runtime =
            OAuthRuntime::new_bound(journal.path(), "http://127.0.0.1:7659".to_owned());
        let wrong_binding = refresh_request(
            &wrong_binding_runtime,
            &immediate_previous,
            &client.client_id,
        );
        assert_eq!(wrong_binding.status, 400);
        assert_eq!(fs::read(&path).unwrap(), before_rejections);

        set_now(base_time + REFRESH_HISTORY_LIMIT as i64 + 1 + 31);
        let replay = refresh_request(&oauth, &immediate_previous, &client.client_id);
        assert_eq!(replay.status, 400);
        assert_eq!(
            response_json(&replay),
            serde_json::json!({"error": "invalid_grant"})
        );
        assert!(
            !oauth
                .store
                .list_grants()
                .unwrap()
                .iter()
                .any(|grant| grant.id == grant_id)
        );
        assert!(matches!(
            oauth
                .store
                .verify_access_token(&tokens.access_token, &oauth.binding()),
            Err(OAuthStoreError::InvalidToken)
        ));
        let current_refresh = refresh_request(&oauth, &tokens.refresh_token, &client.client_id);
        assert_eq!(current_refresh.status, 400);
        assert_eq!(response_json(&current_refresh)["error"], "invalid_grant");
        assert!(
            PermissionStore::open(journal.path())
                .read()
                .unwrap()
                .permissions
                .iter()
                .all(|record| record.connection != format!("oauth:{grant_id}"))
        );
        let repeated = refresh_request(&oauth, &immediate_previous, &client.client_id);
        assert_eq!(repeated.status, 400);
        assert_eq!(response_json(&repeated)["error"], "invalid_grant");
    }

    #[test]
    fn previous_refresh_grace_is_idempotent_through_30s() {
        let _guard = NowGuard;
        let base_time = 1_810_000_000_i64;
        set_now(base_time);
        let journal = journal_root();
        let store = store_in(&journal);
        let client_id = "https://client.example/grace.json";
        let client = store
            .register_client(
                client_id,
                vec!["http://127.0.0.1/callback".to_owned()],
                Some("grace fixture".to_owned()),
                "192.0.2.1",
            )
            .unwrap();
        let binding = test_binding();
        let mut first = issue_tokens(&store, &client.id, client_id, &binding, "relay", None);
        let pre_secret_refresh = first.refresh_token.clone();
        set_now(base_time + 1);
        first = store
            .refresh_grant(&first.refresh_token, client_id, &binding)
            .unwrap();
        let grace_refresh = first.refresh_token.clone();
        set_now(base_time + 2);
        let current = store
            .refresh_grant(&first.refresh_token, client_id, &binding)
            .unwrap();
        let path = journal.path().join("mcp-endpoint/oauth.json");
        let bytes_before = fs::read(&path).unwrap();
        let grant_before = store
            .read_store()
            .unwrap()
            .grants
            .into_iter()
            .find(|grant| grant.id == current.token_id)
            .unwrap();

        set_now(base_time + 2 + 30);
        let grace = refresh_request(
            &OAuthRuntime::new(journal.path(), "https://mcp.test".to_owned()),
            &grace_refresh,
            client_id,
        );
        assert_eq!(grace.status, 200);
        let grace_json = response_json(&grace);
        assert_eq!(grace_json["access_token"], current.access_token);
        assert_eq!(grace_json["refresh_token"], current.refresh_token);
        assert_eq!(fs::read(&path).unwrap(), bytes_before);
        let grant_after = store
            .read_store()
            .unwrap()
            .grants
            .into_iter()
            .find(|grant| grant.id == current.token_id)
            .unwrap();
        assert_eq!(
            grant_after.access_expires_at,
            grant_before.access_expires_at
        );
        assert_eq!(
            grant_after.refresh_generation,
            grant_before.refresh_generation
        );

        let too_old_in_window = refresh_request(
            &OAuthRuntime::new(journal.path(), "https://mcp.test".to_owned()),
            &pre_secret_refresh,
            client_id,
        );
        assert_eq!(too_old_in_window.status, 400);
        assert_eq!(response_json(&too_old_in_window)["error"], "invalid_grant");
        assert!(
            store
                .list_grants()
                .unwrap()
                .iter()
                .all(|grant| grant.id != current.token_id),
            "a token two rotations back revokes the grant even inside the window"
        );

        set_now(base_time + 100);
        let pre_secret_grant = issue_tokens(&store, &client.id, client_id, &binding, "relay", None);
        set_now(base_time + 101);
        let once_rotated = store
            .refresh_grant(&pre_secret_grant.refresh_token, client_id, &binding)
            .unwrap();
        set_now(base_time + 102);
        let bytes_before_first_grace = fs::read(&path).unwrap();
        let first_rotation_grace = refresh_request(
            &OAuthRuntime::new(journal.path(), "https://mcp.test".to_owned()),
            &pre_secret_grant.refresh_token,
            client_id,
        );
        assert_eq!(
            first_rotation_grace.status, 200,
            "a grant's first rotation has the same grace as any other"
        );
        let first_rotation_json = response_json(&first_rotation_grace);
        assert_eq!(
            first_rotation_json["access_token"],
            once_rotated.access_token
        );
        assert_eq!(
            first_rotation_json["refresh_token"],
            once_rotated.refresh_token
        );
        assert_eq!(fs::read(&path).unwrap(), bytes_before_first_grace);
        set_now(base_time + 101 + 31);
        let pre_secret_replay = refresh_request(
            &OAuthRuntime::new(journal.path(), "https://mcp.test".to_owned()),
            &pre_secret_grant.refresh_token,
            client_id,
        );
        assert_eq!(pre_secret_replay.status, 400);
        assert_eq!(response_json(&pre_secret_replay)["error"], "invalid_grant");
        assert!(
            store
                .list_grants()
                .unwrap()
                .iter()
                .all(|grant| grant.id != once_rotated.token_id)
        );

        set_now(base_time + 200);
        let mut late = issue_tokens(&store, &client.id, client_id, &binding, "relay", None);
        set_now(base_time + 201);
        late = store
            .refresh_grant(&late.refresh_token, client_id, &binding)
            .unwrap();
        let late_previous = late.refresh_token.clone();
        set_now(base_time + 202);
        late = store
            .refresh_grant(&late.refresh_token, client_id, &binding)
            .unwrap();
        set_now(base_time + 202 + 31);
        let expired_grace = refresh_request(
            &OAuthRuntime::new(journal.path(), "https://mcp.test".to_owned()),
            &late_previous,
            client_id,
        );
        assert_eq!(expired_grace.status, 400);
        assert_eq!(response_json(&expired_grace)["error"], "invalid_grant");
        assert!(
            store
                .list_grants()
                .unwrap()
                .iter()
                .all(|grant| grant.id != late.token_id)
        );
    }

    #[test]
    fn the_stored_file_alone_cannot_mint_a_grants_tokens() {
        let _guard = NowGuard;
        let base_time = 1_815_000_000_i64;
        set_now(base_time);
        let journal = journal_root();
        let store = store_in(&journal);
        let client_id = "https://client.example/mint.json";
        let client = store
            .register_client(
                client_id,
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "192.0.2.1",
            )
            .unwrap();
        let binding = test_binding();
        let mut tokens = issue_tokens(&store, &client.id, client_id, &binding, "relay", None);
        for rotation in 1..=3 {
            set_now(base_time + rotation);
            tokens = store
                .refresh_grant(&tokens.refresh_token, client_id, &binding)
                .unwrap();
        }
        let grant = store
            .read_store()
            .unwrap()
            .grants
            .into_iter()
            .find(|grant| grant.id == tokens.token_id)
            .unwrap();
        let parsed = crate::oauth::store::parse_refresh_verifier(&grant.refresh_verifier).unwrap();
        let secret = parsed.secret.expect("a rotated grant keeps its secret");
        let current_access = URL_SAFE_NO_PAD.decode(&tokens.access_token).unwrap();
        let current_refresh = URL_SAFE_NO_PAD.decode(&tokens.refresh_token).unwrap();
        // Everything the file offers as a derivation input: nothing, and every
        // verifier it holds, at every generation up to the next.
        let mut inputs = vec![Vec::new(), parsed.current_verifier.to_vec()];
        inputs.extend(parsed.history.iter().map(|entry| entry.verifier.to_vec()));
        for generation in 0..=grant.refresh_generation + 1 {
            for input in &inputs {
                let (access, refresh) =
                    crate::oauth::store::derive_token_pair(&secret, generation, input);
                assert_ne!(access.as_slice(), current_access.as_slice());
                assert_ne!(refresh.as_slice(), current_refresh.as_slice());
            }
        }
    }

    #[test]
    fn oauth_store_holds_no_token_plaintext() {
        let _guard = NowGuard;
        let base_time = 1_820_000_000_i64;
        set_now(base_time);
        let journal = journal_root();
        let store = store_in(&journal);
        let client_id = "https://client.example/plaintext.json";
        let client = store
            .register_client(
                client_id,
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "192.0.2.1",
            )
            .unwrap();
        let binding = test_binding();
        let mut tokens = issue_tokens(&store, &client.id, client_id, &binding, "relay", None);
        let mut issued = vec![tokens.access_token.clone(), tokens.refresh_token.clone()];
        for rotation in 1..=2 {
            set_now(base_time + rotation);
            tokens = store
                .refresh_grant(&tokens.refresh_token, client_id, &binding)
                .unwrap();
            issued.push(tokens.access_token.clone());
            issued.push(tokens.refresh_token.clone());
            if rotation == 2 {
                set_now(base_time + rotation + 30);
                let grace = refresh_request(
                    &OAuthRuntime::new(journal.path(), "https://mcp.test".to_owned()),
                    &issued[issued.len() - 3],
                    client_id,
                );
                assert_eq!(grace.status, 200);
                let grace_json = response_json(&grace);
                issued.push(grace_json["access_token"].as_str().unwrap().to_owned());
                issued.push(grace_json["refresh_token"].as_str().unwrap().to_owned());
            }
        }
        fn files_below(path: &std::path::Path, output: &mut Vec<Vec<u8>>) {
            for entry in fs::read_dir(path).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    files_below(&path, output);
                } else {
                    output.push(fs::read(path).unwrap());
                }
            }
        }
        let mut files = Vec::new();
        files_below(&journal.path().join("mcp-endpoint"), &mut files);
        for token in issued {
            assert!(
                files.iter().all(|bytes| !bytes
                    .windows(token.len())
                    .any(|window| window == token.as_bytes())),
                "issued token plaintext appeared on disk"
            );
        }
    }

    #[test]
    fn schema_1_grants_migrate_and_emitted_file_is_unsupported_schema() {
        let _guard = NowGuard;
        let base_time = 1_830_000_000_i64;
        set_now(base_time);
        let journal = journal_root();
        let store = store_in(&journal);
        let path = journal.path().join("mcp-endpoint/oauth.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        const CLIENT_ID: &str = "https://client.example/migration.json";
        let created_at = "2026-01-01T00:00:00Z";
        let access_values = [0x41_u8, 0x42, 0x43, 0x44];
        let refresh_values = [0x51_u8, 0x52, 0x53, 0x54];
        let grant_inputs = [
            (
                "grant-local",
                access_values[0],
                refresh_values[0],
                Some(solstone_core_journal_config::MCP_LOCAL_DOOR_RESOURCE),
                None,
            ),
            (
                "grant-lan",
                access_values[1],
                refresh_values[1],
                Some(solstone_core_journal_config::MCP_LAN_DOOR_RESOURCE),
                None,
            ),
            (
                "grant-hostname",
                access_values[2],
                refresh_values[2],
                Some("https://journal.example/mcp"),
                Some(1_u64),
            ),
            (
                "grant-relay",
                access_values[3],
                refresh_values[3],
                None,
                None,
            ),
        ];
        let mut old_refresh_tokens = Vec::new();
        let mut grants = Vec::new();
        for (id, access_byte, refresh_byte, resource, generation) in grant_inputs {
            let access_bytes = [access_byte; 32];
            let refresh_bytes = [refresh_byte; 32];
            old_refresh_tokens.push((id, URL_SAFE_NO_PAD.encode(refresh_bytes)));
            let mut grant = serde_json::json!({
                "id": id,
                "client_record_id": "migration-client-record",
                "client_id": CLIENT_ID,
                "access_verifier": sha256_b64(&access_bytes),
                "refresh_verifier": sha256_b64(&refresh_bytes),
                "refresh_generation": 0,
                "revocation_generation": 0,
                "access_expires_at": "2099-12-31T23:59:59Z",
                "refresh_expires_at": "2099-12-31T23:59:59Z",
                "created_at": created_at,
            });
            if let Some(resource) = resource {
                grant["resource"] = serde_json::json!(resource);
            }
            if let Some(generation) = generation {
                grant["generation"] = serde_json::json!(generation);
            }
            grants.push(grant);
        }
        let schema_1 = serde_json::json!({
            "schema": 1,
            "pairing_generation": 0,
            "clients": [{
                "id": "migration-client-record",
                "client_id": CLIENT_ID,
                "redirect_uris": ["http://127.0.0.1/callback"],
                "client_name": "migration fixture",
                "source": "192.0.2.1",
                "created_at": created_at,
                "last_used_at": created_at,
                "revocation_generation": 0,
            }],
            "grants": grants,
            "pending": [],
            "pairing": null,
        });
        fs::write(&path, serde_json::to_vec(&schema_1).unwrap()).unwrap();
        let initial_bytes = fs::read(&path).unwrap();
        assert_eq!(store.list_grants().unwrap().len(), 4);
        assert_eq!(fs::read(&path).unwrap(), initial_bytes);

        #[allow(dead_code)]
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct LegacyOAuthStoreFile {
            schema: u32,
            #[serde(default)]
            pairing_generation: u64,
            clients: Vec<LegacyStoredClient>,
            grants: Vec<LegacyStoredGrant>,
            pending: Vec<LegacyStoredPending>,
            pairing: Option<LegacyStoredPairing>,
        }
        #[allow(dead_code)]
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct LegacyStoredClient {
            id: String,
            client_id: String,
            redirect_uris: Vec<String>,
            client_name: Option<String>,
            source: String,
            created_at: chrono::DateTime<Utc>,
            last_used_at: Option<chrono::DateTime<Utc>>,
            revocation_generation: u64,
        }
        #[allow(dead_code)]
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct LegacyStoredGrant {
            id: String,
            client_record_id: String,
            client_id: String,
            access_verifier: String,
            refresh_verifier: String,
            refresh_generation: u64,
            revocation_generation: u64,
            access_expires_at: chrono::DateTime<Utc>,
            refresh_expires_at: chrono::DateTime<Utc>,
            created_at: chrono::DateTime<Utc>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            resource: Option<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            generation: Option<u64>,
        }
        #[allow(dead_code)]
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct LegacyStoredPending {
            transaction_id: String,
            client_record_id: String,
            redirect_uri: String,
            resource: String,
            issuer: String,
            pkce_s256: String,
            pkce_method: String,
            state: Option<String>,
            source: String,
            created_at: chrono::DateTime<Utc>,
            expires_at: chrono::DateTime<Utc>,
            failure_count: u8,
            authorization_code_verifier: Option<String>,
            code_expires_at: Option<chrono::DateTime<Utc>>,
            #[serde(default)]
            permission: Option<ReadPermission>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            generation: Option<u64>,
            #[serde(default, skip_serializing_if = "std::ops::Not::not")]
            pairing_verified: bool,
        }
        #[allow(dead_code)]
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct LegacyStoredPairing {
            verifier: String,
            expires_at: chrono::DateTime<Utc>,
            generation: u64,
            locked: bool,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            door: Option<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            config_generation: Option<u64>,
        }

        let local = OAuthRuntime::new_bound(
            journal.path(),
            solstone_core_journal_config::MCP_LOCAL_DOOR_ORIGIN.to_owned(),
        );
        let lan = OAuthRuntime::new_lan_door(journal.path());
        let hostname = OAuthRuntime::new_byo(journal.path(), "journal.example", 1);
        let relay = OAuthRuntime::new(journal.path(), "https://mcp.test".to_owned());
        let cases = [
            (local, old_refresh_tokens[0].clone()),
            (lan, old_refresh_tokens[1].clone()),
            (hostname, old_refresh_tokens[2].clone()),
            (relay, old_refresh_tokens[3].clone()),
        ];
        for (index, (runtime, (id, refresh))) in cases.into_iter().enumerate() {
            let rotated_at = base_time + 100 * (index as i64 + 1);
            set_now(rotated_at);
            let rotated = runtime
                .store
                .refresh_grant(refresh.as_str(), CLIENT_ID, &runtime.binding())
                .unwrap();
            set_now(rotated_at + crate::oauth::store::REFRESH_GRACE_SECS);
            let retried = runtime
                .store
                .refresh_grant(refresh.as_str(), CLIENT_ID, &runtime.binding())
                .unwrap();
            assert_eq!(retried.access_token, rotated.access_token);
            assert_eq!(retried.refresh_token, rotated.refresh_token);
            set_now(rotated_at + crate::oauth::store::REFRESH_GRACE_SECS + 1);
            if index == 0 {
                let legacy: LegacyOAuthStoreFile =
                    serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                assert_ne!(legacy.schema, 1);
                assert_eq!(legacy.grants.len(), 4);
                assert_eq!(store.read_store().unwrap().schema, OAUTH_SCHEMA);
            }
            assert!(matches!(
                runtime
                    .store
                    .refresh_grant(refresh.as_str(), CLIENT_ID, &runtime.binding()),
                Err(OAuthStoreError::RefreshReused { grant_id }) if grant_id == id
            ));
            assert!(
                !store
                    .list_grants()
                    .unwrap()
                    .iter()
                    .any(|grant| grant.id == id)
            );
            assert!(!rotated.access_token.is_empty());
        }

        let mut unsupported =
            serde_json::from_slice::<serde_json::Value>(&fs::read(&path).unwrap()).unwrap();
        unsupported["schema"] = serde_json::json!(77);
        fs::write(&path, serde_json::to_vec(&unsupported).unwrap()).unwrap();
        assert!(matches!(
            store.read_store(),
            Err(OAuthStoreError::UnsupportedSchema { found: 77, .. })
        ));
        assert_eq!(MAX_REPLAY_NOTICES, 16);
    }

    #[test]
    fn proxy_source_changes_quota_buckets_only() {
        let _guard = NowGuard;
        set_now(1_840_000_000);
        let journal = journal_root();
        let runtime = OAuthRuntime::new(journal.path(), "https://mcp.test".to_owned());
        let first_ip = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1));
        let second_ip = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 2));
        let first_source = runtime.source_cohort(first_ip);
        let second_source = runtime.source_cohort(second_ip);
        for index in 0..MAX_CLIENTS_PER_SOURCE {
            let client = runtime
                .store
                .register_client(
                    &format!("https://client.example/proxy-{index}.json"),
                    vec!["http://127.0.0.1/callback".to_owned()],
                    None,
                    &first_source,
                )
                .unwrap();
            let _tokens = issue_tokens_from_source(
                &runtime.store,
                &client.id,
                &client.client_id,
                &runtime.binding(),
                "relay",
                None,
                &first_source,
            );
        }
        assert!(matches!(
            runtime.store.register_client(
                "https://client.example/proxy-over.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                &first_source,
            ),
            Err(OAuthStoreError::Quota)
        ));
        let second_client = runtime
            .store
            .register_client(
                "https://client.example/proxy-second.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                Some("second source".to_owned()),
                &second_source,
            )
            .unwrap();
        let tokens = issue_tokens_from_source(
            &runtime.store,
            &second_client.id,
            &second_client.client_id,
            &runtime.binding(),
            "relay",
            None,
            &second_source,
        );
        assert!(
            runtime
                .store
                .verify_access_token(&tokens.access_token, &runtime.binding())
                .is_ok()
        );
    }

    fn redeem_access(store: &OAuthStore, client_record_id: &str, client_id: &str) -> String {
        let pairing = store.generate_pairing_code_with_door("relay").unwrap();
        let transaction = open_transaction(store, client_record_id, "192.0.2.1");
        let issued = store
            .complete_pairing(&transaction, &pairing.code, &test_binding())
            .unwrap();
        store
            .redeem_authorization_code(
                &issued.code,
                client_id,
                "http://127.0.0.1/callback",
                "https://mcp.test/mcp",
                "pkce-verifier",
                &test_binding(),
            )
            .unwrap()
            .access_token
    }

    #[test]
    fn access_token_identity_is_the_stored_client_id() {
        let journal = journal_root();
        let store = store_in(&journal);
        let cimd_id = "https://client.example/cimd.json";
        let cimd = seed_client(&store, "192.0.2.1");
        let cimd_token = redeem_access(&store, &cimd, cimd_id);
        let cimd_verified = store
            .verify_access_token(&cimd_token, &test_binding())
            .unwrap();
        assert_eq!(cimd_verified.agent_identity, cimd_id);
        assert!(!cimd_verified.agent_identity.starts_with("oauth:dcr:"));

        let minted = "oauth:dcr:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let classic = store
            .register_client(
                minted,
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "198.51.100.2",
            )
            .unwrap();
        let classic_token = redeem_access(&store, &classic.id, minted);
        let classic_verified = store
            .verify_access_token(&classic_token, &test_binding())
            .unwrap();
        assert_eq!(classic_verified.agent_identity, minted);
        assert!(
            !classic_verified
                .agent_identity
                .starts_with("oauth:dcr:oauth:dcr:")
        );
        assert!(cimd_verified.agent_identity.contains(':'));
        assert!(classic_verified.agent_identity.contains(':'));
    }

    #[test]
    fn revoke_client_rejects_outstanding_access_tokens() {
        let journal = journal_root();
        let store = store_in(&journal);
        let client = seed_client(&store, "192.0.2.1");
        let pairing = store.generate_pairing_code_with_door("relay").unwrap();
        let transaction = open_transaction(&store, &client, "192.0.2.1");
        let issued = store
            .complete_pairing(&transaction, &pairing.code, &test_binding())
            .unwrap();
        let tokens = store
            .redeem_authorization_code(
                &issued.code,
                "https://client.example/cimd.json",
                "http://127.0.0.1/callback",
                "https://mcp.test/mcp",
                "pkce-verifier",
                &test_binding(),
            )
            .unwrap();
        store
            .verify_access_token(&tokens.access_token, &test_binding())
            .unwrap();
        store.revoke_client(&client).unwrap();
        assert!(matches!(
            store.verify_access_token(&tokens.access_token, &test_binding()),
            Err(OAuthStoreError::InvalidToken)
        ));
    }

    #[test]
    fn list_clients_includes_cimd_and_classic_ids() {
        let journal = journal_root();
        let store = store_in(&journal);
        seed_client(&store, "192.0.2.1");
        let minted = "oauth:dcr:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        store
            .register_client(
                minted,
                vec!["http://127.0.0.1/callback".to_owned()],
                Some("classic".to_owned()),
                "198.51.100.2",
            )
            .unwrap();
        let listed = store.list_clients().unwrap();
        assert_eq!(listed.len(), 2);
        assert!(
            listed
                .iter()
                .any(|client| client.client_id == "https://client.example/cimd.json")
        );
        assert!(
            listed.iter().any(|client| client.client_id == minted
                && client.client_name.as_deref() == Some("classic"))
        );
    }

    #[test]
    fn revoke_client_by_client_id_invalidates_only_that_client() {
        let journal = journal_root();
        let store = store_in(&journal);
        let cimd_id = "https://client.example/cimd.json";
        let cimd = seed_client(&store, "192.0.2.1");
        let minted = "oauth:dcr:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let classic = store
            .register_client(
                minted,
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "198.51.100.2",
            )
            .unwrap();
        let cimd_token = redeem_access(&store, &cimd, cimd_id);
        let classic_token = redeem_access(&store, &classic.id, minted);
        store.revoke_client_by_client_id(cimd_id).unwrap();
        assert!(matches!(
            store.verify_access_token(&cimd_token, &test_binding()),
            Err(OAuthStoreError::InvalidToken)
        ));
        store
            .verify_access_token(&classic_token, &test_binding())
            .expect("unrelated client remains valid");
        assert!(matches!(
            store.revoke_client_by_client_id("https://missing.example/cimd.json"),
            Err(OAuthStoreError::ClientNotFound)
        ));
        assert!(matches!(
            store.revoke_client("missing-record"),
            Err(OAuthStoreError::ClientNotFound)
        ));
    }

    #[test]
    fn unused_clients_prune_after_ttl_used_clients_survive() {
        let _guard = NowGuard;
        let journal = journal_root();
        let store = store_in(&journal);
        let start = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        set_now(start.timestamp());
        store
            .register_client(
                "https://client.example/unused.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "192.0.2.8",
            )
            .unwrap();
        let used = seed_client(&store, "192.0.2.9");
        redeem_access(&store, &used, "https://client.example/cimd.json");
        set_now(start.timestamp() + CLIENT_UNUSED_TTL_SECS + 1);
        store.generate_pairing_code_with_door("relay").unwrap();
        assert!(
            store
                .lookup_client_by_cimd_url("https://client.example/unused.json")
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .lookup_client_by_cimd_url("https://client.example/cimd.json")
                .unwrap()
                .is_some()
        );
        assert_eq!(store.list_clients().unwrap().len(), 1);
    }

    #[test]
    fn grants_are_not_evicted_at_quota() {
        let journal = journal_root();
        let store = store_in(&journal);
        let client = seed_client(&store, "192.0.2.1");
        let pairing = store.generate_pairing_code_with_door("relay").unwrap();
        let transaction = open_transaction(&store, &client, "192.0.2.1");
        let issued = store
            .complete_pairing(&transaction, &pairing.code, &test_binding())
            .unwrap();
        let path = journal.path().join("mcp-endpoint/oauth.json");
        let mut file: OAuthStoreFile = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let now = Utc.timestamp_opt(4_000_000_000, 0).unwrap();
        let token_bytes = [0x11_u8; 32];
        let access = URL_SAFE_NO_PAD.encode(token_bytes);
        let verifier = sha256_b64(&token_bytes);
        file.grants = (0..MAX_GRANTS)
            .map(|index| StoredGrant {
                id: format!("grant-{index}"),
                client_record_id: client.clone(),
                client_id: "https://client.example/cimd.json".to_owned(),
                access_verifier: verifier.clone(),
                refresh_verifier: verifier.clone(),
                refresh_generation: 0,
                revocation_generation: 0,
                access_expires_at: now,
                refresh_expires_at: now,
                created_at: now,
                resource: None,
                generation: None,
            })
            .collect();
        fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
        assert!(matches!(
            store.redeem_authorization_code(
                &issued.code,
                "https://client.example/cimd.json",
                "http://127.0.0.1/callback",
                "https://mcp.test/mcp",
                "pkce-verifier",
                &test_binding(),
            ),
            Err(OAuthStoreError::Quota)
        ));
        store.verify_access_token(&access, &test_binding()).unwrap();
    }

    #[test]
    fn cross_runtime_grant_verification_binding_vs_unbound() {
        let journal = journal_root();
        let unbound = OAuthRuntime::new(journal.path(), "https://mcp.test".to_owned());
        let bound = OAuthRuntime::new_bound(journal.path(), "http://127.0.0.1:7659".to_owned());

        let unbound_client = seed_client(&unbound.store, "192.0.2.1");
        let unbound_pairing = unbound
            .store
            .generate_pairing_code_with_door("relay")
            .unwrap();
        let unbound_tx = unbound
            .store
            .create_transaction(
                &unbound_client,
                "http://127.0.0.1/callback",
                "https://mcp.test/mcp",
                "https://mcp.test",
                &sha256_b64(b"pkce-unbound"),
                "S256",
                None,
                "192.0.2.1",
            )
            .unwrap();
        let unbound_auth = unbound
            .store
            .complete_pairing(&unbound_tx, &unbound_pairing.code, &unbound.binding())
            .unwrap();
        let unbound_tokens = unbound
            .store
            .redeem_authorization_code(
                &unbound_auth.code,
                "https://client.example/cimd.json",
                "http://127.0.0.1/callback",
                "https://mcp.test/mcp",
                "pkce-unbound",
                &unbound.binding(),
            )
            .unwrap();

        assert!(
            unbound
                .store
                .verify_access_token(&unbound_tokens.access_token, &unbound.binding())
                .is_ok()
        );
        assert!(matches!(
            bound
                .store
                .verify_access_token(&unbound_tokens.access_token, &bound.binding()),
            Err(OAuthStoreError::InvalidToken)
        ));

        let bound_pairing = bound
            .store
            .generate_pairing_code_with_door("local")
            .unwrap();
        let bound_tx = bound
            .store
            .create_transaction(
                &unbound_client,
                "http://127.0.0.1/callback",
                "http://127.0.0.1:7659/mcp",
                "http://127.0.0.1:7659",
                &sha256_b64(b"pkce-bound"),
                "S256",
                None,
                "192.0.2.1",
            )
            .unwrap();
        let bound_auth = bound
            .store
            .complete_pairing(&bound_tx, &bound_pairing.code, &bound.binding())
            .unwrap();
        let bound_tokens = bound
            .store
            .redeem_authorization_code(
                &bound_auth.code,
                "https://client.example/cimd.json",
                "http://127.0.0.1/callback",
                "http://127.0.0.1:7659/mcp",
                "pkce-bound",
                &bound.binding(),
            )
            .unwrap();

        assert!(
            bound
                .store
                .verify_access_token(&bound_tokens.access_token, &bound.binding())
                .is_ok()
        );
        assert!(matches!(
            unbound
                .store
                .verify_access_token(&bound_tokens.access_token, &unbound.binding()),
            Err(OAuthStoreError::InvalidToken)
        ));
    }

    #[test]
    fn handwritten_json_grant_loads_and_key_allowlist_matches_after_flow() {
        let journal = journal_root();
        let directory = journal.path().join("mcp-endpoint");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("oauth.json");

        let raw_token_bytes = [0x42_u8; 32];
        let presented_token = URL_SAFE_NO_PAD.encode(raw_token_bytes);
        let stored_verifier = sha256_b64(&raw_token_bytes);

        let json_content = format!(
            r#"{{
  "schema": 1,
  "pairing_generation": 0,
  "clients": [
    {{
      "id": "test-client-rec-1",
      "client_id": "https://client.example/handwritten.json",
      "redirect_uris": ["http://127.0.0.1/callback"],
      "client_name": "handwritten",
      "source": "192.0.2.1",
      "created_at": "2099-01-01T00:00:00Z",
      "last_used_at": "2099-01-01T00:00:00Z",
      "revocation_generation": 0
    }}
  ],
  "grants": [
    {{
      "id": "test-grant-1",
      "client_record_id": "test-client-rec-1",
      "client_id": "https://client.example/handwritten.json",
      "access_verifier": "{stored_verifier}",
      "refresh_verifier": "{stored_verifier}",
      "refresh_generation": 0,
      "revocation_generation": 0,
      "access_expires_at": "2099-12-31T23:59:59Z",
      "refresh_expires_at": "2099-12-31T23:59:59Z",
      "created_at": "2099-01-01T00:00:00Z"
    }}
  ],
  "pending": [],
  "pairing": null
}}"#
        );
        fs::write(&path, json_content.as_bytes()).unwrap();

        let unbound = OAuthRuntime::new(journal.path(), "https://mcp.test".to_owned());
        let bound = OAuthRuntime::new_bound(journal.path(), "http://127.0.0.1:7659".to_owned());

        assert!(
            unbound
                .store
                .verify_access_token(&presented_token, &unbound.binding())
                .is_ok()
        );
        assert!(matches!(
            bound
                .store
                .verify_access_token(&presented_token, &bound.binding()),
            Err(OAuthStoreError::InvalidToken)
        ));

        let _guard = NowGuard;
        let base_time = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        set_now(base_time.timestamp());

        let client = unbound
            .store
            .register_client(
                "https://client.example/flow.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                Some("flow".to_owned()),
                "192.0.2.20",
            )
            .unwrap();

        let pairing = unbound
            .store
            .generate_pairing_code_with_door("relay")
            .unwrap();
        let challenge = sha256_b64(b"flow-verifier");
        let tx = unbound
            .store
            .create_transaction(
                &client.id,
                "http://127.0.0.1/callback",
                "https://mcp.test/mcp",
                "https://mcp.test",
                &challenge,
                "S256",
                Some("flow-state"),
                "192.0.2.20",
            )
            .unwrap();
        let auth = unbound
            .store
            .complete_pairing(&tx, &pairing.code, &unbound.binding())
            .unwrap();
        let tokens = unbound
            .store
            .redeem_authorization_code(
                &auth.code,
                "https://client.example/flow.json",
                "http://127.0.0.1/callback",
                "https://mcp.test/mcp",
                "flow-verifier",
                &unbound.binding(),
            )
            .unwrap();
        let _ = unbound
            .store
            .refresh_grant(
                &tokens.refresh_token,
                "https://client.example/flow.json",
                &unbound.binding(),
            )
            .unwrap();

        for i in 0..MAX_CLIENTS_PER_SOURCE {
            unbound
                .store
                .register_client(
                    &format!("https://client.example/evict_{i}.json"),
                    vec!["http://127.0.0.1/callback".to_owned()],
                    None,
                    "192.0.2.30",
                )
                .unwrap();
        }

        let _ = unbound
            .store
            .generate_pairing_code_with_door("relay")
            .unwrap();
        let _ = unbound
            .store
            .create_transaction(
                &client.id,
                "http://127.0.0.1/callback",
                "https://mcp.test/mcp",
                "https://mcp.test",
                &challenge,
                "S256",
                None,
                "192.0.2.20",
            )
            .unwrap();

        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).expect("reads json");

        let file_allowed: HashSet<&str> = [
            "schema",
            "pairing_generation",
            "clients",
            "grants",
            "pending",
            "pairing",
        ]
        .into_iter()
        .collect();
        let client_allowed: HashSet<&str> = [
            "id",
            "client_id",
            "redirect_uris",
            "client_name",
            "source",
            "created_at",
            "last_used_at",
            "revocation_generation",
        ]
        .into_iter()
        .collect();
        let grant_allowed: HashSet<&str> = [
            "id",
            "client_record_id",
            "client_id",
            "access_verifier",
            "refresh_verifier",
            "refresh_generation",
            "revocation_generation",
            "access_expires_at",
            "refresh_expires_at",
            "created_at",
        ]
        .into_iter()
        .collect();
        let pending_allowed: HashSet<&str> = [
            "transaction_id",
            "client_record_id",
            "redirect_uri",
            "resource",
            "issuer",
            "pkce_s256",
            "pkce_method",
            "state",
            "source",
            "created_at",
            "expires_at",
            "failure_count",
            "authorization_code_verifier",
            "code_expires_at",
            "permission",
            "pairing_window",
        ]
        .into_iter()
        .collect();
        let pairing_allowed: HashSet<&str> = [
            "verifier",
            "expires_at",
            "generation",
            "locked",
            "door",
            "config_generation",
        ]
        .into_iter()
        .collect();

        for key in value.as_object().unwrap().keys() {
            assert!(
                file_allowed.contains(key.as_str()),
                "unknown file key {key}"
            );
        }
        for client in value["clients"].as_array().unwrap() {
            for key in client.as_object().unwrap().keys() {
                assert!(
                    client_allowed.contains(key.as_str()),
                    "unknown client key {key}"
                );
            }
        }
        for grant in value["grants"].as_array().unwrap() {
            for key in grant.as_object().unwrap().keys() {
                assert!(
                    grant_allowed.contains(key.as_str()),
                    "unknown grant key {key}"
                );
            }
        }
        for pending in value["pending"].as_array().unwrap() {
            for key in pending.as_object().unwrap().keys() {
                assert!(
                    pending_allowed.contains(key.as_str()),
                    "unknown pending key {key}"
                );
            }
        }
        if let Some(pairing) = value.get("pairing").and_then(|p| p.as_object()) {
            for key in pairing.keys() {
                assert!(
                    pairing_allowed.contains(key.as_str()),
                    "unknown pairing key {key}"
                );
            }
        }
    }

    #[test]
    fn per_source_cap_evicts_oldest_idle_client_after_grant_revocation() {
        let journal = journal_root();
        let store = store_in(&journal);
        let _guard = NowGuard;
        let base_now = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        set_now(base_now.timestamp());

        let mut client_records = Vec::new();
        for index in 0..MAX_CLIENTS_PER_SOURCE {
            set_now(base_now.timestamp() + index as i64);
            let client = store
                .register_client(
                    &format!("https://client.example/revoked_{index}.json"),
                    vec!["http://127.0.0.1/callback".to_owned()],
                    None,
                    "192.0.2.50",
                )
                .unwrap();
            let pairing = store.generate_pairing_code_with_door("relay").unwrap();
            let tx = store
                .create_transaction(
                    &client.id,
                    "http://127.0.0.1/callback",
                    "https://mcp.test/mcp",
                    "https://mcp.test",
                    &sha256_b64(b"pkce-verifier"),
                    "S256",
                    None,
                    "192.0.2.50",
                )
                .unwrap();
            let auth = store
                .complete_pairing(&tx, &pairing.code, &test_binding())
                .unwrap();
            let tokens = store
                .redeem_authorization_code(
                    &auth.code,
                    &client.client_id,
                    "http://127.0.0.1/callback",
                    "https://mcp.test/mcp",
                    "pkce-verifier",
                    &test_binding(),
                )
                .unwrap();
            let _ = store.revoke_grant_by_id(&tokens.token_id).unwrap();
            client_records.push(client);
        }

        set_now(base_now.timestamp() + MAX_CLIENTS_PER_SOURCE as i64);
        let seventeenth = store
            .register_client(
                "https://client.example/seventeenth.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "192.0.2.50",
            )
            .unwrap();
        assert_eq!(
            seventeenth.client_id,
            "https://client.example/seventeenth.json"
        );
        assert!(
            store
                .lookup_client_by_cimd_url(&client_records[0].client_id)
                .unwrap()
                .is_none(),
            "oldest client was evicted"
        );
        assert!(
            store
                .lookup_client_by_cimd_url(&client_records[1].client_id)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn client_with_expired_access_and_valid_refresh_survives_eviction() {
        let journal = journal_root();
        let store = store_in(&journal);
        let _guard = NowGuard;
        let base_now = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        set_now(base_now.timestamp());

        let oldest_client = store
            .register_client(
                "https://client.example/oldest_live.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "192.0.2.60",
            )
            .unwrap();
        let pairing = store.generate_pairing_code_with_door("relay").unwrap();
        let tx = store
            .create_transaction(
                &oldest_client.id,
                "http://127.0.0.1/callback",
                "https://mcp.test/mcp",
                "https://mcp.test",
                &sha256_b64(b"pkce-verifier"),
                "S256",
                None,
                "192.0.2.60",
            )
            .unwrap();
        let auth = store
            .complete_pairing(&tx, &pairing.code, &test_binding())
            .unwrap();
        let tokens = store
            .redeem_authorization_code(
                &auth.code,
                &oldest_client.client_id,
                "http://127.0.0.1/callback",
                "https://mcp.test/mcp",
                "pkce-verifier",
                &test_binding(),
            )
            .unwrap();

        for index in 1..MAX_CLIENTS_PER_SOURCE {
            set_now(base_now.timestamp() + index as i64);
            store
                .register_client(
                    &format!("https://client.example/idle_{index}.json"),
                    vec!["http://127.0.0.1/callback".to_owned()],
                    None,
                    "192.0.2.60",
                )
                .unwrap();
        }

        set_now(base_now.timestamp() + 3600 + 10);

        let over = store
            .register_client(
                "https://client.example/over.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "192.0.2.60",
            )
            .unwrap();
        assert_eq!(over.client_id, "https://client.example/over.json");
        assert!(
            store
                .lookup_client_by_cimd_url(&oldest_client.client_id)
                .unwrap()
                .is_some(),
            "oldest client with live refresh must survive"
        );
        assert!(
            store
                .lookup_client_by_cimd_url("https://client.example/idle_1.json")
                .unwrap()
                .is_none(),
            "newer idle client was evicted"
        );

        assert!(
            store
                .refresh_grant(
                    &tokens.refresh_token,
                    &oldest_client.client_id,
                    &test_binding()
                )
                .is_ok()
        );
    }

    #[test]
    fn client_with_pending_transaction_or_issued_code_survives_eviction() {
        let journal = journal_root();
        let store = store_in(&journal);
        let _guard = NowGuard;
        let base_now = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        set_now(base_now.timestamp());

        store.generate_pairing_code_with_door("relay").unwrap();
        let pending_client = store
            .register_client(
                "https://client.example/pending_client.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "192.0.2.70",
            )
            .unwrap();
        let _tx = store
            .create_transaction(
                &pending_client.id,
                "http://127.0.0.1/callback",
                "https://mcp.test/mcp",
                "https://mcp.test",
                &sha256_b64(b"pkce-verifier"),
                "S256",
                None,
                "192.0.2.70",
            )
            .unwrap();

        for index in 1..MAX_CLIENTS_PER_SOURCE {
            set_now(base_now.timestamp() + index as i64);
            store
                .register_client(
                    &format!("https://client.example/idle_p_{index}.json"),
                    vec!["http://127.0.0.1/callback".to_owned()],
                    None,
                    "192.0.2.70",
                )
                .unwrap();
        }

        set_now(base_now.timestamp() + MAX_CLIENTS_PER_SOURCE as i64);
        let over = store
            .register_client(
                "https://client.example/over_p.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "192.0.2.70",
            )
            .unwrap();
        assert_eq!(over.client_id, "https://client.example/over_p.json");
        assert!(
            store
                .lookup_client_by_cimd_url(&pending_client.client_id)
                .unwrap()
                .is_some()
        );
        assert!(
            store
                .lookup_client_by_cimd_url("https://client.example/idle_p_1.json")
                .unwrap()
                .is_none()
        );

        let t = 1_800_000_000;
        set_now(t);
        let code_client = store
            .register_client(
                "https://client.example/code_client.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "192.0.2.71",
            )
            .unwrap();
        let pairing = store.generate_pairing_code_with_door("relay").unwrap();
        let tx = store
            .create_transaction(
                &code_client.id,
                "http://127.0.0.1/callback",
                "https://mcp.test/mcp",
                "https://mcp.test",
                &sha256_b64(b"pkce-verifier"),
                "S256",
                None,
                "192.0.2.71",
            )
            .unwrap();
        set_now(t + 400);
        let _auth = store
            .complete_pairing(&tx, &pairing.code, &test_binding())
            .unwrap();

        for index in 1..MAX_CLIENTS_PER_SOURCE {
            set_now(t + 400 + index as i64);
            store
                .register_client(
                    &format!("https://client.example/idle_c_{index}.json"),
                    vec!["http://127.0.0.1/callback".to_owned()],
                    None,
                    "192.0.2.71",
                )
                .unwrap();
        }

        set_now(t + 650);
        let over_c = store
            .register_client(
                "https://client.example/over_c.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "192.0.2.71",
            )
            .unwrap();
        assert_eq!(over_c.client_id, "https://client.example/over_c.json");
        assert!(
            store
                .lookup_client_by_cimd_url(&code_client.client_id)
                .unwrap()
                .is_some(),
            "client with active authorization code must survive"
        );
        assert!(
            store
                .lookup_client_by_cimd_url("https://client.example/idle_c_1.json")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn client_with_unbound_grant_survives_eviction_under_bound_runtime() {
        let journal = journal_root();
        let unbound = OAuthRuntime::new(journal.path(), "https://mcp.test".to_owned());
        let bound = OAuthRuntime::new_bound(journal.path(), "http://127.0.0.1:7659".to_owned());

        let _guard = NowGuard;
        let base_now = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        set_now(base_now.timestamp());

        let client = unbound
            .store
            .register_client(
                "https://client.example/unbound_holder.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "192.0.2.80",
            )
            .unwrap();
        let pairing = unbound
            .store
            .generate_pairing_code_with_door("relay")
            .unwrap();
        let tx = unbound
            .store
            .create_transaction(
                &client.id,
                "http://127.0.0.1/callback",
                "https://mcp.test/mcp",
                "https://mcp.test",
                &sha256_b64(b"pkce-verifier"),
                "S256",
                None,
                "192.0.2.80",
            )
            .unwrap();
        let auth = unbound
            .store
            .complete_pairing(&tx, &pairing.code, &unbound.binding())
            .unwrap();
        let tokens = unbound
            .store
            .redeem_authorization_code(
                &auth.code,
                &client.client_id,
                "http://127.0.0.1/callback",
                "https://mcp.test/mcp",
                "pkce-verifier",
                &unbound.binding(),
            )
            .unwrap();

        for index in 1..MAX_CLIENTS_PER_SOURCE {
            set_now(base_now.timestamp() + index as i64);
            bound
                .store
                .register_client(
                    &format!("https://client.example/bound_idle_{index}.json"),
                    vec!["http://127.0.0.1/callback".to_owned()],
                    None,
                    "192.0.2.80",
                )
                .unwrap();
        }

        set_now(base_now.timestamp() + MAX_CLIENTS_PER_SOURCE as i64);
        let over = bound
            .store
            .register_client(
                "https://client.example/bound_over.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "192.0.2.80",
            )
            .unwrap();
        assert_eq!(over.client_id, "https://client.example/bound_over.json");
        assert!(
            bound
                .store
                .lookup_client_by_cimd_url(&client.client_id)
                .unwrap()
                .is_some()
        );
        assert!(
            bound
                .store
                .lookup_client_by_cimd_url("https://client.example/bound_idle_1.json")
                .unwrap()
                .is_none()
        );
        assert!(
            unbound
                .store
                .verify_access_token(&tokens.access_token, &unbound.binding())
                .is_ok()
        );
        assert!(
            unbound
                .store
                .refresh_grant(&tokens.refresh_token, &client.client_id, &unbound.binding())
                .is_ok()
        );
    }

    #[test]
    fn revoked_client_is_idle_and_evicted_with_permissions_cleared() {
        let journal = journal_root();
        let store = store_in(&journal);
        let perm_store = PermissionStore::open(journal.path());
        let _guard = NowGuard;
        let base_now = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        set_now(base_now.timestamp());

        let target_client = store
            .register_client(
                "https://client.example/to_be_revoked.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "192.0.2.90",
            )
            .unwrap();
        let pairing = store.generate_pairing_code_with_door("relay").unwrap();
        let tx = store
            .create_transaction(
                &target_client.id,
                "http://127.0.0.1/callback",
                "https://mcp.test/mcp",
                "https://mcp.test",
                &sha256_b64(b"pkce-verifier"),
                "S256",
                None,
                "192.0.2.90",
            )
            .unwrap();
        let auth = store
            .complete_pairing(&tx, &pairing.code, &test_binding())
            .unwrap();
        let tokens = store
            .redeem_authorization_code(
                &auth.code,
                &target_client.client_id,
                "http://127.0.0.1/callback",
                "https://mcp.test/mcp",
                "pkce-verifier",
                &test_binding(),
            )
            .unwrap();

        perm_store
            .set_permission(
                &format!("oauth:{}", tokens.token_id),
                ReadPermission::default_whole_journal(),
            )
            .unwrap();
        assert!(
            perm_store
                .get_permission(&format!("oauth:{}", tokens.token_id))
                .unwrap()
                .is_some()
        );

        store
            .revoke_client_by_client_id(&target_client.client_id)
            .unwrap();

        for index in 1..MAX_CLIENTS_PER_SOURCE {
            set_now(base_now.timestamp() + index as i64);
            store
                .register_client(
                    &format!("https://client.example/idle_rev_{index}.json"),
                    vec!["http://127.0.0.1/callback".to_owned()],
                    None,
                    "192.0.2.90",
                )
                .unwrap();
        }

        set_now(base_now.timestamp() + MAX_CLIENTS_PER_SOURCE as i64);
        let over = store
            .register_client(
                "https://client.example/rev_over.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "192.0.2.90",
            )
            .unwrap();
        assert_eq!(over.client_id, "https://client.example/rev_over.json");
        assert!(
            store
                .lookup_client_by_cimd_url(&target_client.client_id)
                .unwrap()
                .is_none(),
            "revoked client was evicted"
        );
        assert!(
            store
                .lookup_client_by_cimd_url("https://client.example/idle_rev_1.json")
                .unwrap()
                .is_some(),
            "newer idle client remained"
        );
        assert!(
            perm_store
                .get_permission(&format!("oauth:{}", tokens.token_id))
                .unwrap()
                .is_none(),
            "permission record was removed"
        );
        assert!(
            !store
                .list_grants()
                .unwrap()
                .iter()
                .any(|g| g.id == tokens.token_id)
        );
        assert!(matches!(
            store.verify_access_token(&tokens.access_token, &test_binding()),
            Err(OAuthStoreError::InvalidToken)
        ));
    }

    #[test]
    fn source_at_cap_evicts_its_own_idle_client_preserving_other_sources() {
        let journal = journal_root();
        let store = store_in(&journal);
        let _guard = NowGuard;
        let base_now = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        set_now(base_now.timestamp());

        let client_b = store
            .register_client(
                "https://client.example/source_b_older.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "198.51.100.2",
            )
            .unwrap();

        let mut a_clients = Vec::new();
        for index in 0..MAX_CLIENTS_PER_SOURCE {
            set_now(base_now.timestamp() + 100 + index as i64);
            let client = store
                .register_client(
                    &format!("https://client.example/source_a_{index}.json"),
                    vec!["http://127.0.0.1/callback".to_owned()],
                    None,
                    "192.0.2.1",
                )
                .unwrap();
            a_clients.push(client);
        }

        set_now(base_now.timestamp() + 100 + MAX_CLIENTS_PER_SOURCE as i64);
        let over = store
            .register_client(
                "https://client.example/source_a_over.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "192.0.2.1",
            )
            .unwrap();
        assert_eq!(over.client_id, "https://client.example/source_a_over.json");
        assert!(
            store
                .lookup_client_by_cimd_url(&a_clients[0].client_id)
                .unwrap()
                .is_none(),
            "A's idle client was evicted"
        );
        assert!(
            store
                .lookup_client_by_cimd_url(&client_b.client_id)
                .unwrap()
                .is_some(),
            "B's older idle client was preserved"
        );
    }

    #[test]
    fn quota_refusal_when_all_clients_live_and_reregistration_survives() {
        let journal = journal_root();
        let store = store_in(&journal);
        let _guard = NowGuard;
        let base_now = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        set_now(base_now.timestamp());

        let mut tokens_list = Vec::new();
        let mut clients = Vec::new();
        for index in 0..MAX_CLIENTS_PER_SOURCE {
            set_now(base_now.timestamp() + index as i64);
            let client = store
                .register_client(
                    &format!("https://client.example/live_{index}.json"),
                    vec!["http://127.0.0.1/callback".to_owned()],
                    None,
                    "192.0.2.100",
                )
                .unwrap();
            let pairing = store.generate_pairing_code_with_door("relay").unwrap();
            let tx = store
                .create_transaction(
                    &client.id,
                    "http://127.0.0.1/callback",
                    "https://mcp.test/mcp",
                    "https://mcp.test",
                    &sha256_b64(b"pkce-verifier"),
                    "S256",
                    None,
                    "192.0.2.100",
                )
                .unwrap();
            let auth = store
                .complete_pairing(&tx, &pairing.code, &test_binding())
                .unwrap();
            let tokens = store
                .redeem_authorization_code(
                    &auth.code,
                    &client.client_id,
                    "http://127.0.0.1/callback",
                    "https://mcp.test/mcp",
                    "pkce-verifier",
                    &test_binding(),
                )
                .unwrap();
            tokens_list.push(tokens);
            clients.push(client);
        }

        set_now(base_now.timestamp() + MAX_CLIENTS_PER_SOURCE as i64);
        assert!(matches!(
            store.register_client(
                "https://client.example/refused_17th.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "192.0.2.100",
            ),
            Err(OAuthStoreError::Quota)
        ));

        for c in &clients {
            assert!(
                store
                    .lookup_client_by_cimd_url(&c.client_id)
                    .unwrap()
                    .is_some()
            );
        }
        for t in &tokens_list {
            assert!(
                store
                    .verify_access_token(&t.access_token, &test_binding())
                    .is_ok()
            );
        }

        let existing = store
            .register_client(
                &clients[0].client_id,
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "192.0.2.100",
            )
            .unwrap();
        assert_eq!(existing.id, clients[0].id);
    }

    #[test]
    fn global_cap_evicts_oldest_idle_client_or_returns_quota_when_all_live() {
        let journal = journal_root();
        let store = store_in(&journal);
        let path = journal.path().join("mcp-endpoint/oauth.json");
        let _guard = NowGuard;
        let base_now = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        set_now(base_now.timestamp());

        let mut file = OAuthStoreFile::default();
        for index in 0..MAX_CLIENTS {
            file.clients.push(StoredClient {
                id: format!("global-client-{index}"),
                client_id: format!("https://client.example/global_{index}.json"),
                redirect_uris: vec!["http://127.0.0.1/callback".to_owned()],
                client_name: None,
                source: format!(
                    "10.{}.{}.{}",
                    (index / 65536) % 256,
                    (index / 256) % 256,
                    index % 256
                ),
                created_at: base_now + Duration::seconds(index as i64),
                last_used_at: Some(base_now),
                revocation_generation: 0,
            });
        }
        file.clients[0].source = "198.51.100.99".to_owned();
        file.clients[0].client_id = "https://client.example/b_idle.json".to_owned();

        fs::create_dir_all(journal.path().join("mcp-endpoint")).unwrap();
        fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();

        let over = store
            .register_client(
                "https://client.example/new_a.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "192.0.2.1",
            )
            .unwrap();
        assert_eq!(over.client_id, "https://client.example/new_a.json");
        assert!(
            store
                .lookup_client_by_cimd_url("https://client.example/b_idle.json")
                .unwrap()
                .is_none(),
            "B's idle client was evicted at global cap"
        );

        let mut all_live_file = OAuthStoreFile::default();
        let verifier = sha256_b64(b"test-bytes");
        for index in 0..MAX_CLIENTS {
            let cid = format!("live-client-{index}");
            all_live_file.clients.push(StoredClient {
                id: cid.clone(),
                client_id: format!("https://client.example/all_live_{index}.json"),
                redirect_uris: vec!["http://127.0.0.1/callback".to_owned()],
                client_name: None,
                source: format!(
                    "10.{}.{}.{}",
                    (index / 65536) % 256,
                    (index / 256) % 256,
                    index % 256
                ),
                created_at: base_now + Duration::seconds(index as i64),
                last_used_at: Some(base_now),
                revocation_generation: 0,
            });
            all_live_file.grants.push(StoredGrant {
                id: format!("live-grant-{index}"),
                client_record_id: cid,
                client_id: format!("https://client.example/all_live_{index}.json"),
                access_verifier: verifier.clone(),
                refresh_verifier: verifier.clone(),
                refresh_generation: 0,
                revocation_generation: 0,
                access_expires_at: base_now + Duration::seconds(3600),
                refresh_expires_at: base_now + Duration::seconds(30 * 86400),
                created_at: base_now,
                resource: None,
                generation: None,
            });
        }
        fs::write(&path, serde_json::to_vec(&all_live_file).unwrap()).unwrap();

        assert!(matches!(
            store.register_client(
                "https://client.example/refused_global.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "192.0.2.1",
            ),
            Err(OAuthStoreError::Quota)
        ));
        assert_eq!(store.list_clients().unwrap().len(), MAX_CLIENTS);
    }

    #[test]
    fn full_source_without_idle_client_is_refused_even_when_global_idle_exists() {
        let journal = journal_root();
        let store = store_in(&journal);
        let path = journal.path().join("mcp-endpoint/oauth.json");
        let _guard = NowGuard;
        let base_now = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        set_now(base_now.timestamp());

        let mut file = OAuthStoreFile::default();
        let verifier = sha256_b64(b"test-bytes");
        for index in 0..MAX_CLIENTS_PER_SOURCE {
            let cid = format!("source-a-live-{index}");
            file.clients.push(StoredClient {
                id: cid.clone(),
                client_id: format!("https://client.example/source_a_{index}.json"),
                redirect_uris: vec!["http://127.0.0.1/callback".to_owned()],
                client_name: None,
                source: "192.0.2.1".to_owned(),
                created_at: base_now + Duration::seconds(index as i64),
                last_used_at: Some(base_now),
                revocation_generation: 0,
            });
            file.grants.push(StoredGrant {
                id: format!("grant-a-{index}"),
                client_record_id: cid,
                client_id: format!("https://client.example/source_a_{index}.json"),
                access_verifier: verifier.clone(),
                refresh_verifier: verifier.clone(),
                refresh_generation: 0,
                revocation_generation: 0,
                access_expires_at: base_now + Duration::seconds(3600),
                refresh_expires_at: base_now + Duration::seconds(30 * 86400),
                created_at: base_now,
                resource: None,
                generation: None,
            });
        }
        file.clients.push(StoredClient {
            id: "source-b-idle-1".to_owned(),
            client_id: "https://client.example/source_b_idle.json".to_owned(),
            redirect_uris: vec!["http://127.0.0.1/callback".to_owned()],
            client_name: None,
            source: "198.51.100.2".to_owned(),
            created_at: base_now - Duration::seconds(100),
            last_used_at: Some(base_now),
            revocation_generation: 0,
        });

        fs::create_dir_all(journal.path().join("mcp-endpoint")).unwrap();
        fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();

        assert!(matches!(
            store.register_client(
                "https://client.example/source_a_refused.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "192.0.2.1",
            ),
            Err(OAuthStoreError::Quota)
        ));

        assert!(
            store
                .lookup_client_by_cimd_url("https://client.example/source_b_idle.json")
                .unwrap()
                .is_some(),
            "B's idle client is still present"
        );
    }

    #[test]
    fn byo_pairing_code_requires_matching_generation() {
        let journal = journal_root();
        let store = store_in(&journal);
        let byo_binding_gen1 = RuntimeBinding::Byo {
            canonical: "https://mcp.example.com/mcp".to_owned(),
            generation: 1,
        };
        let byo_binding_gen2 = RuntimeBinding::Byo {
            canonical: "https://mcp.example.com/mcp".to_owned(),
            generation: 2,
        };
        let local_binding = RuntimeBinding::Bound {
            canonical: "https://local.example.com/mcp".to_owned(),
        };
        let lan_binding = RuntimeBinding::Bound {
            canonical: solstone_core_journal_config::MCP_LAN_DOOR_RESOURCE.to_owned(),
        };

        // Write journal config with byo_hostname enabled
        let config_dir = journal.path().join("config");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("journal.json"),
            serde_json::to_string(&serde_json::json!({
                "mcp_endpoint": {
                    "byo_hostname": {
                        "hostname": "mcp.example.com",
                        "enabled": true,
                        "generation": 1
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let client = store
            .register_client(
                "https://client.example/cimd.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "byo:https://mcp.example.com/mcp",
            )
            .unwrap();

        // 1. Pairing code with door="byo" and generation=1
        let created_pairing = store.generate_pairing_code_with_door("byo").unwrap();

        let challenge = sha256_b64(b"pkce-verifier");
        let tx_id = store
            .create_transaction_with_random(
                &client.id,
                "http://127.0.0.1/callback",
                "https://mcp.example.com/mcp",
                "https://mcp.example.com",
                &challenge,
                "S256",
                None,
                "byo:127.0.0.1",
                Some(1),
                &crate::tokens::SystemRandomSource,
            )
            .unwrap();

        // Completing on local_binding or lan_binding fails
        assert!(matches!(
            store.complete_pairing(&tx_id, &created_pairing.code, &local_binding),
            Err(OAuthStoreError::PairingMismatch
                | OAuthStoreError::BindingMismatch
                | OAuthStoreError::TransactionNotFound)
        ));
        assert!(matches!(
            store.complete_pairing(&tx_id, &created_pairing.code, &lan_binding),
            Err(OAuthStoreError::PairingMismatch
                | OAuthStoreError::BindingMismatch
                | OAuthStoreError::TransactionNotFound)
        ));

        // Completing on byo_binding_gen2 fails
        assert!(matches!(
            store.complete_pairing(&tx_id, &created_pairing.code, &byo_binding_gen2),
            Err(OAuthStoreError::PairingMismatch
                | OAuthStoreError::BindingMismatch
                | OAuthStoreError::TransactionNotFound)
        ));

        // Completing on byo_binding_gen1 succeeds
        let auth = store
            .complete_pairing(&tx_id, &created_pairing.code, &byo_binding_gen1)
            .expect("matching generation completes pairing");

        assert_eq!(auth.redirect_uri, "http://127.0.0.1/callback");

        // 2. Unbound pairing code fails on BYO binding
        let unbound_pairing = store.generate_pairing_code_with_door("relay").unwrap();
        let tx_id2 = store
            .create_transaction_with_random(
                &client.id,
                "http://127.0.0.1/callback",
                "https://mcp.example.com/mcp",
                "https://mcp.example.com",
                &challenge,
                "S256",
                None,
                "byo:127.0.0.1",
                Some(1),
                &crate::tokens::SystemRandomSource,
            )
            .unwrap();

        assert!(matches!(
            store.complete_pairing(&tx_id2, &unbound_pairing.code, &byo_binding_gen1),
            Err(OAuthStoreError::PairingMismatch
                | OAuthStoreError::BindingMismatch
                | OAuthStoreError::TransactionNotFound)
        ));
    }

    #[test]
    fn byo_cohort_isolation() {
        let journal = journal_root();
        let store = store_in(&journal);
        let base_now = Utc::now();
        let path = journal.path().join("mcp-endpoint").join("oauth.json");
        let mut file = OAuthStoreFile::default();

        // Populate local cohort with 1024 idle clients
        for i in 0..1024 {
            file.clients.push(StoredClient {
                id: format!("local-client-{i}"),
                client_id: format!("https://client.example/local_{i}.json"),
                redirect_uris: vec!["http://127.0.0.1/callback".to_owned()],
                client_name: None,
                source: format!("127.0.0.{}", (i % 200) + 1),
                created_at: base_now - Duration::seconds(1000 - i as i64),
                last_used_at: None,
                revocation_generation: 0,
            });
        }
        fs::create_dir_all(journal.path().join("mcp-endpoint")).unwrap();
        fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();

        // 1 BYO registration succeeds without evicting any local clients
        let byo_client = store
            .register_client(
                "https://client.example/byo_1.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "byo:https://mcp.example.com/mcp",
            )
            .expect("byo registration succeeds under full local cohort");

        assert_eq!(byo_client.client_id, "https://client.example/byo_1.json");
        let read = store.read_store().unwrap();
        assert_eq!(read.clients.len(), 1025);

        // Same client_id in other cohort is a different row
        let same_id = "https://client.example/shared_id.json";
        let local_reg = store
            .register_client(
                same_id,
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "127.0.0.1",
            )
            .expect("local registration of same_id succeeds with eviction of oldest local idle");

        let byo_reg = store
            .register_client(
                same_id,
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "byo:https://mcp.example.com/mcp",
            )
            .expect("byo registration of same_id succeeds");

        assert_ne!(local_reg.id, byo_reg.id);
        assert_eq!(local_reg.client_id, byo_reg.client_id);
    }

    #[test]
    fn relay_pairing_code_accepted_on_unbound_refused_on_loopback_lan_and_hostname() {
        let journal = journal_root();
        let store = store_in(&journal);
        let client = seed_client(&store, "192.0.2.1");

        let unbound_binding = test_binding();
        let loopback_binding = RuntimeBinding::Bound {
            canonical: "http://127.0.0.1:7659/mcp".to_owned(),
        };
        let lan_binding = RuntimeBinding::Bound {
            canonical: solstone_core_journal_config::MCP_LAN_DOOR_RESOURCE.to_owned(),
        };
        let byo_binding = RuntimeBinding::Byo {
            canonical: "https://hostname.example/mcp".to_owned(),
            generation: 1,
        };

        // 1. Refused on loopback
        let pairing = store.generate_pairing_code_with_door("relay").unwrap();
        let tx = open_transaction_for_binding(&store, &client, &loopback_binding, "192.0.2.1");
        assert!(matches!(
            store.complete_pairing(&tx, &pairing.code, &loopback_binding),
            Err(OAuthStoreError::PairingMismatch)
        ));

        // 2. Refused on LAN
        let pairing = store.generate_pairing_code_with_door("relay").unwrap();
        let tx = open_transaction_for_binding(&store, &client, &lan_binding, "192.0.2.1");
        assert!(matches!(
            store.complete_pairing(&tx, &pairing.code, &lan_binding),
            Err(OAuthStoreError::PairingMismatch)
        ));

        // 3. Refused on BYO
        let pairing = store.generate_pairing_code_with_door("relay").unwrap();
        let tx = open_transaction_for_binding(&store, &client, &byo_binding, "192.0.2.1");
        assert!(matches!(
            store.complete_pairing(&tx, &pairing.code, &byo_binding),
            Err(OAuthStoreError::PairingMismatch)
        ));

        // 4. Accepted on unbound
        let pairing = store.generate_pairing_code_with_door("relay").unwrap();
        let tx = open_transaction_for_binding(&store, &client, &unbound_binding, "192.0.2.1");
        let completed = store
            .complete_pairing(&tx, &pairing.code, &unbound_binding)
            .expect("relay code accepted on unbound");
        assert_eq!(completed.redirect_uri, "http://127.0.0.1/callback");
    }

    #[test]
    fn doorless_pairing_code_refused_across_all_doors_and_increments_failure_count() {
        let journal = journal_root();
        let store = store_in(&journal);
        let client = seed_client(&store, "192.0.2.1");

        let code = "K7Q2M9XA";
        let verifier = sha256_b64(code.as_bytes());
        let now = Utc::now();

        let bindings = [
            test_binding(),
            RuntimeBinding::Bound {
                canonical: "http://127.0.0.1:7659/mcp".to_owned(),
            },
            RuntimeBinding::Bound {
                canonical: solstone_core_journal_config::MCP_LAN_DOOR_RESOURCE.to_owned(),
            },
            RuntimeBinding::Byo {
                canonical: "https://hostname.example/mcp".to_owned(),
                generation: 1,
            },
        ];

        for (idx, binding) in bindings.iter().enumerate() {
            // Plant a doorless pairing row
            let path = journal.path().join("mcp-endpoint/oauth.json");
            let mut file = store.read_store().unwrap();
            file.pairing = Some(super::StoredPairing {
                verifier: verifier.clone(),
                expires_at: now + Duration::seconds(600),
                generation: (idx + 1) as u64,
                locked: false,
                door: None,
                config_generation: None,
                reconnect_to: None,
            });
            fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
            assert!(
                !store.pairing_window_open(binding).unwrap(),
                "a doorless code opens no door"
            );
            let tx = open_transaction_for_binding(&store, &client, binding, "192.0.2.1");

            assert!(matches!(
                store.complete_pairing(&tx, code, binding),
                Err(OAuthStoreError::PairingMismatch)
            ));

            let read_back = store.read_store().unwrap();
            let pending_tx = read_back
                .pending
                .iter()
                .find(|p| p.transaction_id == tx)
                .expect("pending tx exists");
            assert_eq!(pending_tx.failure_count, 1);
        }
    }

    #[test]
    fn local_pairing_code_accepted_on_loopback_refused_on_unbound_lan_and_hostname() {
        let journal = journal_root();
        let store = store_in(&journal);
        let client = seed_client(&store, "192.0.2.1");

        let unbound_binding = test_binding();
        let loopback_binding = RuntimeBinding::Bound {
            canonical: "http://127.0.0.1:7659/mcp".to_owned(),
        };
        let lan_binding = RuntimeBinding::Bound {
            canonical: solstone_core_journal_config::MCP_LAN_DOOR_RESOURCE.to_owned(),
        };
        let byo_binding = RuntimeBinding::Byo {
            canonical: "https://hostname.example/mcp".to_owned(),
            generation: 1,
        };

        // 1. Refused on unbound
        let pairing = store.generate_pairing_code_with_door("local").unwrap();
        let tx = open_transaction_for_binding(&store, &client, &unbound_binding, "192.0.2.1");
        assert!(matches!(
            store.complete_pairing(&tx, &pairing.code, &unbound_binding),
            Err(OAuthStoreError::PairingMismatch)
        ));

        // 2. Refused on LAN
        let pairing = store.generate_pairing_code_with_door("local").unwrap();
        let tx = open_transaction_for_binding(&store, &client, &lan_binding, "192.0.2.1");
        assert!(matches!(
            store.complete_pairing(&tx, &pairing.code, &lan_binding),
            Err(OAuthStoreError::PairingMismatch)
        ));

        // 3. Refused on BYO
        let pairing = store.generate_pairing_code_with_door("local").unwrap();
        let tx = open_transaction_for_binding(&store, &client, &byo_binding, "192.0.2.1");
        assert!(matches!(
            store.complete_pairing(&tx, &pairing.code, &byo_binding),
            Err(OAuthStoreError::PairingMismatch)
        ));

        // 4. Accepted on loopback
        let pairing = store.generate_pairing_code_with_door("local").unwrap();
        let tx = open_transaction_for_binding(&store, &client, &loopback_binding, "192.0.2.1");
        let completed = store
            .complete_pairing(&tx, &pairing.code, &loopback_binding)
            .expect("local code accepted on loopback");
        assert_eq!(completed.redirect_uri, "http://127.0.0.1/callback");
    }

    #[test]
    fn invalid_mint_door_symbols_and_raw_resources_rejected_pre_mutate_leaving_file_identical() {
        let journal = journal_root();
        let store = store_in(&journal);

        // Case A: file does not exist yet
        let invalid_symbols = [
            "urn:solstone:mcp-door:lan",
            "https://example.com/mcp",
            "unknown",
            "",
            "byo", // byo fails because no config exists
            "local ",
            "RELAY",
        ];

        let path = journal.path().join("mcp-endpoint/oauth.json");

        for sym in invalid_symbols {
            assert!(matches!(
                store.generate_pairing_code_with_door(sym),
                Err(OAuthStoreError::BindingMismatch)
            ));
            assert!(!path.exists(), "no file created for symbol {sym}");
        }

        // Case B: file exists with initial content
        let initial_pairing = store.generate_pairing_code_with_door("relay").unwrap();
        assert_eq!(initial_pairing.door, "relay");
        let initial_bytes = fs::read(&path).unwrap();

        for sym in invalid_symbols {
            assert!(matches!(
                store.generate_pairing_code_with_door(sym),
                Err(OAuthStoreError::BindingMismatch)
            ));
            let current_bytes = fs::read(&path).unwrap();
            assert_eq!(
                initial_bytes, current_bytes,
                "file modified for symbol {sym}"
            );
        }
    }

    #[test]
    fn stored_pairing_json_keys_strictly_match_schema_with_conditional_config_generation() {
        let journal = journal_root();
        let store = store_in(&journal);
        let path = journal.path().join("mcp-endpoint/oauth.json");

        // 1. local
        let p = store.generate_pairing_code_with_door("local").unwrap();
        assert_eq!(p.door, "local");
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).expect("reads json");
        let pairing_obj = value["pairing"].as_object().unwrap();
        assert_eq!(pairing_obj["door"].as_str(), Some("local"));
        assert!(!pairing_obj.contains_key("config_generation"));

        // 2. relay
        let p = store.generate_pairing_code_with_door("relay").unwrap();
        assert_eq!(p.door, "relay");
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).expect("reads json");
        let pairing_obj = value["pairing"].as_object().unwrap();
        assert_eq!(pairing_obj["door"].as_str(), Some("relay"));
        assert!(!pairing_obj.contains_key("config_generation"));

        // 3. lan
        let p = store.generate_pairing_code_with_door("lan").unwrap();
        assert_eq!(p.door, "lan");
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).expect("reads json");
        let pairing_obj = value["pairing"].as_object().unwrap();
        assert_eq!(
            pairing_obj["door"].as_str(),
            Some(solstone_core_journal_config::MCP_LAN_DOOR_RESOURCE)
        );
        assert!(!pairing_obj.contains_key("config_generation"));

        // 4. byo
        let config_dir = journal.path().join("config");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("journal.json"),
            serde_json::to_string(&serde_json::json!({
                "mcp_endpoint": {
                    "byo_hostname": {
                        "hostname": "byo.example.com",
                        "enabled": true,
                        "generation": 42
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let p = store.generate_pairing_code_with_door("byo").unwrap();
        assert_eq!(p.door, "byo");
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).expect("reads json");
        let pairing_obj = value["pairing"].as_object().unwrap();
        assert_eq!(
            pairing_obj["door"].as_str(),
            Some("https://byo.example.com/mcp")
        );
        assert_eq!(pairing_obj["config_generation"].as_u64(), Some(42));

        let allowed_keys: HashSet<&str> = [
            "verifier",
            "expires_at",
            "generation",
            "locked",
            "door",
            "config_generation",
        ]
        .into_iter()
        .collect();

        for key in pairing_obj.keys() {
            assert!(
                allowed_keys.contains(key.as_str()),
                "unexpected key {key} in pairing json"
            );
        }
    }

    #[test]
    fn lan_pairing_code_accepted_on_lan_refused_on_loopback_unbound_and_hostname() {
        let journal = journal_root();
        let store = store_in(&journal);
        let client = seed_client(&store, "192.0.2.1");

        let unbound_binding = test_binding();
        let loopback_binding = RuntimeBinding::Bound {
            canonical: "http://127.0.0.1:7659/mcp".to_owned(),
        };
        let lan_binding = RuntimeBinding::Bound {
            canonical: solstone_core_journal_config::MCP_LAN_DOOR_RESOURCE.to_owned(),
        };
        let byo_binding = RuntimeBinding::Byo {
            canonical: "https://hostname.example/mcp".to_owned(),
            generation: 1,
        };

        // 1. Refused on unbound
        let pairing = store.generate_pairing_code_with_door("lan").unwrap();
        let tx = open_transaction_for_binding(&store, &client, &unbound_binding, "192.0.2.1");
        assert!(matches!(
            store.complete_pairing(&tx, &pairing.code, &unbound_binding),
            Err(OAuthStoreError::PairingMismatch)
        ));

        // 2. Refused on loopback
        let pairing = store.generate_pairing_code_with_door("lan").unwrap();
        let tx = open_transaction_for_binding(&store, &client, &loopback_binding, "192.0.2.1");
        assert!(matches!(
            store.complete_pairing(&tx, &pairing.code, &loopback_binding),
            Err(OAuthStoreError::PairingMismatch)
        ));

        // 3. Refused on BYO
        let pairing = store.generate_pairing_code_with_door("lan").unwrap();
        let tx = open_transaction_for_binding(&store, &client, &byo_binding, "192.0.2.1");
        assert!(matches!(
            store.complete_pairing(&tx, &pairing.code, &byo_binding),
            Err(OAuthStoreError::PairingMismatch)
        ));

        // 4. Accepted on LAN
        let pairing = store.generate_pairing_code_with_door("lan").unwrap();
        let tx = open_transaction_for_binding(&store, &client, &lan_binding, "192.0.2.1");
        let completed = store
            .complete_pairing(&tx, &pairing.code, &lan_binding)
            .expect("lan code accepted on lan");
        assert_eq!(completed.redirect_uri, "http://127.0.0.1/callback");
    }

    #[test]
    fn byo_pairing_code_accepted_on_matching_hostname_refused_on_other_doors_and_mismatched_generation()
     {
        let journal = journal_root();
        let config_dir = journal.path().join("config");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("journal.json"),
            serde_json::to_string(&serde_json::json!({
                "mcp_endpoint": {
                    "byo_hostname": {
                        "hostname": "byo.example.com",
                        "enabled": true,
                        "generation": 42
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let store = store_in(&journal);
        let client = seed_client(&store, "192.0.2.1");

        let unbound_binding = test_binding();
        let loopback_binding = RuntimeBinding::Bound {
            canonical: "http://127.0.0.1:7659/mcp".to_owned(),
        };
        let lan_binding = RuntimeBinding::Bound {
            canonical: solstone_core_journal_config::MCP_LAN_DOOR_RESOURCE.to_owned(),
        };
        let byo_matching_binding = RuntimeBinding::Byo {
            canonical: "https://byo.example.com/mcp".to_owned(),
            generation: 42,
        };
        let byo_mismatched_gen = RuntimeBinding::Byo {
            canonical: "https://byo.example.com/mcp".to_owned(),
            generation: 43,
        };
        let byo_mismatched_host = RuntimeBinding::Byo {
            canonical: "https://other.example.com/mcp".to_owned(),
            generation: 42,
        };

        // 1. Refused on unbound
        let pairing = store.generate_pairing_code_with_door("byo").unwrap();
        let tx = open_transaction_for_binding(&store, &client, &unbound_binding, "192.0.2.1");
        assert!(matches!(
            store.complete_pairing(&tx, &pairing.code, &unbound_binding),
            Err(OAuthStoreError::PairingMismatch)
        ));

        // 2. Refused on loopback
        let pairing = store.generate_pairing_code_with_door("byo").unwrap();
        let tx = open_transaction_for_binding(&store, &client, &loopback_binding, "192.0.2.1");
        assert!(matches!(
            store.complete_pairing(&tx, &pairing.code, &loopback_binding),
            Err(OAuthStoreError::PairingMismatch)
        ));

        // 3. Refused on LAN
        let pairing = store.generate_pairing_code_with_door("byo").unwrap();
        let tx = open_transaction_for_binding(&store, &client, &lan_binding, "192.0.2.1");
        assert!(matches!(
            store.complete_pairing(&tx, &pairing.code, &lan_binding),
            Err(OAuthStoreError::PairingMismatch)
        ));

        // 4. Refused on mismatched generation
        let pairing = store.generate_pairing_code_with_door("byo").unwrap();
        let tx = open_transaction_for_binding(&store, &client, &byo_mismatched_gen, "192.0.2.1");
        assert!(matches!(
            store.complete_pairing(&tx, &pairing.code, &byo_mismatched_gen),
            Err(OAuthStoreError::PairingMismatch)
        ));

        // 5. Refused on mismatched hostname
        let pairing = store.generate_pairing_code_with_door("byo").unwrap();
        let tx = open_transaction_for_binding(&store, &client, &byo_mismatched_host, "192.0.2.1");
        assert!(matches!(
            store.complete_pairing(&tx, &pairing.code, &byo_mismatched_host),
            Err(OAuthStoreError::PairingMismatch)
        ));

        // 6. Accepted on matching BYO
        let pairing = store.generate_pairing_code_with_door("byo").unwrap();
        let tx = open_transaction_for_binding(&store, &client, &byo_matching_binding, "192.0.2.1");
        let completed = store
            .complete_pairing(&tx, &pairing.code, &byo_matching_binding)
            .expect("byo code accepted on matching byo");
        assert_eq!(completed.redirect_uri, "http://127.0.0.1/callback");
    }

    #[test]
    fn stored_door_is_always_one_of_the_four_canonical_values() {
        let journal = journal_root();
        let config_dir = journal.path().join("config");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("journal.json"),
            serde_json::to_string(&serde_json::json!({
                "mcp_endpoint": {
                    "byo_hostname": {
                        "hostname": "canonical.example.com",
                        "enabled": true,
                        "generation": 1
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let store = store_in(&journal);
        let path = journal.path().join("mcp-endpoint/oauth.json");

        // local
        store.generate_pairing_code_with_door("local").unwrap();
        let value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["pairing"]["door"].as_str(), Some("local"));

        // relay
        store.generate_pairing_code_with_door("relay").unwrap();
        let value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["pairing"]["door"].as_str(), Some("relay"));

        // lan
        store.generate_pairing_code_with_door("lan").unwrap();
        let value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            value["pairing"]["door"].as_str(),
            Some(solstone_core_journal_config::MCP_LAN_DOOR_RESOURCE)
        );

        // byo
        store.generate_pairing_code_with_door("byo").unwrap();
        let value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            value["pairing"]["door"].as_str(),
            Some("https://canonical.example.com/mcp")
        );
    }

    #[test]
    fn test_redeem_takeover_without_oauth_lock_compiles() {
        let journal = tempfile::tempdir().unwrap();
        let store = store_in(&journal);
        let binding = test_binding();
        let _ = store.redeem_takeover_without_oauth_lock(
            "dummy_code",
            "dummy_client",
            "http://127.0.0.1/callback",
            binding.canonical(),
            "verifier",
            &binding,
        );
    }

    #[test]
    fn idle_pause_refresh_day_29_extends_to_day_59_and_day_31_does_not() {
        let _guard = NowGuard;
        let start = Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap();
        set_now(start.timestamp());

        let binding = test_binding();
        let agent = "https://client.example/cimd.json";

        // Journal A
        let journal_a = journal_root();
        let store_a = store_in(&journal_a);
        let client_a = store_a
            .register_client(
                agent,
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "192.0.2.1",
            )
            .unwrap();
        let tokens_a = issue_tokens(&store_a, &client_a.id, agent, &binding, "relay", None);

        // Journal B
        let journal_b = journal_root();
        let store_b = store_in(&journal_b);
        let client_b = store_b
            .register_client(
                agent,
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "192.0.2.1",
            )
            .unwrap();
        let tokens_b = issue_tokens(&store_b, &client_b.id, agent, &binding, "relay", None);

        // Journal A refreshes at start+29 days; grant_refresh_expires_at equals start+59 days.
        set_now((start + Duration::days(29)).timestamp());
        store_a
            .refresh_grant(&tokens_a.refresh_token, agent, &binding)
            .unwrap();
        assert_eq!(
            store_a
                .grant_refresh_expires_at(&tokens_a.token_id)
                .unwrap(),
            Some(start + Duration::days(59))
        );

        // Journal B does not refresh until start+31 days; refresh returns Ok,
        // the grant id is unchanged and still in list_grants, refresh_expires_at is still
        // start+30 days, and mcp-endpoint/agent-state.json does not exist.
        set_now((start + Duration::days(31)).timestamp());
        let refreshed_b = store_b
            .refresh_grant(&tokens_b.refresh_token, agent, &binding)
            .unwrap();
        assert_eq!(refreshed_b.token_id, tokens_b.token_id);
        assert!(
            store_b
                .list_grants()
                .unwrap()
                .iter()
                .any(|g| g.id == tokens_b.token_id)
        );
        assert_eq!(
            store_b
                .grant_refresh_expires_at(&tokens_b.token_id)
                .unwrap(),
            Some(start + Duration::days(30))
        );
        assert!(
            !journal_b
                .path()
                .join("mcp-endpoint/agent-state.json")
                .exists()
        );

        // A second refresh at start+60 days still returns Ok and leaves refresh_expires_at at start+30 days.
        set_now((start + Duration::days(60)).timestamp());
        let refreshed_b2 = store_b
            .refresh_grant(&refreshed_b.refresh_token, agent, &binding)
            .unwrap();
        assert_eq!(refreshed_b2.token_id, tokens_b.token_id);
        assert_eq!(
            store_b
                .grant_refresh_expires_at(&tokens_b.token_id)
                .unwrap(),
            Some(start + Duration::days(30))
        );

        // Then resume_refresh_expiry makes it now + REFRESH_TTL_SECS.
        let now_60 = start + Duration::days(60);
        let resumed = store_b.resume_refresh_expiry(&tokens_b.token_id).unwrap();
        assert_eq!(resumed, now_60 + Duration::seconds(super::REFRESH_TTL_SECS));
        assert_eq!(
            store_b
                .grant_refresh_expires_at(&tokens_b.token_id)
                .unwrap(),
            Some(now_60 + Duration::seconds(super::REFRESH_TTL_SECS))
        );
    }

    #[test]
    fn idle_pause_owner_pause_does_not_slide_refresh_expiry() {
        let _guard = NowGuard;
        let start = Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap();
        set_now(start.timestamp());

        let journal = journal_root();
        let store = store_in(&journal);
        let binding = test_binding();
        let agent = "https://client.example/cimd.json";
        let client = store
            .register_client(
                agent,
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "192.0.2.1",
            )
            .unwrap();
        let tokens = issue_tokens(&store, &client.id, agent, &binding, "relay", None);

        let conn = format!("oauth:{}", tokens.token_id);
        let random = crate::tokens::SystemRandomSource;
        crate::agent_state::set_owner_pause(journal.path(), &conn, start, &random).unwrap();

        let recorded_expiry = store.grant_refresh_expires_at(&tokens.token_id).unwrap();

        set_now((start + Duration::days(5)).timestamp());
        store
            .refresh_grant(&tokens.refresh_token, agent, &binding)
            .unwrap();

        let expiry_after = store.grant_refresh_expires_at(&tokens.token_id).unwrap();
        assert_eq!(recorded_expiry, expiry_after);
    }

    #[test]
    fn idle_pause_oldest_client_at_the_cap_survives_eviction() {
        let journal = journal_root();
        let store = store_in(&journal);
        let _guard = NowGuard;
        let base_now = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        set_now(base_now.timestamp());

        let oldest_client = store
            .register_client(
                "https://client.example/oldest_live.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "192.0.2.60",
            )
            .unwrap();
        let pairing = store.generate_pairing_code_with_door("relay").unwrap();
        let tx = store
            .create_transaction(
                &oldest_client.id,
                "http://127.0.0.1/callback",
                "https://mcp.test/mcp",
                "https://mcp.test",
                &sha256_b64(b"pkce-verifier"),
                "S256",
                None,
                "192.0.2.60",
            )
            .unwrap();
        let auth = store
            .complete_pairing(&tx, &pairing.code, &test_binding())
            .unwrap();
        let tokens = store
            .redeem_authorization_code(
                &auth.code,
                &oldest_client.client_id,
                "http://127.0.0.1/callback",
                "https://mcp.test/mcp",
                "pkce-verifier",
                &test_binding(),
            )
            .unwrap();

        for index in 1..MAX_CLIENTS_PER_SOURCE {
            set_now(base_now.timestamp() + index as i64);
            store
                .register_client(
                    &format!("https://client.example/idle_{index}.json"),
                    vec!["http://127.0.0.1/callback".to_owned()],
                    None,
                    "192.0.2.60",
                )
                .unwrap();
        }

        let expiry = store
            .grant_refresh_expires_at(&tokens.token_id)
            .unwrap()
            .expect("grant exists");
        set_now(expiry.timestamp() + 1);

        let over = store
            .register_client(
                "https://client.example/over.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                None,
                "192.0.2.60",
            )
            .unwrap();
        assert_eq!(over.client_id, "https://client.example/over.json");
        assert!(
            store
                .lookup_client_by_cimd_url(&oldest_client.client_id)
                .unwrap()
                .is_some(),
            "oldest client record is still there"
        );
        assert!(
            store
                .lookup_client_by_cimd_url("https://client.example/idle_1.json")
                .unwrap()
                .is_none(),
            "an idle client with no grant is the one gone"
        );
        assert!(
            store
                .list_grants()
                .unwrap()
                .iter()
                .any(|g| g.id == tokens.token_id),
            "the idle grant's id is still in list_grants"
        );
    }

    #[test]
    fn idle_pause_replay_after_expiry_revokes_once() {
        let _guard = NowGuard;
        let base_time = 1_800_000_000_i64;
        set_now(base_time);
        let journal = journal_root();
        let oauth = OAuthRuntime::new(journal.path(), "https://mcp.test".to_owned());
        let source = oauth.source_cohort(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)));
        let client = oauth
            .store
            .register_client(
                "https://client.example/replay.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                Some("replay fixture".to_owned()),
                &source,
            )
            .unwrap();
        let tokens0 = issue_tokens(
            &oauth.store,
            &client.id,
            &client.client_id,
            &oauth.binding(),
            "relay",
            Some(ReadPermission::default_whole_journal()),
        );
        let grant_id = tokens0.token_id.clone();

        // Refresh once
        set_now(base_time + 10);
        let tokens1 = oauth
            .store
            .refresh_grant(&tokens0.refresh_token, &client.client_id, &oauth.binding())
            .unwrap();

        // Move set_now to just past that grant's refresh_expires_at
        let expiry = oauth
            .store
            .grant_refresh_expires_at(&grant_id)
            .unwrap()
            .unwrap();
        set_now(expiry.timestamp() + 1);

        // Refresh the current token (rotates, expiry unchanged, grant still listed)
        let rotated_out_token = tokens1.refresh_token.clone();
        let tokens2 = oauth
            .store
            .refresh_grant(&rotated_out_token, &client.client_id, &oauth.binding())
            .unwrap();
        assert_eq!(
            oauth.store.grant_refresh_expires_at(&grant_id).unwrap(),
            Some(expiry)
        );
        assert!(
            oauth
                .store
                .list_grants()
                .unwrap()
                .iter()
                .any(|g| g.id == grant_id)
        );

        // A refresh of the rotated-out token with a wrong client id returns the same
        // invalid_grant shape as replay_after_grace_revokes_the_grant_on_disk and does not revoke
        let path = journal.path().join("mcp-endpoint/oauth.json");
        let before_rejections = fs::read(&path).unwrap();
        let wrong_client =
            refresh_request(&oauth, &rotated_out_token, "https://wrong.example/client");
        assert_eq!(wrong_client.status, 400);
        assert_eq!(response_json(&wrong_client)["error"], "invalid_grant");
        assert_eq!(fs::read(&path).unwrap(), before_rejections);
        assert!(
            oauth
                .store
                .list_grants()
                .unwrap()
                .iter()
                .any(|g| g.id == grant_id)
        );

        // Advance REFRESH_GRACE_SECS + 1
        set_now(expiry.timestamp() + 1 + super::REFRESH_GRACE_SECS + 1);

        // Replay the rotated-out token with the real client id
        let replay = refresh_request(&oauth, &rotated_out_token, &client.client_id);
        assert_eq!(replay.status, 400);
        assert_eq!(
            response_json(&replay),
            serde_json::json!({"error": "invalid_grant"})
        );

        // Assert the grant is gone, its permission row is gone, and list_replay_notices has one notice
        assert!(
            !oauth
                .store
                .list_grants()
                .unwrap()
                .iter()
                .any(|g| g.id == grant_id)
        );
        assert!(
            PermissionStore::open(journal.path())
                .read()
                .unwrap()
                .permissions
                .iter()
                .all(|record| record.connection != format!("oauth:{grant_id}"))
        );
        let notices = oauth.store.list_replay_notices().unwrap();
        assert_eq!(notices.len(), 1);

        // The access token from the post-expiry refresh no longer verifies
        assert!(matches!(
            oauth
                .store
                .verify_access_token(&tokens2.access_token, &oauth.binding()),
            Err(OAuthStoreError::InvalidToken)
        ));
    }

    static TAKEOVER_TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());
    struct TakeoverGuard<'a>(Option<std::sync::MutexGuard<'a, ()>>);
    impl TakeoverGuard<'_> {
        fn acquire() -> Self {
            let guard = TAKEOVER_TEST_MUTEX
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            Self(Some(guard))
        }
    }
    impl Drop for TakeoverGuard<'_> {
        fn drop(&mut self) {
            super::TAKEOVER_STOP_AFTER.with(|stop| stop.set(0));
            if let Ok(mut g) = super::I4_TEST_HOLD.lock() {
                *g = None;
            }
            drop(self.0.take());
        }
    }

    #[test]
    fn idle_pause_takeover_prefixes() {
        let _guard = NowGuard;
        let _takeover_guard = TakeoverGuard::acquire();
        let base_time = 1_800_000_000_i64;
        set_now(base_time);

        for is_paused in [true, false] {
            let max_step = if is_paused { 6 } else { 4 };
            for step in 1..=max_step {
                set_now(base_time);
                super::TAKEOVER_STOP_AFTER.with(|stop| stop.set(0));

                let journal = journal_root();
                let oauth = OAuthRuntime::new(journal.path(), "https://mcp.test".to_owned());
                let source = "192.0.2.1";

                let client_a = oauth
                    .store
                    .register_client(
                        "https://client.a/cimd.json",
                        vec!["http://127.0.0.1/callback".to_owned()],
                        Some("Client A".to_owned()),
                        source,
                    )
                    .unwrap();

                let tokens_a = issue_tokens(
                    &oauth.store,
                    &client_a.id,
                    &client_a.client_id,
                    &oauth.binding(),
                    "relay",
                    Some(ReadPermission {
                        categories: vec!["facets".to_owned()],
                        scope: crate::permissions::ReadScope::Facets {
                            ids: vec!["f1".to_owned()],
                        },
                    }),
                );
                let grant_id = tokens_a.token_id.clone();
                let conn_key = format!("oauth:{grant_id}");

                // Rotate refresh token once so we have an old rotated token too
                set_now(base_time + 10);
                let tokens_a_rot = oauth
                    .store
                    .refresh_grant(
                        &tokens_a.refresh_token,
                        &client_a.client_id,
                        &oauth.binding(),
                    )
                    .unwrap();
                let old_rotated_token = tokens_a.refresh_token.clone();
                let old_current_token = tokens_a_rot.refresh_token.clone();

                if is_paused {
                    crate::agent_state::set_owner_pause(
                        journal.path(),
                        &conn_key,
                        current_time(),
                        &crate::tokens::SystemRandomSource,
                    )
                    .unwrap();
                    PermissionStore::open(journal.path())
                        .set_evaluation_only(&conn_key, "hold")
                        .unwrap();
                }

                let perm_bytes_before =
                    fs::read(journal.path().join("mcp-endpoint/permissions.json")).unwrap();
                let current_source_before =
                    crate::agent_state::memory_source(journal.path(), &conn_key).unwrap();

                // Build a second client and a targeted code oauth:<grant_id>
                let client_b = oauth
                    .store
                    .register_client(
                        "https://client.b/cimd.json",
                        vec!["http://127.0.0.1/callback".to_owned()],
                        Some("Client B".to_owned()),
                        source,
                    )
                    .unwrap();
                let pairing_b = oauth
                    .store
                    .generate_pairing_code_with_door_and_reconnect(
                        "relay",
                        Some(format!("oauth:{grant_id}")),
                    )
                    .unwrap();
                let challenge_b = URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(b"verifier-b"));
                let tx_b = oauth
                    .store
                    .create_transaction_with_generation(
                        &client_b.id,
                        "http://127.0.0.1/callback",
                        oauth.binding().canonical(),
                        "https://mcp.test",
                        &challenge_b,
                        "S256",
                        None,
                        source,
                        oauth.binding().stored_grant_generation(),
                    )
                    .unwrap();
                let verified_b = oauth
                    .store
                    .verify_pairing(&tx_b, &pairing_b.code, &oauth.binding())
                    .unwrap();
                let auth_b = oauth
                    .store
                    .complete_confirm_pairing(
                        &verified_b,
                        Some(ReadPermission {
                            categories: vec!["facets".to_owned()],
                            scope: crate::permissions::ReadScope::Facets {
                                ids: vec!["f2".to_owned()],
                            },
                        }),
                        &oauth.binding(),
                    )
                    .unwrap()
                    .unwrap();

                let pre_store = OAuthStore::open(journal.path()).read_store().unwrap();
                let pre_grant = pre_store
                    .grants
                    .iter()
                    .find(|g| g.id == grant_id)
                    .unwrap()
                    .clone();
                let oauth_bytes_before_redeem =
                    fs::read(journal.path().join("mcp-endpoint/oauth.json")).unwrap();

                // Stop after numbered write
                super::TAKEOVER_STOP_AFTER.with(|stop| stop.set(step));
                let redeem_res = oauth.store.redeem_authorization_code(
                    &auth_b.code,
                    &client_b.client_id,
                    "http://127.0.0.1/callback",
                    oauth.binding().canonical(),
                    "verifier-b",
                    &oauth.binding(),
                );
                assert!(redeem_res.is_err());

                // Reopen stores and check:
                let store = OAuthStore::open(journal.path());
                let perm_store = PermissionStore::open(journal.path());

                if step == 1 {
                    // After 1: the grant's refresh token still verifies, permission bytes are unchanged,
                    // an owner pause is unchanged, and the mapping row is oauth:<same id> to that grant's current source component.
                    // oauth.json has no new verifiers.
                    let post_store = store.read_store().unwrap();
                    let post_grant = post_store.grants.iter().find(|g| g.id == grant_id).unwrap();
                    assert_eq!(post_grant.access_verifier, pre_grant.access_verifier);
                    assert_eq!(post_grant.refresh_verifier, pre_grant.refresh_verifier);
                    assert_eq!(
                        fs::read(journal.path().join("mcp-endpoint/oauth.json")).unwrap(),
                        oauth_bytes_before_redeem
                    );
                    assert_eq!(
                        fs::read(journal.path().join("mcp-endpoint/permissions.json")).unwrap(),
                        perm_bytes_before
                    );
                    assert_eq!(
                        crate::agent_state::is_owner_paused(journal.path(), &conn_key)
                            .unwrap()
                            .is_some(),
                        is_paused
                    );
                    assert_eq!(
                        crate::agent_state::memory_source(journal.path(), &conn_key).unwrap(),
                        current_source_before
                    );
                    let refresh_ok = store.refresh_grant(
                        &old_current_token,
                        &client_a.client_id,
                        &oauth.binding(),
                    );
                    assert!(refresh_ok.is_ok());
                } else if step == 2 {
                    // After 2: the old refresh token is InvalidToken or the HTTP invalid_grant shape,
                    // list_replay_notices is empty, the grant is still listed, and it is still paused
                    // (owner pause still there, or refresh_expires_at equal to the pinned now when it was not owner-paused).
                    // The permission row is still the old bytes.
                    assert!(matches!(
                        store.refresh_grant(
                            &old_current_token,
                            &client_a.client_id,
                            &oauth.binding()
                        ),
                        Err(OAuthStoreError::InvalidToken)
                    ));
                    assert!(store.list_replay_notices().unwrap().is_empty());
                    assert!(
                        store
                            .list_grants()
                            .unwrap()
                            .iter()
                            .any(|g| g.id == grant_id)
                    );
                    if is_paused {
                        assert!(
                            crate::agent_state::is_owner_paused(journal.path(), &conn_key)
                                .unwrap()
                                .is_some()
                        );
                    } else {
                        let exp = store.grant_refresh_expires_at(&grant_id).unwrap().unwrap();
                        assert_eq!(exp, current_time());
                    }
                    assert_eq!(
                        fs::read(journal.path().join("mcp-endpoint/permissions.json")).unwrap(),
                        perm_bytes_before
                    );
                } else if step == 3 {
                    // After 3: the permission read grant is the pending choice, generation is unchanged,
                    // evaluation stays hold if it was owner-paused and enforce otherwise, and the grant is still paused.
                    let perm_rec = perm_store
                        .read()
                        .unwrap()
                        .permissions
                        .into_iter()
                        .find(|p| p.connection == conn_key)
                        .unwrap();
                    assert_eq!(
                        perm_rec.read,
                        Some(ReadPermission {
                            categories: vec!["facets".to_owned()],
                            scope: crate::permissions::ReadScope::Facets {
                                ids: vec!["f2".to_owned()],
                            },
                        })
                    );
                    let old_perm_rec: PermissionsFile =
                        serde_json::from_slice(&perm_bytes_before).unwrap();
                    let old_p = old_perm_rec
                        .permissions
                        .into_iter()
                        .find(|p| p.connection == conn_key)
                        .unwrap();
                    assert_eq!(perm_rec.generation, old_p.generation);
                    assert_eq!(
                        perm_rec.evaluation,
                        if is_paused { "hold" } else { "enforce" }
                    );
                    if is_paused {
                        assert!(
                            crate::agent_state::is_owner_paused(journal.path(), &conn_key)
                                .unwrap()
                                .is_some()
                        );
                    } else {
                        let exp = store.grant_refresh_expires_at(&grant_id).unwrap().unwrap();
                        assert_eq!(exp, current_time());
                    }
                } else if step == 4 {
                    // After 4: refresh_expires_at is now + REFRESH_TTL_SECS. An owner-paused target is still owner-paused.
                    // An unpaused target is no longer idle.
                    let exp = store.grant_refresh_expires_at(&grant_id).unwrap().unwrap();
                    assert_eq!(
                        exp,
                        current_time() + Duration::seconds(super::REFRESH_TTL_SECS)
                    );
                    if is_paused {
                        assert!(
                            crate::agent_state::is_owner_paused(journal.path(), &conn_key)
                                .unwrap()
                                .is_some()
                        );
                    } else {
                        assert!(exp > current_time());
                    }
                } else if step == 5 {
                    // After 5, only the owner-paused case: the owner-pause row is gone and evaluation is still hold.
                    assert!(
                        crate::agent_state::is_owner_paused(journal.path(), &conn_key)
                            .unwrap()
                            .is_none()
                    );
                    let perm_rec = perm_store
                        .read()
                        .unwrap()
                        .permissions
                        .into_iter()
                        .find(|p| p.connection == conn_key)
                        .unwrap();
                    assert_eq!(perm_rec.evaluation, "hold");
                } else if step == 6 {
                    // After 6, only the owner-paused case: evaluation is enforce.
                    assert!(
                        crate::agent_state::is_owner_paused(journal.path(), &conn_key)
                            .unwrap()
                            .is_none()
                    );
                    let perm_rec = perm_store
                        .read()
                        .unwrap()
                        .permissions
                        .into_iter()
                        .find(|p| p.connection == conn_key)
                        .unwrap();
                    assert_eq!(perm_rec.evaluation, "enforce");
                }

                // From each of those prefixes, a new targeted code for the same grant runs to IssuedTokens.
                super::TAKEOVER_STOP_AFTER.with(|stop| stop.set(0));
                let client_c = oauth
                    .store
                    .register_client(
                        "https://client.c/cimd.json",
                        vec!["http://127.0.0.1/callback".to_owned()],
                        Some("Client C".to_owned()),
                        source,
                    )
                    .unwrap();
                let pairing_c = oauth
                    .store
                    .generate_pairing_code_with_door_and_reconnect(
                        "relay",
                        Some(format!("oauth:{grant_id}")),
                    )
                    .unwrap();
                let challenge_c = URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(b"verifier-c"));
                let tx_c = oauth
                    .store
                    .create_transaction_with_generation(
                        &client_c.id,
                        "http://127.0.0.1/callback",
                        oauth.binding().canonical(),
                        "https://mcp.test",
                        &challenge_c,
                        "S256",
                        None,
                        source,
                        oauth.binding().stored_grant_generation(),
                    )
                    .unwrap();
                let verified_c = oauth
                    .store
                    .verify_pairing(&tx_c, &pairing_c.code, &oauth.binding())
                    .unwrap();
                let auth_c = oauth
                    .store
                    .complete_confirm_pairing(
                        &verified_c,
                        Some(ReadPermission {
                            categories: vec!["facets".to_owned()],
                            scope: crate::permissions::ReadScope::Facets {
                                ids: vec!["f3".to_owned()],
                            },
                        }),
                        &oauth.binding(),
                    )
                    .unwrap()
                    .unwrap();

                let issued_c = oauth
                    .store
                    .redeem_authorization_code(
                        &auth_c.code,
                        &client_c.client_id,
                        "http://127.0.0.1/callback",
                        oauth.binding().canonical(),
                        "verifier-c",
                        &oauth.binding(),
                    )
                    .unwrap();

                // The grant id is unchanged.
                assert_eq!(issued_c.token_id, grant_id);

                // Old current and old rotated refresh tokens are plain invalid and write no replay notice.
                assert!(matches!(
                    oauth.store.refresh_grant(
                        &old_current_token,
                        &client_a.client_id,
                        &oauth.binding()
                    ),
                    Err(OAuthStoreError::InvalidToken)
                ));
                assert!(matches!(
                    oauth.store.refresh_grant(
                        &old_rotated_token,
                        &client_a.client_id,
                        &oauth.binding()
                    ),
                    Err(OAuthStoreError::InvalidToken)
                ));
                assert!(oauth.store.list_replay_notices().unwrap().is_empty());
                // Presented by the new client, so the client check passes: a carried
                // refresh history would answer reuse here, and none is carried.
                for old in [&old_current_token, &old_rotated_token] {
                    assert!(matches!(
                        oauth
                            .store
                            .refresh_grant(old, &client_c.client_id, &oauth.binding()),
                        Err(OAuthStoreError::InvalidToken)
                    ));
                }
                assert!(oauth.store.list_replay_notices().unwrap().is_empty());
                // The old instance's access tokens stop verifying at once.
                for old in [&tokens_a.access_token, &tokens_a_rot.access_token] {
                    assert!(matches!(
                        oauth.store.verify_access_token(old, &oauth.binding()),
                        Err(OAuthStoreError::InvalidToken)
                    ));
                }
                assert!(
                    oauth
                        .store
                        .list_grants()
                        .unwrap()
                        .iter()
                        .any(|g| g.id == grant_id)
                );

                // The new access token verifies on the new client's door.
                assert!(
                    oauth
                        .store
                        .verify_access_token(&issued_c.access_token, &oauth.binding())
                        .is_ok()
                );

                // list_grants shows the new door resource and a created_at equal to the redemption clock.
                let grants = oauth.store.list_grants().unwrap();
                let g = grants.iter().find(|g| g.id == grant_id).unwrap();
                assert_eq!(g.resource, oauth.binding().stored_grant_resource());
                assert_eq!(g.created_at, current_time());

                // A paused target is not owner-paused at the end, and refresh_expires_at is after now.
                assert!(
                    crate::agent_state::is_owner_paused(journal.path(), &conn_key)
                        .unwrap()
                        .is_none()
                );
                let exp = oauth
                    .store
                    .grant_refresh_expires_at(&grant_id)
                    .unwrap()
                    .unwrap();
                assert!(exp > current_time());

                // One day later, generate_pairing_code_with_door leaves the grant listed and the new refresh token still refreshes.
                set_now(current_time().timestamp() + 86400);
                oauth
                    .store
                    .generate_pairing_code_with_door("relay")
                    .unwrap();
                assert!(
                    oauth
                        .store
                        .list_grants()
                        .unwrap()
                        .iter()
                        .any(|g| g.id == grant_id)
                );
                assert!(
                    oauth
                        .store
                        .refresh_grant(
                            &issued_c.refresh_token,
                            &client_c.client_id,
                            &oauth.binding()
                        )
                        .is_ok()
                );
            }
        }
    }

    #[test]
    fn idle_pause_takeover_i4_second_exchange_is_locked_out() {
        let _guard = NowGuard;
        let _takeover_guard = TakeoverGuard::acquire();
        let base_time = 1_800_000_000_i64;
        set_now(base_time);

        let journal = journal_root();
        let oauth = OAuthRuntime::new(journal.path(), "https://mcp.test".to_owned());
        let source = "192.0.2.1";
        let hex = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

        let client1 = oauth
            .store
            .register_client(
                "https://client1.test/cimd.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                Some("Client 1".to_owned()),
                source,
            )
            .unwrap();
        let client2 = oauth
            .store
            .register_client(
                "https://client2.test/cimd.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                Some("Client 2".to_owned()),
                source,
            )
            .unwrap();

        // Code 1
        let pairing1 = oauth
            .store
            .generate_pairing_code_with_door_and_reconnect("relay", Some(format!("source:{hex}")))
            .unwrap();
        let challenge1 = URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(b"verifier1"));
        let tx1 = oauth
            .store
            .create_transaction_with_generation(
                &client1.id,
                "http://127.0.0.1/callback",
                oauth.binding().canonical(),
                "https://mcp.test",
                &challenge1,
                "S256",
                None,
                source,
                oauth.binding().stored_grant_generation(),
            )
            .unwrap();
        let verified1 = oauth
            .store
            .verify_pairing(&tx1, &pairing1.code, &oauth.binding())
            .unwrap();
        let auth1 = oauth
            .store
            .complete_confirm_pairing(
                &verified1,
                Some(ReadPermission::default_whole_journal()),
                &oauth.binding(),
            )
            .unwrap()
            .unwrap();

        // Code 2
        let pairing2 = oauth
            .store
            .generate_pairing_code_with_door_and_reconnect("relay", Some(format!("source:{hex}")))
            .unwrap();
        let challenge2 = URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(b"verifier2"));
        let tx2 = oauth
            .store
            .create_transaction_with_generation(
                &client2.id,
                "http://127.0.0.1/callback",
                oauth.binding().canonical(),
                "https://mcp.test",
                &challenge2,
                "S256",
                None,
                source,
                oauth.binding().stored_grant_generation(),
            )
            .unwrap();
        let verified2 = oauth
            .store
            .verify_pairing(&tx2, &pairing2.code, &oauth.binding())
            .unwrap();
        let auth2 = oauth
            .store
            .complete_confirm_pairing(
                &verified2,
                Some(ReadPermission::default_whole_journal()),
                &oauth.binding(),
            )
            .unwrap()
            .unwrap();

        // Channels for synchronization without sleep
        let (tx_to_main, rx_in_main) = std::sync::mpsc::channel();
        let (tx_to_thread, rx_in_thread) = std::sync::mpsc::channel();
        let rx_in_thread = std::sync::Mutex::new(rx_in_thread);

        if let Ok(mut g) = super::I4_TEST_HOLD.lock() {
            *g = Some((
                journal.path().to_path_buf(),
                Box::new(move || {
                    tx_to_main.send(()).unwrap();
                    rx_in_thread.lock().unwrap().recv().unwrap();
                }),
            ));
        }

        let store_thread = OAuthStore::open(journal.path());
        let binding_thread = oauth.binding();
        let code1 = auth1.code.clone();
        let client_id1 = client1.client_id.clone();

        let handle = std::thread::spawn(move || {
            store_thread.redeem_authorization_code(
                &code1,
                &client_id1,
                "http://127.0.0.1/callback",
                binding_thread.canonical(),
                "verifier1",
                &binding_thread,
            )
        });

        // Main thread waits until thread 1 blocks in I4_TEST_HOLD
        rx_in_main.recv().unwrap();

        // A second OAuthStore::open(...).with_lock_timeout(Duration::ZERO) redeem returns OAuthStoreError::Lock
        let locked_store =
            OAuthStore::open(journal.path()).with_lock_timeout(std::time::Duration::ZERO);
        let res2 = locked_store.redeem_authorization_code(
            &auth2.code,
            &client2.client_id,
            "http://127.0.0.1/callback",
            oauth.binding().canonical(),
            "verifier2",
            &oauth.binding(),
        );
        assert!(matches!(res2, Err(OAuthStoreError::Lock(_))));
        assert!(oauth.store.list_grants().unwrap().is_empty());

        // Release the first thread
        tx_to_thread.send(()).unwrap();
        let issued1 = handle.join().unwrap().unwrap();

        // It creates exactly one grant whose mapping is that hex
        let grants_after1 = oauth.store.list_grants().unwrap();
        assert_eq!(grants_after1.len(), 1);
        assert_eq!(grants_after1[0].id, issued1.token_id);
        let conn1 = format!("oauth:{}", issued1.token_id);
        assert_eq!(
            crate::agent_state::memory_source(journal.path(), &conn1)
                .unwrap()
                .component(),
            hex
        );

        // Redeeming the second code again returns InvalidGrant
        let res2_again = oauth.store.redeem_authorization_code(
            &auth2.code,
            &client2.client_id,
            "http://127.0.0.1/callback",
            oauth.binding().canonical(),
            "verifier2",
            &oauth.binding(),
        );
        assert!(matches!(res2_again, Err(OAuthStoreError::InvalidGrant)));

        // Still exactly one grant maps to that hex
        let grants_final = oauth.store.list_grants().unwrap();
        assert_eq!(grants_final.len(), 1);

        if let Ok(mut g) = super::I4_TEST_HOLD.lock() {
            *g = None;
        }
    }

    #[test]
    fn idle_pause_lock_free_mutant_can_double_grant() {
        let _guard = NowGuard;
        let _takeover_guard = TakeoverGuard::acquire();
        let base_time = 1_800_000_000_i64;
        set_now(base_time);

        let journal = journal_root();
        let oauth = OAuthRuntime::new(journal.path(), "https://mcp.test".to_owned());
        let source = "192.0.2.1";
        let hex = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

        let client1 = oauth
            .store
            .register_client(
                "https://client1.mutant/cimd.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                Some("Mutant 1".to_owned()),
                source,
            )
            .unwrap();
        let client2 = oauth
            .store
            .register_client(
                "https://client2.mutant/cimd.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                Some("Mutant 2".to_owned()),
                source,
            )
            .unwrap();

        // Code 1
        let pairing1 = oauth
            .store
            .generate_pairing_code_with_door_and_reconnect("relay", Some(format!("source:{hex}")))
            .unwrap();
        let challenge1 = URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(b"verifier-m1"));
        let tx1 = oauth
            .store
            .create_transaction_with_generation(
                &client1.id,
                "http://127.0.0.1/callback",
                oauth.binding().canonical(),
                "https://mcp.test",
                &challenge1,
                "S256",
                None,
                source,
                oauth.binding().stored_grant_generation(),
            )
            .unwrap();
        let verified1 = oauth
            .store
            .verify_pairing(&tx1, &pairing1.code, &oauth.binding())
            .unwrap();
        let auth1 = oauth
            .store
            .complete_confirm_pairing(
                &verified1,
                Some(ReadPermission::default_whole_journal()),
                &oauth.binding(),
            )
            .unwrap()
            .unwrap();

        // Code 2
        let pairing2 = oauth
            .store
            .generate_pairing_code_with_door_and_reconnect("relay", Some(format!("source:{hex}")))
            .unwrap();
        let challenge2 = URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(b"verifier-m2"));
        let tx2 = oauth
            .store
            .create_transaction_with_generation(
                &client2.id,
                "http://127.0.0.1/callback",
                oauth.binding().canonical(),
                "https://mcp.test",
                &challenge2,
                "S256",
                None,
                source,
                oauth.binding().stored_grant_generation(),
            )
            .unwrap();
        let verified2 = oauth
            .store
            .verify_pairing(&tx2, &pairing2.code, &oauth.binding())
            .unwrap();
        let auth2 = oauth
            .store
            .complete_confirm_pairing(
                &verified2,
                Some(ReadPermission::default_whole_journal()),
                &oauth.binding(),
            )
            .unwrap()
            .unwrap();

        let (tx_to_main, rx_in_main) = std::sync::mpsc::channel();
        let (tx_to_thread, rx_in_thread) = std::sync::mpsc::channel();
        let rx_in_thread = std::sync::Mutex::new(rx_in_thread);

        if let Ok(mut g) = super::I4_TEST_HOLD.lock() {
            *g = Some((
                journal.path().to_path_buf(),
                Box::new(move || {
                    tx_to_main.send(()).unwrap();
                    rx_in_thread.lock().unwrap().recv().unwrap();
                }),
            ));
        }

        let store_thread = OAuthStore::open(journal.path());
        let binding_thread = oauth.binding();
        let code1 = auth1.code.clone();
        let client_id1 = client1.client_id.clone();

        let handle = std::thread::spawn(move || {
            store_thread.redeem_takeover_without_oauth_lock(
                &code1,
                &client_id1,
                "http://127.0.0.1/callback",
                binding_thread.canonical(),
                "verifier-m1",
                &binding_thread,
            )
        });

        // Main thread waits until thread 1 triggers I4_TEST_HOLD
        rx_in_main.recv().unwrap();

        // Now thread 2 calls redeem_takeover_without_oauth_lock while thread 1 is paused
        let res2 = oauth.store.redeem_takeover_without_oauth_lock(
            &auth2.code,
            &client2.client_id,
            "http://127.0.0.1/callback",
            oauth.binding().canonical(),
            "verifier-m2",
            &oauth.binding(),
        );
        assert!(res2.is_ok());

        // Resume thread 1
        tx_to_thread.send(()).unwrap();
        let res1 = handle.join().unwrap();
        assert!(res1.is_ok());

        // Both leave two grants
        let grants = oauth.store.list_grants().unwrap();
        assert_eq!(grants.len(), 2);

        if let Ok(mut g) = super::I4_TEST_HOLD.lock() {
            *g = None;
        }
    }
}
