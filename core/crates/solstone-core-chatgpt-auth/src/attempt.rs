// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Sign-in session lifecycle and high-level auth workflows for ChatGPT.

use std::collections::BTreeMap;
use std::fmt;
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use solstone_core_auth_flow::random_token;
use solstone_core_journal_io::{FileLock, LockError};

use crate::authorize::{DYNAMIC_CLIENT_ID, build_authorize_params};
use crate::callback::{
    bind_loopback_listener, parse_pasted_callback_detailed, receive_browser_callback,
    respond_to_callback_stream,
};
use crate::credential::{ClosedOutcome, CredentialError};
use crate::exchange::exchange_code_for_tokens;
use crate::overrides::auth_base_url;
use crate::revoke::revoke_refresh_token;
use crate::store::{
    ChatGptAccountIdentity, ChatGptSignInDoc, LoadResult, Registration, acquire_credential_lock,
    credential_lock_options, credential_path, load_credential_file,
    rotate_unreadable_credential_file, save_credential_file,
};
use crate::transport::ChatGptTransport;

pub const ATTEMPT_EXPIRATION_SECS: u64 = 600; // 10 minutes

pub fn now_unix_secs() -> u64 {
    crate::store::now_secs()
}

#[derive(Clone)]
pub struct SignInAttempt {
    pub attempt_id: String,
    pub verifier: String,
    pub state: String,
    pub nonce: String,
    pub redirect_uri: String,
    pub client_id: String,
    pub authorize_url: String,
    pub begin_epoch: u64,
    pub begin_registration: Option<Registration>,
    pub created_at_unix: u64,
    pub listener: Option<Arc<TcpListener>>,
}

impl fmt::Debug for SignInAttempt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SignInAttempt")
            .field("attempt_id", &self.attempt_id)
            .field("client_id", &self.client_id)
            .field("redirect_uri", &self.redirect_uri)
            .field("created_at_unix", &self.created_at_unix)
            .finish_non_exhaustive()
    }
}

impl SignInAttempt {
    pub fn is_expired(&self) -> bool {
        now_unix_secs().saturating_sub(self.created_at_unix) >= ATTEMPT_EXPIRATION_SECS
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignInResult {
    pub outcome: ClosedOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignOutResult {
    pub revoked: bool,
}

/// Whether the journal holds a usable ChatGPT sign-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignInState {
    SignedOut,
    SignedIn,
}

/// A local, token-free view of the credential file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatGptStatus {
    pub state: SignInState,
    pub signed_in: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    pub plan_usage_declined: bool,
    pub client_refused: bool,
}

impl ChatGptStatus {
    fn from_doc(doc: Option<&ChatGptSignInDoc>) -> Self {
        let tokens = doc.and_then(|doc| doc.tokens.as_ref());
        Self {
            state: if tokens.is_some() {
                SignInState::SignedIn
            } else {
                SignInState::SignedOut
            },
            signed_in: tokens.is_some(),
            email: doc.and_then(|doc| doc.email.clone()),
            expires_at: tokens.map(|tokens| tokens.expires_at),
            plan_usage_declined: doc.is_some_and(|doc| doc.plan_usage_declined),
            client_refused: doc
                .and_then(|doc| doc.registration.as_ref())
                .is_some_and(|registration| registration.client_refused),
        }
    }
}

/// Where a sign-in attempt stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptState {
    Pending,
    SignedIn,
    Failed,
    Expired,
}

/// An attempt's state, with the outcome that ended it once it is no longer pending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptStatus {
    pub state: AttemptState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<ClosedOutcome>,
}

impl AttemptStatus {
    fn ended(outcome: ClosedOutcome) -> Self {
        let state = match outcome {
            ClosedOutcome::SignedIn => AttemptState::SignedIn,
            ClosedOutcome::Expired => AttemptState::Expired,
            _ => AttemptState::Failed,
        };
        Self {
            state,
            reason: Some(outcome),
        }
    }
}

struct ActiveAttemptEntry {
    attempt: SignInAttempt,
    claimed: bool,
}

struct FinishedAttempt {
    created_at_unix: u64,
    outcome: ClosedOutcome,
}

// Lock order: ACTIVE_ATTEMPT before FINISHED_ATTEMPTS.
static ACTIVE_ATTEMPT: Mutex<Option<ActiveAttemptEntry>> = Mutex::new(None);
static FINISHED_ATTEMPTS: Mutex<BTreeMap<String, FinishedAttempt>> = Mutex::new(BTreeMap::new());

fn past_lifetime(created_at_unix: u64) -> bool {
    now_unix_secs().saturating_sub(created_at_unix) >= ATTEMPT_EXPIRATION_SECS
}

/// Record how an attempt ended. The first outcome recorded for an attempt stands.
fn record_finished(attempt: &SignInAttempt, outcome: ClosedOutcome) {
    if let Ok(mut finished) = FINISHED_ATTEMPTS.lock() {
        finished.retain(|_, record| !past_lifetime(record.created_at_unix));
        finished
            .entry(attempt.attempt_id.clone())
            .or_insert(FinishedAttempt {
                created_at_unix: attempt.created_at_unix,
                outcome,
            });
    }
}

fn finished_outcome(attempt_id: &str) -> Option<(u64, ClosedOutcome)> {
    FINISHED_ATTEMPTS.lock().ok().and_then(|finished| {
        finished
            .get(attempt_id)
            .map(|record| (record.created_at_unix, record.outcome))
    })
}

fn is_cancelled(attempt_id: &str) -> bool {
    matches!(
        finished_outcome(attempt_id),
        Some((_, ClosedOutcome::Cancelled))
    )
}

/// Record the outcome of an attempt this process is tracking as its active attempt. A replaced
/// attempt already reads `cancelled`, and an attempt begun by another process is not tracked.
fn record_outcome(attempt: &SignInAttempt, outcome: ClosedOutcome) {
    if let Ok(guard) = ACTIVE_ATTEMPT.lock()
        && guard
            .as_ref()
            .is_some_and(|entry| entry.attempt.attempt_id == attempt.attempt_id)
    {
        record_finished(attempt, outcome);
    }
}

enum Claim {
    Claimed,
    Cancelled,
    AlreadyClaimed,
}

/// Take the single use of an attempt for one callback.
fn claim(attempt: &SignInAttempt) -> Claim {
    let Ok(mut guard) = ACTIVE_ATTEMPT.lock() else {
        return Claim::Claimed;
    };
    if is_cancelled(&attempt.attempt_id) {
        return Claim::Cancelled;
    }
    if let Some(entry) = guard.as_mut()
        && entry.attempt.attempt_id == attempt.attempt_id
    {
        if entry.claimed {
            return Claim::AlreadyClaimed;
        }
        entry.claimed = true;
    }
    Claim::Claimed
}

/// Make `attempt` the one pending attempt; a previous one reads `cancelled` from now on.
pub fn set_active_attempt(attempt: SignInAttempt) {
    if let Ok(mut guard) = ACTIVE_ATTEMPT.lock() {
        if let Some(previous) = guard.take() {
            record_finished(&previous.attempt, ClosedOutcome::Cancelled);
        }
        *guard = Some(ActiveAttemptEntry {
            attempt,
            claimed: false,
        });
    }
}

pub fn get_active_attempt() -> Option<SignInAttempt> {
    if let Ok(guard) = ACTIVE_ATTEMPT.lock() {
        guard.as_ref().map(|e| e.attempt.clone())
    } else {
        None
    }
}

/// The active attempt with this id, or the outcome refusing it: `cancelled` for a replaced
/// attempt, `expired` for anything else.
pub fn active_attempt_named(attempt_id: &str) -> Result<SignInAttempt, ClosedOutcome> {
    if let Ok(guard) = ACTIVE_ATTEMPT.lock()
        && let Some(entry) = guard.as_ref()
        && entry.attempt.attempt_id == attempt_id
    {
        if entry.attempt.is_expired() {
            return Err(ClosedOutcome::Expired);
        }
        return Ok(entry.attempt.clone());
    }
    match finished_outcome(attempt_id) {
        Some((created_at_unix, ClosedOutcome::Cancelled)) if !past_lifetime(created_at_unix) => {
            Err(ClosedOutcome::Cancelled)
        }
        _ => Err(ClosedOutcome::Expired),
    }
}

/// Where an attempt stands. An unknown id, including one from before a restart, and any attempt
/// past its lifetime read `expired`.
pub fn attempt_status(attempt_id: &str) -> AttemptStatus {
    let active_created_at = ACTIVE_ATTEMPT.lock().ok().and_then(|guard| {
        guard
            .as_ref()
            .filter(|entry| entry.attempt.attempt_id == attempt_id)
            .map(|entry| entry.attempt.created_at_unix)
    });
    match (finished_outcome(attempt_id), active_created_at) {
        (Some((created_at_unix, _)), _) | (None, Some(created_at_unix))
            if past_lifetime(created_at_unix) =>
        {
            AttemptStatus::ended(ClosedOutcome::Expired)
        }
        (Some((_, outcome)), _) => AttemptStatus::ended(outcome),
        (None, Some(_)) => AttemptStatus {
            state: AttemptState::Pending,
            reason: None,
        },
        (None, None) => AttemptStatus::ended(ClosedOutcome::Expired),
    }
}

/// A fresh `urn:uuid:` host id from a random version 4 UUID.
pub fn generate_host_id() -> Result<String, CredentialError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| CredentialError::Io(e.to_string()))?;
    Ok(host_id_from_bytes(bytes))
}

fn host_id_from_bytes(mut bytes: [u8; 16]) -> String {
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "urn:uuid:{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

pub fn begin_sign_in(journal: &Path) -> Result<SignInAttempt, CredentialError> {
    let auth_base = auth_base_url();

    let _lock =
        acquire_credential_lock(journal, credential_lock_options()).map_err(|e| match e {
            LockError::Timeout(_) => CredentialError::Busy,
            other => CredentialError::Io(other.to_string()),
        })?;

    let doc = match load_credential_file(journal) {
        LoadResult::Present(d) => d,
        LoadResult::Absent => {
            let host_id = generate_host_id()?;
            let initial = ChatGptSignInDoc::new_initial(host_id);
            if let Err(e) = save_credential_file(journal, &initial) {
                return Err(CredentialError::Io(e.to_string()));
            }
            initial
        }
        LoadResult::Unreadable => {
            return Err(CredentialError::Storage(credential_path(journal)));
        }
    };

    let (listener, redirect_uri) = bind_loopback_listener()
        .map_err(|e| CredentialError::Io(format!("failed to bind loopback listener: {e}")))?;

    let params = build_authorize_params(
        &auth_base,
        &redirect_uri,
        &doc.host_id,
        doc.registration.as_ref(),
        doc.email.as_deref(),
        doc.plan_usage_declined,
    )
    .map_err(|e| CredentialError::Io(e.to_string()))?;

    let attempt_id = random_token(16).map_err(|e| CredentialError::Io(e.to_string()))?;
    let attempt = SignInAttempt {
        attempt_id,
        verifier: params.verifier,
        state: params.state,
        nonce: params.nonce,
        redirect_uri: params.redirect_uri,
        client_id: params.client_id,
        authorize_url: params.authorize_url,
        begin_epoch: doc.epoch,
        begin_registration: doc.registration.clone(),
        created_at_unix: now_unix_secs(),
        listener: Some(Arc::new(listener)),
    };

    set_active_attempt(attempt.clone());

    Ok(attempt)
}

pub fn finish_sign_in(
    journal: &Path,
    transport: &dyn ChatGptTransport,
    attempt: &SignInAttempt,
    callback_input: Option<&str>,
    timeout: Duration,
) -> Result<SignInResult, ClosedOutcome> {
    if attempt.is_expired() {
        return Err(ClosedOutcome::Expired);
    }

    let expected_client_id =
        (attempt.client_id != DYNAMIC_CLIENT_ID).then_some(attempt.client_id.as_str());

    // 1. Receive callback or parse pasted input
    let (code, callback_client_id, mut response_stream) = match callback_input {
        Some(input) => {
            match parse_pasted_callback_detailed(input, &attempt.state, expected_client_id) {
                Ok((c, cid)) => (c, cid, None),
                // A wrong or missing state, or an address that is not the callback, leaves the
                // attempt pending.
                Err(Err(_)) => return Err(ClosedOutcome::CallbackInvalid),
                Err(Ok(outcome)) => return end_before_exchange(attempt, outcome),
            }
        }
        None => {
            let Some(listener) = &attempt.listener else {
                return Err(ClosedOutcome::CallbackInvalid);
            };
            match receive_browser_callback(listener, &attempt.state, expected_client_id, timeout) {
                Ok(req) => (req.code, req.client_id, req.stream),
                // The listener's wait ending is not the attempt ending; its lifetime decides.
                Err(ClosedOutcome::Expired) => return Err(ClosedOutcome::Expired),
                Err(outcome) => return end_before_exchange(attempt, outcome),
            }
        }
    };

    match claim(attempt) {
        Claim::Claimed => {}
        Claim::Cancelled => {
            respond_to_callback_stream(response_stream.as_mut(), ClosedOutcome::Cancelled);
            return Err(ClosedOutcome::Cancelled);
        }
        Claim::AlreadyClaimed => {
            respond_to_callback_stream(response_stream.as_mut(), ClosedOutcome::CallbackInvalid);
            return Err(ClosedOutcome::CallbackInvalid);
        }
    }

    let result = exchange_and_save(
        journal,
        transport,
        attempt,
        code,
        callback_client_id,
        response_stream,
    );
    record_outcome(
        attempt,
        match &result {
            Ok(res) => res.outcome,
            Err(outcome) => *outcome,
        },
    );
    result
}

/// End an attempt whose callback was refused before any exchange, unless another callback
/// already holds it.
fn end_before_exchange(
    attempt: &SignInAttempt,
    outcome: ClosedOutcome,
) -> Result<SignInResult, ClosedOutcome> {
    if matches!(claim(attempt), Claim::Claimed) {
        record_outcome(attempt, outcome);
    }
    Err(outcome)
}

/// Step 3: lock and re-read. The registration must still hold this client id and the epoch
/// must match the begin snapshot; otherwise another sign-in or a forget won and this result is
/// discarded. On refusal the lock is already released.
fn lock_for_result(
    journal: &Path,
    attempt: &SignInAttempt,
    client_id: &str,
) -> Result<(FileLock, ChatGptSignInDoc), ClosedOutcome> {
    let lock = acquire_credential_lock(journal, credential_lock_options())
        .map_err(|_| ClosedOutcome::Busy)?;
    if is_cancelled(&attempt.attempt_id) {
        return Err(ClosedOutcome::Cancelled);
    }
    let LoadResult::Present(doc) = load_credential_file(journal) else {
        return Err(ClosedOutcome::Superseded);
    };
    let registration_matches = doc
        .registration
        .as_ref()
        .is_some_and(|registration| registration.client_id == client_id);
    if doc.epoch != attempt.begin_epoch || !registration_matches {
        return Err(ClosedOutcome::Superseded);
    }
    Ok((lock, doc))
}

fn exchange_and_save(
    journal: &Path,
    transport: &dyn ChatGptTransport,
    attempt: &SignInAttempt,
    code: String,
    callback_client_id: Option<String>,
    mut response_stream: Option<std::net::TcpStream>,
) -> Result<SignInResult, ClosedOutcome> {
    let auth_base = auth_base_url();

    // Step 1: Lock. Epoch and registration must still match the begin snapshot.
    let lock_res = acquire_credential_lock(journal, credential_lock_options());
    let _lock = match lock_res {
        Ok(l) => l,
        Err(LockError::Timeout(_)) => {
            respond_to_callback_stream(response_stream.as_mut(), ClosedOutcome::Busy);
            return Err(ClosedOutcome::Busy);
        }
        Err(_) => {
            respond_to_callback_stream(response_stream.as_mut(), ClosedOutcome::Storage);
            return Err(ClosedOutcome::Storage);
        }
    };

    let mut doc = match load_credential_file(journal) {
        LoadResult::Present(d) => d,
        _ => {
            respond_to_callback_stream(response_stream.as_mut(), ClosedOutcome::Superseded);
            return Err(ClosedOutcome::Superseded);
        }
    };

    if doc.epoch != attempt.begin_epoch || doc.registration != attempt.begin_registration {
        respond_to_callback_stream(response_stream.as_mut(), ClosedOutcome::Superseded);
        return Err(ClosedOutcome::Superseded);
    }

    let is_first_reg = attempt.client_id == DYNAMIC_CLIENT_ID;
    let effective_client_id = if is_first_reg {
        let Some(issued_cid) = callback_client_id else {
            respond_to_callback_stream(response_stream.as_mut(), ClosedOutcome::CallbackInvalid);
            return Err(ClosedOutcome::CallbackInvalid);
        };
        if issued_cid.is_empty() || issued_cid == DYNAMIC_CLIENT_ID {
            respond_to_callback_stream(response_stream.as_mut(), ClosedOutcome::CallbackInvalid);
            return Err(ClosedOutcome::CallbackInvalid);
        }
        doc.registration = Some(Registration {
            client_id: issued_cid.clone(),
            client_refused: false,
        });
        // A new registration starts with no account bound to it.
        doc.subject = None;
        doc.email = None;
        if save_credential_file(journal, &doc).is_err() {
            respond_to_callback_stream(response_stream.as_mut(), ClosedOutcome::Storage);
            return Err(ClosedOutcome::Storage);
        }
        issued_cid
    } else {
        attempt.client_id.clone()
    };

    let saved_subject = doc.subject.clone();
    drop(_lock); // Release lock before exchange

    if is_cancelled(&attempt.attempt_id) {
        respond_to_callback_stream(response_stream.as_mut(), ClosedOutcome::Cancelled);
        return Err(ClosedOutcome::Cancelled);
    }

    // Step 2: Exchange outside the lock.
    let exchange_res = exchange_code_for_tokens(
        transport,
        &auth_base,
        &code,
        &attempt.redirect_uri,
        &effective_client_id,
        &attempt.verifier,
        &attempt.nonce,
        saved_subject.as_deref(),
        Duration::from_secs(30),
    );

    let exchange = match exchange_res {
        Ok(ex) => ex,
        Err(ClosedOutcome::RegistrationRefused) => {
            // The refusal is written only past the same re-read check as any other result.
            let outcome = match lock_for_result(journal, attempt, &effective_client_id) {
                Ok((lock, mut doc)) => {
                    if let Some(registration) = &mut doc.registration {
                        registration.client_refused = true;
                    }
                    let saved = save_credential_file(journal, &doc).is_ok();
                    drop(lock);
                    if saved {
                        ClosedOutcome::RegistrationRefused
                    } else {
                        ClosedOutcome::Storage
                    }
                }
                Err(outcome) => outcome,
            };
            respond_to_callback_stream(response_stream.as_mut(), outcome);
            return Err(outcome);
        }
        Err(err) => {
            respond_to_callback_stream(response_stream.as_mut(), err);
            return Err(err);
        }
    };

    // Step 3: every write from this result happens only past the re-read check. A discarded
    // result's refresh token is revoked after the lock is released.
    let revoke = || {
        let _ = revoke_refresh_token(
            transport,
            &auth_base,
            &exchange.refresh_token,
            &effective_client_id,
        );
    };
    let (lock, mut doc) = match lock_for_result(journal, attempt, &effective_client_id) {
        Ok(held) => held,
        Err(outcome) => {
            revoke();
            respond_to_callback_stream(response_stream.as_mut(), outcome);
            return Err(outcome);
        }
    };

    if !exchange.plan_usage_granted {
        doc.subject = exchange.subject.clone();
        doc.email = exchange.email.clone();
        doc.plan_usage_declined = true;
        doc.tokens = None;
        let saved = save_credential_file(journal, &doc).is_ok();
        drop(lock);
        revoke();
        let outcome = if saved {
            ClosedOutcome::PlanUsageNotGranted
        } else {
            ClosedOutcome::Storage
        };
        respond_to_callback_stream(response_stream.as_mut(), outcome);
        return Err(outcome);
    }

    doc.tokens = exchange.tokens.clone();
    doc.subject = exchange.subject.clone();
    doc.email = exchange.email.clone();
    doc.plan_usage_declined = false;
    let Ok(sign_in_id) = random_token(16) else {
        drop(lock);
        revoke();
        respond_to_callback_stream(response_stream.as_mut(), ClosedOutcome::Storage);
        return Err(ClosedOutcome::Storage);
    };
    doc.sign_in_id = Some(sign_in_id);

    if save_credential_file(journal, &doc).is_err() {
        drop(lock);
        revoke();
        respond_to_callback_stream(response_stream.as_mut(), ClosedOutcome::Storage);
        return Err(ClosedOutcome::Storage);
    }

    drop(lock);

    respond_to_callback_stream(response_stream.as_mut(), ClosedOutcome::SignedIn);
    Ok(SignInResult {
        outcome: ClosedOutcome::SignedIn,
    })
}

pub fn finish_active_sign_in(
    journal: &Path,
    transport: &dyn ChatGptTransport,
    callback_input: Option<&str>,
    timeout: Duration,
) -> Result<SignInResult, ClosedOutcome> {
    let Some(attempt) = get_active_attempt() else {
        return Err(ClosedOutcome::Expired);
    };
    if attempt.is_expired() {
        return Err(ClosedOutcome::Expired);
    }
    finish_sign_in(journal, transport, &attempt, callback_input, timeout)
}

pub fn sign_out(
    journal: &Path,
    transport: &dyn ChatGptTransport,
    forget: bool,
) -> Result<SignOutResult, CredentialError> {
    let auth_base = auth_base_url();

    let _lock =
        acquire_credential_lock(journal, credential_lock_options()).map_err(|e| match e {
            LockError::Timeout(_) => CredentialError::Busy,
            other => CredentialError::Io(other.to_string()),
        })?;

    let load_res = load_credential_file(journal);
    let (mut doc, to_revoke) = match load_res {
        LoadResult::Absent => {
            return Ok(SignOutResult { revoked: true });
        }
        LoadResult::Unreadable => {
            if forget {
                let _ = rotate_unreadable_credential_file(journal);
                let host_id = generate_host_id()?;
                let fresh = ChatGptSignInDoc::new_initial(host_id);
                if let Err(e) = save_credential_file(journal, &fresh) {
                    return Err(CredentialError::Io(e.to_string()));
                }
                return Ok(SignOutResult { revoked: false });
            } else {
                return Err(CredentialError::Storage(credential_path(journal)));
            }
        }
        LoadResult::Present(d) => {
            let revoke_info = d.tokens.as_ref().and_then(|t| {
                let cid = d.registration.as_ref().map(|r| r.client_id.clone())?;
                Some((t.refresh_token.clone(), cid))
            });
            (d, revoke_info)
        }
    };

    if forget {
        doc.registration = None;
        doc.tokens = None;
        doc.subject = None;
        doc.email = None;
        doc.plan_usage_declined = false;
        doc.sign_in_id = None;
        doc.epoch = doc.epoch.saturating_add(1);
    } else {
        doc.tokens = None;
        doc.sign_in_id = None;
    }

    if let Err(e) = save_credential_file(journal, &doc) {
        return Err(CredentialError::Io(e.to_string()));
    }

    drop(_lock); // Release lock before revoking

    if let Some((refresh_tok, client_id)) = to_revoke {
        let rev_ok = revoke_refresh_token(transport, &auth_base, &refresh_tok, &client_id);
        Ok(SignOutResult { revoked: rev_ok })
    } else {
        // Nothing was held, so nothing is left to revoke.
        Ok(SignOutResult { revoked: true })
    }
}

pub fn get_status(journal: &Path) -> Result<ChatGptStatus, CredentialError> {
    match load_credential_file(journal) {
        LoadResult::Present(doc) => Ok(ChatGptStatus::from_doc(Some(&doc))),
        LoadResult::Absent => Ok(ChatGptStatus::from_doc(None)),
        LoadResult::Unreadable => Err(CredentialError::Storage(credential_path(journal))),
    }
}

pub fn registration(journal: &Path) -> Result<Option<Registration>, CredentialError> {
    match load_credential_file(journal) {
        LoadResult::Present(doc) => Ok(doc.registration),
        LoadResult::Absent => Ok(None),
        LoadResult::Unreadable => Err(CredentialError::Storage(credential_path(journal))),
    }
}

pub fn account_identity(journal: &Path) -> Result<ChatGptAccountIdentity, CredentialError> {
    match load_credential_file(journal) {
        LoadResult::Present(doc) => {
            let client_id = doc.registration.as_ref().map(|r| r.client_id.clone());
            let sign_in_id = doc.sign_in_id;
            let subject = doc.subject;
            Ok(ChatGptAccountIdentity {
                client_id,
                sign_in_id,
                subject,
            })
        }
        LoadResult::Absent => Ok(ChatGptAccountIdentity {
            client_id: None,
            sign_in_id: None,
            subject: None,
        }),
        LoadResult::Unreadable => Err(CredentialError::Storage(credential_path(journal))),
    }
}
