// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Credential trait and error types for ChatGPT token consumers.

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Closed outcomes for ChatGPT sign-in attempts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClosedOutcome {
    SignedIn,
    Denied,
    CallbackInvalid,
    PlanUsageNotGranted,
    AccountMismatch,
    RegistrationRefused,
    ExchangeFailed,
    Superseded,
    Busy,
    Storage,
    Cancelled,
    Expired,
}

impl ClosedOutcome {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::SignedIn => "signed_in",
            Self::Denied => "denied",
            Self::CallbackInvalid => "callback_invalid",
            Self::PlanUsageNotGranted => "plan_usage_not_granted",
            Self::AccountMismatch => "account_mismatch",
            Self::RegistrationRefused => "registration_refused",
            Self::ExchangeFailed => "exchange_failed",
            Self::Superseded => "superseded",
            Self::Busy => "busy",
            Self::Storage => "storage",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
        }
    }
}

impl fmt::Display for ClosedOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Credential errors for ChatGPT operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialError {
    SignInRequired,
    Busy,
    Storage(PathBuf),
    Io(String),
    GrantNotSaved,
    Network,
    Unavailable,
    Refused(Option<String>),
    Malformed(Option<String>),
    NotEligible,
}

impl fmt::Display for CredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SignInRequired => formatter
                .write_str("ChatGPT sign-in required: run 'journal thinking chatgpt sign-in'"),
            Self::Busy => formatter.write_str("ChatGPT credential store is busy"),
            Self::Storage(path) => write!(
                formatter,
                "{}: credential storage is unreadable or corrupted; run 'journal thinking chatgpt sign-out --forget'",
                path.display()
            ),
            Self::Io(msg) => write!(formatter, "I/O error during credential operation: {msg}"),
            Self::GrantNotSaved => formatter.write_str(
                "ChatGPT token grant was received from the server but could not be saved to disk",
            ),
            Self::Network => {
                formatter.write_str("network error communicating with ChatGPT auth servers")
            }
            Self::Unavailable => {
                formatter.write_str("ChatGPT auth server is temporarily unavailable (429 or 5xx)")
            }
            Self::Refused(Some(code)) => write!(formatter, "ChatGPT request was refused: {code}"),
            Self::Refused(None) => {
                formatter.write_str("ChatGPT request was refused by the auth server")
            }
            Self::Malformed(Some(field)) => write!(
                formatter,
                "ChatGPT server returned malformed payload (missing or invalid {field})"
            ),
            Self::Malformed(None) => {
                formatter.write_str("ChatGPT server returned malformed payload")
            }
            Self::NotEligible => formatter.write_str("this ChatGPT account isn't eligible"),
        }
    }
}

impl std::error::Error for CredentialError {}

/// Trait implemented by ChatGPT credential providers.
///
/// Consumers should call [`ChatGptCredential::access_token_after_rejection`] at most once
/// per failed request. If the resulting token is also rejected, they must call
/// [`ChatGptCredential::mark_token_rejected`] and stop retrying. Under worst-case contention
/// and retry conditions, this accounts for at most two credential calls totaling up to 150s.
pub trait ChatGptCredential: Send + Sync {
    /// Retrieve a valid access token.
    ///
    /// If an unexpired token with at least 5 minutes of remaining lifetime is available,
    /// it is returned lock-free. Otherwise, a single token refresh is attempted under the lock.
    /// Bound: up to 75s (45s lock wait + 30s network timeout).
    /// If an unknown HTTP response status is received during refresh, the owner remains signed in.
    fn access_token(&self) -> Result<String, CredentialError>;

    /// Attempt recovery after an access token was rejected (e.g. 401 Unauthorized).
    ///
    /// Refreshes if the file still holds `rejected`. If the stored token differs from
    /// `rejected` and has at least 5 minutes of lifetime remaining, it returns that token
    /// without refreshing.
    fn access_token_after_rejection(&self, rejected: &str) -> Result<String, CredentialError>;

    /// Mark an access token as rejected, clearing stored tokens if the file still holds `rejected`.
    fn mark_token_rejected(&self, rejected: &str) -> Result<(), CredentialError>;
}
