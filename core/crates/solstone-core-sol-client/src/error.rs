// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;

pub const UNREACHABLE_MESSAGE: &str = "your journal couldn't be reached over HTTP.";
pub const TIMEOUT_MESSAGE: &str = "The journal didn't answer in time.";
pub const SERVICE_DOWN_MESSAGE: &str =
    "journal isn't running. start it with 'journal up' and retry.";
pub const MALFORMED_RESPONSE_MESSAGE: &str = "the journal response couldn't be read.";
pub const SERVER_ERROR_MESSAGE: &str = "The journal returned an unreadable error.";
pub const LOCAL_CONVEY_TIMEOUT_REASON: &str = "local_convey_timeout";

#[derive(Debug, Clone, PartialEq)]
pub enum ClientError {
    Unreachable {
        detail: Option<String>,
    },
    Timeout {
        detail: Option<String>,
    },
    MalformedSuccess {
        status: Option<u16>,
    },
    UnreadableServerError {
        status: Option<u16>,
    },
    ReasonRejected {
        status: u16,
        error: String,
        reason_code: Option<String>,
        detail: Option<String>,
        payload: Box<serde_json::Value>,
    },
}

impl ClientError {
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
        }
    }

    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            ClientError::Unreachable { .. } => UNREACHABLE_MESSAGE,
            ClientError::Timeout { .. } => TIMEOUT_MESSAGE,
            ClientError::MalformedSuccess { .. } => MALFORMED_RESPONSE_MESSAGE,
            ClientError::UnreadableServerError { .. } => SERVER_ERROR_MESSAGE,
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
            | ClientError::Timeout { detail }
            | ClientError::ReasonRejected { detail, .. } => detail.as_deref(),
            ClientError::MalformedSuccess { .. } | ClientError::UnreadableServerError { .. } => {
                None
            }
        }
    }

    #[must_use]
    pub fn status(&self) -> Option<u16> {
        match self {
            ClientError::MalformedSuccess { status }
            | ClientError::UnreadableServerError { status } => *status,
            ClientError::ReasonRejected { status, .. } => Some(*status),
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
