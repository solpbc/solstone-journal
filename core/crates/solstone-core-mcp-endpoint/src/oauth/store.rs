// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable journal-local OAuth ledger.

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, Utc};
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
const OAUTH_SCHEMA: u32 = 1;
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

/// A journal-root-bound OAuth ledger.
pub struct OAuthStore {
    root: PathBuf,
}

/// A newly generated pairing code. The plaintext is returned exactly once.
pub struct CreatedPairingCode {
    pub code: String,
    pub expires_at: DateTime<Utc>,
    pub generation: u64,
}

/// Non-secret state for the owner's currently active pairing window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingCodeSummary {
    pub expires_at: DateTime<Utc>,
    pub generation: u64,
    pub locked: bool,
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
}

/// Failure while operating the OAuth ledger.
#[derive(Debug)]
pub enum OAuthStoreError {
    Randomness,
    InvalidToken,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredPairing {
    verifier: String,
    expires_at: DateTime<Utc>,
    generation: u64,
    locked: bool,
}

impl OAuthStore {
    /// Bind an OAuth store to one journal root.
    #[must_use]
    pub fn open(journal_root: &Path) -> Self {
        Self {
            root: journal_root.to_path_buf(),
        }
    }

    /// Generate one pairing code, replacing any previous code.
    pub fn generate_pairing_code(&self) -> Result<CreatedPairingCode, OAuthStoreError> {
        self.generate_pairing_code_with_random(&SystemRandomSource)
    }

    pub(crate) fn generate_pairing_code_with_random(
        &self,
        random: &dyn RandomSource,
    ) -> Result<CreatedPairingCode, OAuthStoreError> {
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
            });
            Ok(CreatedPairingCode {
                code,
                expires_at,
                generation,
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
                if !binding.grant_matches(&grant.resource) {
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
    #[allow(clippy::too_many_arguments)]
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
            });
            Ok(transaction_id)
        })
    }

    /// Consume the pairing code and issue a bound authorization code.
    #[cfg(all(test, not(feature = "full-tests")))]
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
        let presented = canonicalize_pairing_code(pairing_code);
        let presented_digest = presented
            .as_ref()
            .map(|code| sha256_digest(code.as_bytes()));
        let code_bytes = random_bytes(random)?;
        let authorization_code = URL_SAFE_NO_PAD.encode(code_bytes);
        let authorization_verifier = sha256_b64(&code_bytes);
        self.mutate(|store, now| {
            let index = store
                .pending
                .iter()
                .position(|pending| pending.transaction_id == transaction_id)
                .ok_or(OAuthStoreError::TransactionNotFound)?;
            if store.pending[index].resource != binding.canonical() {
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
            let Some(pairing) = store.pairing.as_ref() else {
                return Err(OAuthStoreError::NoActivePairing);
            };
            if pairing.locked {
                return Err(OAuthStoreError::PairingLocked);
            }
            let verifier =
                decode_b64_32(&pairing.verifier).ok_or_else(|| OAuthStoreError::Malformed {
                    path: PathBuf::from(OAUTH_FILE),
                })?;
            let matches =
                presented_digest.is_some_and(|digest| bool::from(digest.ct_eq(&verifier)));
            if !matches {
                store.pending[index].failure_count += 1;
                if store.pending[index].failure_count >= MAX_TRANSACTION_FAILURES {
                    store.pending.remove(index);
                }
                return Err(OAuthStoreError::PairingMismatch);
            }
            store.pairing_generation = store.pairing_generation.saturating_add(1);
            store.pairing = None;
            let pending = &mut store.pending[index];
            pending.authorization_code_verifier = Some(authorization_verifier);
            pending.code_expires_at = Some(now + Duration::seconds(AUTH_CODE_TTL_SECS));
            pending.permission = permission;
            Ok(IssuedAuthorization {
                code: authorization_code,
                redirect_uri: pending.redirect_uri.clone(),
                state: pending.state.clone(),
                issuer: pending.issuer.clone(),
            })
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
        let presented = decode_sha256(refresh_token).ok_or(OAuthStoreError::InvalidToken)?;
        let access_bytes = random_bytes(random)?;
        let refresh_bytes = random_bytes(random)?;
        self.mutate(|store, now| {
            let mut matched = None;
            for (index, grant) in store.grants.iter().enumerate() {
                let verifier = decode_b64_32(&grant.refresh_verifier).ok_or_else(|| {
                    OAuthStoreError::Malformed {
                        path: PathBuf::from(OAUTH_FILE),
                    }
                })?;
                if bool::from(presented.ct_eq(&verifier)) && matched.is_none() {
                    matched = Some(index);
                }
            }
            let index = matched.ok_or(OAuthStoreError::InvalidToken)?;
            let client = store
                .clients
                .iter()
                .find(|client| client.id == store.grants[index].client_record_id);
            let Some(client) = client else {
                return Err(OAuthStoreError::InvalidToken);
            };
            if store.grants[index].client_id != client_id
                || store.grants[index].refresh_expires_at <= now
                || store.grants[index].revocation_generation != client.revocation_generation
                || !binding.grant_matches(&store.grants[index].resource)
            {
                return Err(OAuthStoreError::InvalidToken);
            }
            let grant = &mut store.grants[index];
            grant.access_verifier = sha256_b64(&access_bytes);
            grant.refresh_verifier = sha256_b64(&refresh_bytes);
            grant.refresh_generation = grant.refresh_generation.saturating_add(1);
            grant.access_expires_at = now + Duration::seconds(ACCESS_TTL_SECS);
            Ok(IssuedTokens {
                access_token: URL_SAFE_NO_PAD.encode(access_bytes),
                refresh_token: URL_SAFE_NO_PAD.encode(refresh_bytes),
                token_id: grant.id.clone(),
                expires_in: ACCESS_TTL_SECS,
            })
        })
    }

    /// Register a CIMD client, returning the existing record when the URL matches.
    #[cfg(all(test, not(feature = "full-tests")))]
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
            if let Some(existing) = store
                .clients
                .iter()
                .find(|client| client.client_id == client_id_owned)
            {
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

            // (a) a client is idle from DCR registration until GET /authorize creates a pending, and a returning CIMD client is idle between lookup and create_transaction, so a concurrent registration from the same source at the cap can evict it and that sign-in fails;
            // (b) a source with 16 live clients is refused until one refresh expires (at most 30 days, because refresh_expires_at is never extended) or the owner revokes that connection in the agents app. Both are no worse than today.
            let source_count = store.clients.iter().filter(|c| c.source == source).count();
            let mut evicted_target = None;

            if source_count >= MAX_CLIENTS_PER_SOURCE {
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
            } else if store.clients.len() >= MAX_CLIENTS {
                let oldest_global_idle = store
                    .clients
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| is_idle(c, store))
                    .min_by_key(|(index, c)| (c.created_at, *index))
                    .map(|(_, c)| c.id.clone());
                let Some(target_id) = oldest_global_idle else {
                    return Err(OAuthStoreError::Quota);
                };
                evicted_target = Some(target_id);
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
    ) -> Result<Option<PendingAuthorization>, OAuthStoreError> {
        let store = self.read_store()?;
        let Some(pending) = store
            .pending
            .iter()
            .find(|pending| pending.transaction_id == transaction_id)
        else {
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
        }))
    }

    /// Look up a registered client by CIMD URL.
    pub(crate) fn lookup_client_by_cimd_url(
        &self,
        client_id: &str,
    ) -> Result<Option<RegisteredClient>, OAuthStoreError> {
        Ok(self
            .read_store()?
            .clients
            .into_iter()
            .find(|client| client.client_id == client_id)
            .map(|client| registered_from(&client)))
    }

    /// Invalidate outstanding tokens for one client without deleting its record.
    #[cfg(all(test, not(feature = "full-tests")))]
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
            });
        }
        Ok(summaries)
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
        let result = operation(&mut store, now);
        match &result {
            Ok(_) => self.write_store(&path, &store)?,
            Err(OAuthStoreError::PairingMismatch)
            | Err(OAuthStoreError::TransactionExpired)
            | Err(OAuthStoreError::TransactionExhausted)
            | Err(OAuthStoreError::CodeExpired)
            | Err(OAuthStoreError::BindingMismatch)
            | Err(OAuthStoreError::Quota) => {
                self.write_store(&path, &store)?;
            }
            Err(_) => {}
        }
        result
    }

    fn endpoint_directory(&self) -> PathBuf {
        self.root.join(OAUTH_DIRECTORY)
    }

    fn oauth_path(&self) -> PathBuf {
        self.endpoint_directory().join(OAUTH_FILE)
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
        if store.schema != OAUTH_SCHEMA {
            return Err(OAuthStoreError::UnsupportedSchema {
                path,
                found: store.schema,
            });
        }
        Ok(store)
    }

    fn write_store(&self, path: &Path, store: &OAuthStoreFile) -> Result<(), OAuthStoreError> {
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

    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use chrono::{Duration, TimeZone, Utc};

    use super::{
        AUTH_CODE_TTL_SECS, CLIENT_UNUSED_TTL_SECS, MAX_CLIENTS, MAX_CLIENTS_PER_SOURCE,
        MAX_GRANTS, MAX_OAUTH_ENTRY_BYTES, MAX_PENDING_PER_SOURCE, OAuthStore, OAuthStoreError,
        OAuthStoreFile, PENDING_TRANSACTION_TTL_SECS, StoredClient, StoredGrant,
        TEST_MAX_STATE_BYTES, TEST_NOW, sha256_b64,
    };
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
            .tempdir_in("/var/tmp")
            .unwrap()
    }

    fn store_in(journal: &tempfile::TempDir) -> OAuthStore {
        OAuthStore::open(journal.path())
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
        let created = store.generate_pairing_code().unwrap();
        assert_eq!(created.generation, 1);
        assert_eq!(store.pairing_generation().unwrap(), 1);
        assert_eq!(created.code.len(), 8);
    }

    #[test]
    fn pairing_is_single_use() {
        let journal = journal_root();
        let store = store_in(&journal);
        let client = seed_client(&store, "192.0.2.1");
        let pairing = store.generate_pairing_code().unwrap();
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
        let pairing = store.generate_pairing_code().unwrap();
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
        let pairing = store.generate_pairing_code().unwrap();
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
        let first = store.generate_pairing_code().unwrap();
        store.lock_pairing_code().unwrap();
        store.revoke_pairing_code().unwrap();
        assert_eq!(store.pairing_generation().unwrap(), 0);
        let second = store.generate_pairing_code().unwrap();
        assert_eq!(second.generation, first.generation + 2);
        let client = seed_client(&store, "192.0.2.1");
        let transaction = open_transaction(&store, &client, "192.0.2.1");
        store
            .complete_pairing(&transaction, &second.code, &test_binding())
            .expect("fresh code is not locked");
    }

    #[test]
    fn complete_pairing_returns_stored_bindings_only() {
        let journal = journal_root();
        let store = store_in(&journal);
        let client = seed_client(&store, "192.0.2.1");
        let pairing = store.generate_pairing_code().unwrap();
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
        let pairing = store.generate_pairing_code().unwrap();
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
        let pairing = store.generate_pairing_code().unwrap();
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
        let pairing = store.generate_pairing_code().unwrap();
        store
            .complete_pairing(&still_open, &pairing.code, &test_binding())
            .expect("unpaired transaction remains valid for 10 minutes");
        set_now(start.timestamp() + PENDING_TRANSACTION_TTL_SECS + 1);
        let pairing = store.generate_pairing_code().unwrap();
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
        let pairing = store.generate_pairing_code().unwrap();
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
            store.generate_pairing_code(),
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
        assert!(store.generate_pairing_code().is_err());
        assert_eq!(fs::read(&path).unwrap(), b"not json");
    }

    #[test]
    fn incomplete_randomness_writes_nothing() {
        let journal = journal_root();
        let store = store_in(&journal);
        assert!(matches!(
            store.generate_pairing_code_with_random(&ShortRandom),
            Err(OAuthStoreError::Randomness)
        ));
        assert!(!journal.path().join("mcp-endpoint/oauth.json").exists());
    }

    #[test]
    fn refresh_rotation_is_constant_size_and_replays_fail() {
        let journal = journal_root();
        let store = store_in(&journal);
        let client = seed_client(&store, "192.0.2.1");
        let pairing = store.generate_pairing_code().unwrap();
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
        let mut sizes = Vec::new();
        for _ in 0..5 {
            let previous = tokens.refresh_token.clone();
            tokens = store
                .refresh_grant(
                    &tokens.refresh_token,
                    "https://client.example/cimd.json",
                    &test_binding(),
                )
                .unwrap();
            assert!(matches!(
                store.refresh_grant(
                    &previous,
                    "https://client.example/cimd.json",
                    &test_binding()
                ),
                Err(OAuthStoreError::InvalidToken)
            ));
            sizes.push(fs::read(&path).unwrap().len());
        }
        let first = sizes[0];
        assert!(sizes.iter().all(|size| size.abs_diff(first) < 64));
    }

    fn redeem_access(store: &OAuthStore, client_record_id: &str, client_id: &str) -> String {
        let pairing = store.generate_pairing_code().unwrap();
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
        let pairing = store.generate_pairing_code().unwrap();
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
        store.generate_pairing_code().unwrap();
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
        let pairing = store.generate_pairing_code().unwrap();
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
        let unbound_pairing = unbound.store.generate_pairing_code().unwrap();
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

        let bound_pairing = bound.store.generate_pairing_code().unwrap();
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

        let pairing = unbound.store.generate_pairing_code().unwrap();
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

        let _ = unbound.store.generate_pairing_code().unwrap();
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
        let pairing_allowed: HashSet<&str> = ["verifier", "expires_at", "generation", "locked"]
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
            let pairing = store.generate_pairing_code().unwrap();
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
        let pairing = store.generate_pairing_code().unwrap();
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
        let pairing = store.generate_pairing_code().unwrap();
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
        let pairing = unbound.store.generate_pairing_code().unwrap();
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
        let pairing = store.generate_pairing_code().unwrap();
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
            let pairing = store.generate_pairing_code().unwrap();
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
}
