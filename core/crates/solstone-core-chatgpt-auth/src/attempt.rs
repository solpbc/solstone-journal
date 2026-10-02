// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Sign-in session lifecycle and high-level auth workflows for ChatGPT.

use std::fmt;
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use solstone_core_auth_flow::random_token;
use solstone_core_journal_io::LockError;

use crate::authorize::build_authorize_params;
use crate::callback::{
    bind_loopback_listener, parse_pasted_callback, receive_browser_callback,
    respond_to_callback_stream,
};
use crate::credential::{ClosedOutcome, CredentialError};
use crate::exchange::exchange_code_for_tokens;
use crate::overrides::auth_base_url;
use crate::revoke::revoke_refresh_token;
use crate::store::{
    ChatGptSignInDoc, LoadResult, Registration, acquire_credential_lock, credential_lock_options,
    credential_path, load_credential_file, rotate_unreadable_credential_file, save_credential_file,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatGptStatus {
    pub signed_in: bool,
    pub plan_usage_declined: bool,
}

struct ActiveAttemptEntry {
    attempt: SignInAttempt,
    cancelled: bool,
    claimed: bool,
}

static ACTIVE_ATTEMPT: Mutex<Option<ActiveAttemptEntry>> = Mutex::new(None);
static CANCELLED_ATTEMPTS: Mutex<std::collections::BTreeSet<String>> =
    Mutex::new(std::collections::BTreeSet::new());

pub fn set_active_attempt(attempt: SignInAttempt) {
    if let Ok(mut guard) = ACTIVE_ATTEMPT.lock() {
        if let Some(entry) = guard.as_mut() {
            entry.cancelled = true;
            if let Ok(mut cancelled_set) = CANCELLED_ATTEMPTS.lock() {
                cancelled_set.insert(entry.attempt.attempt_id.clone());
            }
        }
        *guard = Some(ActiveAttemptEntry {
            attempt,
            cancelled: false,
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

pub fn get_active_attempt_status(attempt_id: &str) -> ClosedOutcome {
    if let Ok(cancelled_set) = CANCELLED_ATTEMPTS.lock()
        && cancelled_set.contains(attempt_id)
    {
        return ClosedOutcome::Cancelled;
    }

    if let Ok(guard) = ACTIVE_ATTEMPT.lock()
        && let Some(entry) = guard.as_ref()
        && entry.attempt.attempt_id == attempt_id
    {
        if entry.cancelled {
            return ClosedOutcome::Cancelled;
        }
        if entry.attempt.is_expired() {
            return ClosedOutcome::Expired;
        }
        return ClosedOutcome::Busy;
    }
    ClosedOutcome::Expired
}

pub fn generate_host_id() -> Result<String, CredentialError> {
    let rand = random_token(12).map_err(|e| CredentialError::Io(e.to_string()))?;
    Ok(format!("solstone-{rand}"))
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

    let auth_base = auth_base_url();

    // 1. Receive callback or parse pasted input
    let (code, callback_client_id, mut response_stream) = match callback_input {
        Some(input) => {
            let (c, cid) = parse_pasted_callback(
                input,
                &attempt.state,
                (attempt.client_id != "dynamic_agent_client").then_some(attempt.client_id.as_str()),
            )?;
            (c, cid, None)
        }
        None => {
            let Some(listener) = &attempt.listener else {
                return Err(ClosedOutcome::CallbackInvalid);
            };
            let req = receive_browser_callback(
                listener,
                &attempt.state,
                (attempt.client_id != "dynamic_agent_client").then_some(attempt.client_id.as_str()),
                timeout,
            )?;
            (req.code, req.client_id, req.stream)
        }
    };

    if let Ok(mut guard) = ACTIVE_ATTEMPT.lock()
        && let Some(entry) = guard.as_mut()
        && entry.attempt.attempt_id == attempt.attempt_id
    {
        if entry.cancelled {
            respond_to_callback_stream(response_stream.as_mut(), ClosedOutcome::Cancelled);
            return Err(ClosedOutcome::Cancelled);
        }
        if entry.claimed {
            respond_to_callback_stream(response_stream.as_mut(), ClosedOutcome::CallbackInvalid);
            return Err(ClosedOutcome::CallbackInvalid);
        }
        entry.claimed = true;
    }
    if let Ok(cancelled_set) = CANCELLED_ATTEMPTS.lock()
        && cancelled_set.contains(&attempt.attempt_id)
    {
        respond_to_callback_stream(response_stream.as_mut(), ClosedOutcome::Cancelled);
        return Err(ClosedOutcome::Cancelled);
    }

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

    let is_first_reg = attempt.client_id == "dynamic_agent_client";
    let effective_client_id = if is_first_reg {
        let Some(issued_cid) = callback_client_id else {
            respond_to_callback_stream(response_stream.as_mut(), ClosedOutcome::CallbackInvalid);
            return Err(ClosedOutcome::CallbackInvalid);
        };
        if issued_cid.is_empty() || issued_cid == "dynamic_agent_client" {
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

    // Check cancellation before exchange
    let is_cancelled_before_exchange = {
        if let Ok(cancelled_set) = CANCELLED_ATTEMPTS.lock()
            && cancelled_set.contains(&attempt.attempt_id)
        {
            true
        } else if let Ok(guard) = ACTIVE_ATTEMPT.lock()
            && let Some(entry) = guard.as_ref()
            && entry.attempt.attempt_id == attempt.attempt_id
            && entry.cancelled
        {
            true
        } else {
            false
        }
    };
    if is_cancelled_before_exchange {
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
            // Update client_refused under lock
            if let Ok(_l) = acquire_credential_lock(journal, credential_lock_options())
                && let LoadResult::Present(mut d) = load_credential_file(journal)
                && let Some(reg) = &mut d.registration
            {
                reg.client_refused = true;
                if save_credential_file(journal, &d).is_err() {
                    respond_to_callback_stream(response_stream.as_mut(), ClosedOutcome::Storage);
                    return Err(ClosedOutcome::Storage);
                }
            }
            respond_to_callback_stream(
                response_stream.as_mut(),
                ClosedOutcome::RegistrationRefused,
            );
            return Err(ClosedOutcome::RegistrationRefused);
        }
        Err(err) => {
            respond_to_callback_stream(response_stream.as_mut(), err);
            return Err(err);
        }
    };

    if !exchange.plan_usage_granted {
        // Under lock: save subject, email, plan_usage_declined = true, tokens = None
        if let Ok(_l) = acquire_credential_lock(journal, credential_lock_options())
            && let LoadResult::Present(mut d) = load_credential_file(journal)
            && d.epoch == attempt.begin_epoch
        {
            d.subject = exchange.subject;
            d.email = exchange.email;
            d.plan_usage_declined = true;
            d.tokens = None;
            if save_credential_file(journal, &d).is_err() {
                drop(_l);
                let _ = revoke_refresh_token(
                    transport,
                    &auth_base,
                    &exchange.refresh_token,
                    &effective_client_id,
                );
                respond_to_callback_stream(response_stream.as_mut(), ClosedOutcome::Storage);
                return Err(ClosedOutcome::Storage);
            }
        }
        let _ = revoke_refresh_token(
            transport,
            &auth_base,
            &exchange.refresh_token,
            &effective_client_id,
        );
        respond_to_callback_stream(response_stream.as_mut(), ClosedOutcome::PlanUsageNotGranted);
        return Err(ClosedOutcome::PlanUsageNotGranted);
    }

    // Step 3: Lock, re-read. Registration must still be that client id and epoch must match.
    // A discarded result's refresh token is revoked after the lock is released.
    let lock_res = acquire_credential_lock(journal, credential_lock_options());
    let _lock = match lock_res {
        Ok(l) => l,
        Err(_) => {
            let _ = revoke_refresh_token(
                transport,
                &auth_base,
                &exchange.refresh_token,
                &effective_client_id,
            );
            respond_to_callback_stream(response_stream.as_mut(), ClosedOutcome::Busy);
            return Err(ClosedOutcome::Busy);
        }
    };

    let is_cancelled_before_save = {
        if let Ok(cancelled_set) = CANCELLED_ATTEMPTS.lock()
            && cancelled_set.contains(&attempt.attempt_id)
        {
            true
        } else if let Ok(guard) = ACTIVE_ATTEMPT.lock()
            && let Some(entry) = guard.as_ref()
            && entry.attempt.attempt_id == attempt.attempt_id
            && entry.cancelled
        {
            true
        } else {
            false
        }
    };
    if is_cancelled_before_save {
        drop(_lock);
        let _ = revoke_refresh_token(
            transport,
            &auth_base,
            &exchange.refresh_token,
            &effective_client_id,
        );
        respond_to_callback_stream(response_stream.as_mut(), ClosedOutcome::Cancelled);
        return Err(ClosedOutcome::Cancelled);
    }

    let mut doc = match load_credential_file(journal) {
        LoadResult::Present(d) => d,
        _ => {
            drop(_lock);
            let _ = revoke_refresh_token(
                transport,
                &auth_base,
                &exchange.refresh_token,
                &effective_client_id,
            );
            respond_to_callback_stream(response_stream.as_mut(), ClosedOutcome::Superseded);
            return Err(ClosedOutcome::Superseded);
        }
    };

    let reg_matches = doc
        .registration
        .as_ref()
        .map(|r| r.client_id == effective_client_id)
        .unwrap_or(false);

    if doc.epoch != attempt.begin_epoch || !reg_matches {
        drop(_lock);
        let _ = revoke_refresh_token(
            transport,
            &auth_base,
            &exchange.refresh_token,
            &effective_client_id,
        );
        respond_to_callback_stream(response_stream.as_mut(), ClosedOutcome::Superseded);
        return Err(ClosedOutcome::Superseded);
    }

    doc.tokens = exchange.tokens;
    doc.subject = exchange.subject;
    doc.email = exchange.email;
    doc.plan_usage_declined = false;
    let Ok(sign_in_id) = random_token(16) else {
        drop(_lock);
        let _ = revoke_refresh_token(
            transport,
            &auth_base,
            &exchange.refresh_token,
            &effective_client_id,
        );
        respond_to_callback_stream(response_stream.as_mut(), ClosedOutcome::Storage);
        return Err(ClosedOutcome::Storage);
    };
    doc.sign_in_id = Some(sign_in_id);

    if save_credential_file(journal, &doc).is_err() {
        drop(_lock);
        let _ = revoke_refresh_token(
            transport,
            &auth_base,
            &exchange.refresh_token,
            &effective_client_id,
        );
        respond_to_callback_stream(response_stream.as_mut(), ClosedOutcome::Storage);
        return Err(ClosedOutcome::Storage);
    }

    drop(_lock);

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
    let attempt = if let Ok(guard) = ACTIVE_ATTEMPT.lock() {
        if let Some(entry) = guard.as_ref() {
            if entry.cancelled {
                return Err(ClosedOutcome::Cancelled);
            }
            if entry.attempt.is_expired() {
                return Err(ClosedOutcome::Expired);
            }
            entry.attempt.clone()
        } else {
            return Err(ClosedOutcome::Expired);
        }
    } else {
        return Err(ClosedOutcome::Busy);
    };

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
        LoadResult::Present(doc) => Ok(ChatGptStatus {
            signed_in: doc.tokens.is_some(),
            plan_usage_declined: doc.plan_usage_declined,
        }),
        LoadResult::Absent => Ok(ChatGptStatus {
            signed_in: false,
            plan_usage_declined: false,
        }),
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
