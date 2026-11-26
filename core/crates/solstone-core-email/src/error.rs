// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Typed errors for the email source.

use thiserror::Error;

/// Failure modes surfaced by the email source and its read-only IMAP client.
#[derive(Debug, Error)]
pub enum EmailError {
    /// Gmail rejected the email address or 16-character app password.
    #[error("Gmail rejected the email address or app password")]
    AuthFailed,
    /// Gmail authenticated, but no `\All` (All Mail) folder is exposed over IMAP.
    #[error("All Mail is not exposed over IMAP")]
    AllMailHidden,
    /// IMAP access is disabled for the account (for example, Workspace admin policy).
    #[error("IMAP access is disabled for this account")]
    ImapDisabled,
    /// Could not establish or maintain a TLS connection to the mail server.
    #[error("could not reach the mail server: {0}")]
    Unreachable(String),
    /// Server response violated the expected read-only IMAP protocol contract.
    #[error("unexpected IMAP response: {0}")]
    Protocol(String),
    /// The read-only wire guard refused a command that could mutate the mailbox.
    #[error("read-only IMAP guard refused command: {0}")]
    ReadOnlyGuard(String),
    /// Invalid owner input (email address, app password, range, or message types).
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// Email source is not connected.
    #[error("email source is not connected")]
    NotConnected,
    /// Another email scan or sync operation holds the source lock.
    #[error("another email scan or sync is already running")]
    Busy,
    /// Scan or sync was cancelled by the owner.
    #[error("cancelled")]
    Cancelled,
    /// SQLite header cache failure.
    #[error("header cache error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// Local storage or durable file write failure.
    #[error("storage error: {0}")]
    Storage(String),
}

impl EmailError {
    /// Stable machine-readable code for HTTP responses and persisted attention state.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::AuthFailed => "auth_failed",
            Self::AllMailHidden => "all_mail_hidden",
            Self::ImapDisabled => "imap_disabled",
            Self::Unreachable(_) => "unreachable",
            Self::Protocol(_) => "protocol",
            Self::ReadOnlyGuard(_) => "read_only_guard",
            Self::InvalidInput(_) => "invalid_input",
            Self::NotConnected => "not_connected",
            Self::Busy => "busy",
            Self::Cancelled => "cancelled",
            Self::Sqlite(_) | Self::Storage(_) => "storage",
        }
    }

    /// Whether this error puts the source into `needs_attention` and pauses
    /// automatic scheduled syncs until the owner fixes it.
    #[must_use]
    pub fn attention_reason(&self) -> Option<&'static str> {
        match self {
            Self::AuthFailed => Some("password_rejected"),
            Self::AllMailHidden => Some("all_mail_hidden"),
            Self::ImapDisabled => Some("imap_disabled"),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, EmailError>;
