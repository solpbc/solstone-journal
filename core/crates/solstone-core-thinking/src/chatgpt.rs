// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Thinking facade for ChatGPT authentication, models, and action logs.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use solstone_core_facets::append_action_log;

pub use solstone_core_chatgpt_auth::*;

pub fn begin_sign_in(journal: &Path) -> Result<SignInAttempt, CredentialError> {
    let attempt = solstone_core_chatgpt_auth::begin_sign_in(journal)?;
    append_action_log(
        journal,
        None,
        "app",
        "thinking",
        "chatgpt_sign_in_begin",
        json!({}),
    )
    .map_err(|error| CredentialError::Io(format!("action log failed: {error}")))?;
    Ok(attempt)
}

pub fn finish_sign_in(
    journal: &Path,
    transport: &dyn ChatGptTransport,
    attempt: &SignInAttempt,
    callback_input: Option<&str>,
    timeout: Duration,
) -> Result<SignInResult, ClosedOutcome> {
    let result = solstone_core_chatgpt_auth::finish_sign_in(
        journal,
        transport,
        attempt,
        callback_input,
        timeout,
    );

    let outcome = match &result {
        Ok(res) => res.outcome,
        Err(err) => *err,
    };

    if outcome != ClosedOutcome::Cancelled && outcome != ClosedOutcome::Expired {
        let log_res = append_action_log(
            journal,
            None,
            "app",
            "thinking",
            "chatgpt_sign_in_finish",
            json!({ "outcome": outcome.as_str() }),
        );
        if log_res.is_err() && result.is_ok() {
            return Err(ClosedOutcome::Storage);
        }
    }

    result
}

pub fn finish_active_sign_in(
    journal: &Path,
    transport: &dyn ChatGptTransport,
    callback_input: Option<&str>,
    timeout: Duration,
) -> Result<SignInResult, ClosedOutcome> {
    let attempt = solstone_core_chatgpt_auth::get_active_attempt().ok_or(ClosedOutcome::Expired)?;
    finish_sign_in(journal, transport, &attempt, callback_input, timeout)
}

/// Finish the attempt named `attempt_id`, refusing one that is no longer the active attempt.
pub fn finish_named_sign_in(
    journal: &Path,
    transport: &dyn ChatGptTransport,
    attempt_id: &str,
    callback_input: Option<&str>,
    timeout: Duration,
) -> Result<SignInResult, ClosedOutcome> {
    let attempt = solstone_core_chatgpt_auth::active_attempt_named(attempt_id)?;
    finish_sign_in(journal, transport, &attempt, callback_input, timeout)
}

pub fn sign_out(
    journal: &Path,
    transport: &dyn ChatGptTransport,
    forget: bool,
) -> Result<SignOutResult, CredentialError> {
    let res = solstone_core_chatgpt_auth::sign_out(journal, transport, forget)?;
    append_action_log(
        journal,
        None,
        "app",
        "thinking",
        "chatgpt_sign_out",
        json!({ "forget": forget, "revoked": res.revoked }),
    )
    .map_err(|error| CredentialError::Io(format!("action log failed: {error}")))?;
    Ok(res)
}

pub fn open_browser_for_url(url: &str) -> bool {
    if let Ok(inv) =
        solstone_core_auth_flow::plan_browser(solstone_core_auth_flow::current_target_os(), url)
    {
        solstone_core_auth_flow::execute_browser_invocation(&inv).is_ok()
    } else {
        false
    }
}

pub fn get_status(journal: &Path) -> Result<ChatGptStatus, CredentialError> {
    solstone_core_chatgpt_auth::get_status(journal)
}

pub fn registration(journal: &Path) -> Result<Option<Registration>, CredentialError> {
    solstone_core_chatgpt_auth::registration(journal)
}

pub fn list_models(
    journal: &Path,
    transport: Arc<dyn ChatGptTransport>,
) -> Result<Vec<ChatGptModel>, CredentialError> {
    let manager = ChatGptAuthManager::new(journal.to_path_buf(), transport.clone());
    models::fetch_models(&manager, transport.as_ref(), models::MODELS_TIMEOUT)
}

pub trait ChatGptModelSource {
    fn status(&self, journal: &Path) -> Result<ChatGptStatus, CredentialError>;
    fn list(&self, journal: &Path) -> Result<Vec<ChatGptModel>, CredentialError>;
}

pub struct LiveChatGptModels;

impl ChatGptModelSource for LiveChatGptModels {
    fn status(&self, journal: &Path) -> Result<ChatGptStatus, CredentialError> {
        get_status(journal)
    }

    fn list(&self, journal: &Path) -> Result<Vec<ChatGptModel>, CredentialError> {
        let transport = Arc::new(solstone_core_chatgpt_auth::UreqTransport);
        list_models(journal, transport)
    }
}
