// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;

pub const UNREACHABLE_MESSAGE: &str = "your journal couldn't be reached over HTTP.";
pub const TIMEOUT_MESSAGE: &str = "The journal didn't answer in time.";
pub const SERVICE_DOWN_MESSAGE: &str =
    "journal isn't running. start it with 'solstone journal up' and retry.";
pub const MALFORMED_RESPONSE_MESSAGE: &str = "the journal response couldn't be read.";
pub const SERVER_ERROR_MESSAGE: &str = "The journal returned an unreadable error.";
pub const NOT_SET_UP_MESSAGE: &str = "your journal isn't set up yet. finish setup, then try again.";
pub const LOCAL_CONVEY_TIMEOUT_REASON: &str = "local_convey_timeout";

#[derive(Debug, Clone, PartialEq)]
pub enum ClientError {
    Unreachable {
        detail: Option<String>,
    },
    Timeout {
        detail: Option<String>,
        presentation: Option<solstone_core_cli_copy::Diagnostic>,
    },
    MalformedSuccess {
        status: Option<u16>,
    },
    UnreadableServerError {
        status: Option<u16>,
    },
    /// The journal sent the call to its first-run setup page.
    NotSetUp {
        status: u16,
    },
    ReasonRejected {
        status: u16,
        error: String,
        reason_code: Option<String>,
        detail: Option<String>,
        payload: Box<serde_json::Value>,
    },
}

pub fn service_down_for(locale: &str) -> String {
    solstone_core_cli_copy::render_for(locale, "journal.cli.streams.supervisor", &[])
}

impl ClientError {
    #[must_use]
    pub fn message_for(&self, locale: &str) -> String {
        let id = match self {
            ClientError::Unreachable { .. } => "journal.cli.client.error.unreachable",
            ClientError::Timeout { .. } => "journal.cli.client.error.timeout",
            ClientError::MalformedSuccess { .. } => "journal.cli.client.error.malformed",
            ClientError::UnreadableServerError { .. } => "journal.cli.client.error.unreadable",
            ClientError::NotSetUp { .. } => "journal.cli.client.error.not.set.up",
            ClientError::ReasonRejected { error, .. } => return error.to_owned(),
        };
        solstone_core_cli_copy::render_for(locale, id, &[])
    }

    #[must_use]
    pub fn unreachable(detail: impl Into<Option<String>>) -> Self {
        Self::Unreachable {
            detail: detail.into(),
        }
    }

    #[must_use]
    pub fn timeout(detail: impl Into<Option<String>>) -> Self {
        Self::Timeout {
            detail: detail.into(),
            presentation: None,
        }
    }

    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            ClientError::Unreachable { .. } => UNREACHABLE_MESSAGE,
            ClientError::Timeout { .. } => TIMEOUT_MESSAGE,
            ClientError::MalformedSuccess { .. } => MALFORMED_RESPONSE_MESSAGE,
            ClientError::UnreadableServerError { .. } => SERVER_ERROR_MESSAGE,
            ClientError::NotSetUp { .. } => NOT_SET_UP_MESSAGE,
            ClientError::ReasonRejected { error, .. } => error,
        }
    }

    #[must_use]
    pub fn reason_code(&self) -> Option<&str> {
        match self {
            ClientError::Timeout { .. } => Some(LOCAL_CONVEY_TIMEOUT_REASON),
            ClientError::ReasonRejected { reason_code, .. } => reason_code.as_deref(),
            _ => None,
        }
    }

    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        match self {
            ClientError::Unreachable { detail }
            | ClientError::Timeout { detail, .. }
            | ClientError::ReasonRejected { detail, .. } => detail.as_deref(),
            ClientError::MalformedSuccess { .. }
            | ClientError::UnreadableServerError { .. }
            | ClientError::NotSetUp { .. } => None,
        }
    }

    #[must_use]
    pub fn detail_for(&self, locale: &str) -> Option<String> {
        match self {
            Self::Timeout {
                presentation: Some(message),
                ..
            } => Some(message.render(locale)),
            _ => self.detail().map(str::to_owned),
        }
    }

    #[must_use]
    pub fn status(&self) -> Option<u16> {
        match self {
            ClientError::MalformedSuccess { status }
            | ClientError::UnreadableServerError { status } => *status,
            ClientError::ReasonRejected { status, .. } | ClientError::NotSetUp { status } => {
                Some(*status)
            }
            ClientError::Unreachable { .. } | ClientError::Timeout { .. } => None,
        }
    }

    #[must_use]
    pub fn payload(&self) -> Option<&serde_json::Value> {
        match self {
            ClientError::ReasonRejected { payload, .. } => Some(payload),
            _ => None,
        }
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for ClientError {}
