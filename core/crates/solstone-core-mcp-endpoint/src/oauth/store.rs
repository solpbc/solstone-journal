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
use crate::tokens::{RandomSource, RandomSourceError, SystemRandomSource, VerifiedToken};

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

/// A journal-root-bound OAuth ledger.
pub struct OAuthStore {
    root: PathBuf,
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
}

/// Public view of a registered OAuth client.
pub(crate) struct RegisteredClient {
    pub(crate) id: String,
    pub(crate) client_id: String,
    pub(crate) redirect_uris: Vec<String>,
    pub(crate) client_name: Option<String>,
    pub(crate) created_at: DateTime<Utc>,
}

/// Stored context needed to faithfully re-render a pending consent request.
pub(crate) struct PendingAuthorization {
    pub(crate) client: RegisteredClient,
    pub(crate) redirect_uri: String,
    pub(crate) pairing_verified: bool,
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
}

impl OAuthStore {
    /// Bind an OAuth store to one journal root.
    #[must_use]
    pub fn open(journal_root: &Path) -> Self {
        Self {
            root: journal_root.to_path_buf(),
        }
    }

    pub fn generate_pairing_code_with_door(
        &self,
        door: &str,
    ) -> Result<CreatedPairingCode, OAuthStoreError> {
        self.generate_pairing_code_with_random_and_door(&SystemRandomSource, door)
    }

    pub(crate) fn generate_pairing_code_with_random_and_door(
        &self,
        random: &dyn RandomSource,
        door: &str,
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

    /// Return the live pairing generation, or 0 when none is active.
    pub(crate) fn pairing_generation(&self) -> Result<u64, OAuthStoreError> {
        Ok(self
            .read_store()?
            .pairing
            .map(|pairing| pairing.generation)
            .unwrap_or(0))
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
            Ok(issue_authorization(
                &mut store.pending[index],
                Some(permission),
                &code_bytes,
                now,
            ))
        })
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
                    || store.grants[index].refresh_expires_at <= now
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
                (
                    EditPersistence::Persist,
                    Ok(IssuedTokens {
                        access_token: URL_SAFE_NO_PAD.encode(access_bytes),
                        refresh_token: URL_SAFE_NO_PAD.encode(refresh_bytes),
                        token_id: grant.id.clone(),
                        expires_in: ACCESS_TTL_SECS,
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
                        && g.refresh_expires_at > now
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
            let client = store
                .clients
                .iter_mut()
                .find(|client| client.client_id == client_id)
                .ok_or(OAuthStoreError::ClientNotFound)?;
            client.revocation_generation = client.revocation_generation.saturating_add(1);
            let client_record_id = client.id.clone();
            grant_ids = store
                .grants
                .iter()
                .filter(|g| g.client_record_id == client_record_id)
                .map(|g| g.id.clone())
                .collect();
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
        let now = current_time();
        let mut summaries = Vec::new();
        for grant in &store.grants {
            if grant.access_expires_at <= now && grant.refresh_expires_at <= now {
                continue;
            }
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
        let _lock = hold_lock(
            &path,
            LockOptions {
                mode: Some(0o600),
                ..LockOptions::default()
            },
        )
        .map_err(OAuthStoreError::Lock)?;
        let mut store = self.read_store()?;
        let now = current_time();
        prune(&mut store, now);
        let (persistence, result) = operation(&mut store, now);
        if persistence == EditPersistence::Persist {
            self.write_store(&path, &mut store)?;
            after_write()?;
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
    let verifier = decode_b64_32(&pairing.verifier).ok_or_else(|| OAuthStoreError::Malformed {
        path: PathBuf::from(OAUTH_FILE),
    })?;
    let door_matches = match pairing.door.as_deref() {
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
    };
    let matches =
        presented_digest.is_some_and(|digest| bool::from(digest.ct_eq(&verifier))) && door_matches;
    if !matches {
        store.pending[index].failure_count += 1;
        if store.pending[index].failure_count >= MAX_TRANSACTION_FAILURES {
            store.pending.remove(index);
        }
        return Err(OAuthStoreError::PairingMismatch);
    }
    store.pairing_generation = store.pairing_generation.saturating_add(1);
    store.pairing = None;
    Ok(())
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

fn prune(store: &mut OAuthStoreFile, now: DateTime<Utc>) {
    store.pending.retain(|pending| {
        if pending.authorization_code_verifier.is_some() {
            pending.code_expires_at.is_some_and(|expires| expires > now)
        } else {
            pending.expires_at > now
        }
    });
    store.grants.retain(|grant| grant.refresh_expires_at > now);
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
        Some(value) if value.starts_with("https://") => "your hostname".to_owned(),
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

fn current_time() -> DateTime<Utc> {
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
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::collections::HashSet;
    use std::fs;
    use std::net::{IpAddr, Ipv4Addr};

    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use chrono::{Duration, TimeZone, Utc};
    use serde::Deserialize;

    use super::{
        AUTH_CODE_TTL_SECS, CLIENT_UNUSED_TTL_SECS, MAX_CLIENTS, MAX_CLIENTS_PER_SOURCE,
        MAX_GRANTS, MAX_OAUTH_ENTRY_BYTES, MAX_PENDING_PER_SOURCE, MAX_REPLAY_NOTICES,
        OAUTH_SCHEMA, OAuthStore, OAuthStoreError, OAuthStoreFile, PENDING_TRANSACTION_TTL_SECS,
        REFRESH_HISTORY_LIMIT, StoredClient, StoredGrant, TEST_MAX_STATE_BYTES, TEST_NOW,
        sha256_b64,
    };
    use crate::http1::{HttpMethod, HttpRequest};
    use crate::oauth::{OAuthRuntime, RuntimeBinding};
    use crate::permissions::{PermissionStore, ReadPermission};
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
        let transaction = open_transaction(&store, &client, "192.0.2.1");
        assert!(matches!(
            store.complete_pairing(&transaction, &pairing.code, &test_binding()),
            Err(OAuthStoreError::NoActivePairing)
        ));
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
        store.lock_pairing_code().unwrap();
        assert_eq!(store.pairing_generation().unwrap(), generation);
        let transaction = open_transaction(&store, &client, "198.51.100.9");
        assert!(matches!(
            store.complete_pairing(&transaction, &pairing.code, &test_binding()),
            Err(OAuthStoreError::PairingLocked)
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
        let still_open = open_transaction(&store, &client_id, "192.0.2.8");
        let expiring = open_transaction(&store, &client_id, "192.0.2.9");
        let late = open_transaction(&store, &client_id, "192.0.2.10");
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
        let pairing = store.generate_pairing_code_with_door("relay").unwrap();
        store
            .complete_pairing(&still_open, &pairing.code, &test_binding())
            .expect("unpaired transaction remains valid for 10 minutes");
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
        let pairing = store.generate_pairing_code_with_door("relay").unwrap();
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

        // 1. Pairing code with door="byo" and generation=1
        let created_pairing = store.generate_pairing_code_with_door("byo").unwrap();

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

        let unbound_pairing = store.generate_pairing_code_with_door("relay").unwrap();

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
            let tx = open_transaction_for_binding(&store, &client, binding, "192.0.2.1");

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
            });
            fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();

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
}
