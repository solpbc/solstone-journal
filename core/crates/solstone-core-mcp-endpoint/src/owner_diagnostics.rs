// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Human-readable projections of the local MCP stores.

use crate::tokens::TokenLabelError;
use crate::{ActivityReadError, OAuthStoreError, PermissionStoreError, TokenStoreError};
use solstone_core_journal_io::cli_diagnostics::{atomic_message, lock_detail, path_detail};
use solstone_core_window_catalog as catalog;

impl OAuthStoreError {
    pub fn message_for(&self, locale: &str) -> String {
        match self {
            Self::Randomness => catalog::text(
                locale,
                "journal.window.mcp.owner.diagnostic.oauth.randomness",
            ),
            Self::InvalidToken => catalog::text(
                locale,
                "journal.window.mcp.owner.diagnostic.oauth.invalid.token",
            ),
            Self::InvalidGrant => catalog::text(
                locale,
                "journal.window.mcp.owner.diagnostic.oauth.invalid.grant",
            ),
            Self::RefreshReused { .. } => catalog::text(
                locale,
                "journal.window.mcp.owner.diagnostic.oauth.refresh.reused",
            ),
            Self::NoActivePairing => catalog::text(
                locale,
                "journal.window.mcp.owner.diagnostic.oauth.no.active.pairing",
            ),
            Self::Quota => catalog::text(locale, "journal.window.mcp.owner.diagnostic.oauth.quota"),
            Self::ClientNotFound => catalog::text(
                locale,
                "journal.window.mcp.owner.diagnostic.oauth.client.not.found",
            ),
            Self::TransactionNotFound => catalog::text(
                locale,
                "journal.window.mcp.owner.diagnostic.oauth.transaction.not.found",
            ),
            Self::TransactionExpired => catalog::text(
                locale,
                "journal.window.mcp.owner.diagnostic.oauth.transaction.expired",
            ),
            Self::TransactionExhausted => catalog::text(
                locale,
                "journal.window.mcp.owner.diagnostic.oauth.transaction.exhausted",
            ),
            Self::CodeExpired => catalog::text(
                locale,
                "journal.window.mcp.owner.diagnostic.oauth.code.expired",
            ),
            Self::BindingMismatch => catalog::text(
                locale,
                "journal.window.mcp.owner.diagnostic.oauth.binding.mismatch",
            ),
            Self::PairingMismatch => catalog::text(
                locale,
                "journal.window.mcp.owner.diagnostic.oauth.pairing.mismatch",
            ),
            Self::PairingLocked => catalog::text(
                locale,
                "journal.window.mcp.owner.diagnostic.oauth.pairing.locked",
            ),
            Self::Directory(_) => catalog::text(
                locale,
                "journal.window.mcp.owner.diagnostic.oauth.directory",
            ),
            Self::Lock(_) => {
                catalog::text(locale, "journal.window.mcp.owner.diagnostic.oauth.lock")
            }
            Self::Read { .. } => {
                catalog::text(locale, "journal.window.mcp.owner.diagnostic.oauth.read")
            }
            Self::Malformed { .. } => catalog::text(
                locale,
                "journal.window.mcp.owner.diagnostic.oauth.malformed",
            ),
            Self::UnsupportedSchema { .. } => catalog::text(
                locale,
                "journal.window.mcp.owner.diagnostic.oauth.unsupported.schema",
            ),
            Self::Write(_) => {
                catalog::text(locale, "journal.window.mcp.owner.diagnostic.oauth.write")
            }
            Self::EntryTooLarge => catalog::text(
                locale,
                "journal.window.mcp.owner.diagnostic.oauth.entry.too.large",
            ),
            Self::StateTooLarge => catalog::text(
                locale,
                "journal.window.mcp.owner.diagnostic.oauth.state.too.large",
            ),
            Self::Permission => catalog::text(
                locale,
                "journal.window.mcp.owner.diagnostic.oauth.permission",
            ),
        }
    }
}

impl TokenLabelError {
    pub fn message_for(&self, locale: &str) -> String {
        match self {
            Self::AsciiControl => catalog::text(
                locale,
                "journal.window.mcp.owner.diagnostic.label.ascii.control",
            ),
            Self::DisallowedByte => catalog::text(
                locale,
                "journal.window.mcp.owner.diagnostic.label.disallowed.byte",
            ),
            Self::Empty => catalog::text(locale, "journal.window.mcp.owner.diagnostic.label.empty"),
            Self::TooLong => {
                catalog::text(locale, "journal.window.mcp.owner.diagnostic.label.too.long")
            }
            Self::Boundary => {
                catalog::text(locale, "journal.window.mcp.owner.diagnostic.label.boundary")
            }
        }
    }
}

impl TokenStoreError {
    pub fn message_for(&self, locale: &str) -> String {
        match self {
            Self::InvalidLabel(error) => catalog::strings(
                locale,
                "journal.window.mcp.owner.diagnostic.token.invalid.label",
                [("error", &error.message_for(locale))],
            ),
            Self::DuplicateLabel { label } => catalog::strings(
                locale,
                "journal.window.mcp.owner.diagnostic.token.duplicate.label",
                [("label", &format!("{:?}", label))],
            ),
            Self::NotFound { label } => catalog::strings(
                locale,
                "journal.window.mcp.owner.diagnostic.token.not.found",
                [("label", &format!("{:?}", label))],
            ),
            Self::Randomness => catalog::text(
                locale,
                "journal.window.mcp.owner.diagnostic.token.randomness",
            ),
            Self::InvalidToken => catalog::text(
                locale,
                "journal.window.mcp.owner.diagnostic.token.invalid.token",
            ),
            Self::Directory(error) => catalog::strings(
                locale,
                "journal.window.mcp.owner.diagnostic.token.directory",
                [("error", &path_detail(locale, error))],
            ),
            Self::Lock(error) => catalog::strings(
                locale,
                "journal.window.mcp.owner.diagnostic.token.lock",
                [("error", &lock_detail(locale, error))],
            ),
            Self::Read { path, source } => catalog::strings(
                locale,
                "journal.window.mcp.owner.diagnostic.token.read",
                [
                    ("path", &path.display().to_string()),
                    ("error", &source.to_string()),
                ],
            ),
            Self::Malformed { path } => catalog::strings(
                locale,
                "journal.window.mcp.owner.diagnostic.token.malformed",
                [("path", &path.display().to_string())],
            ),
            Self::UnsupportedSchema { path, found } => catalog::strings(
                locale,
                "journal.window.mcp.owner.diagnostic.token.unsupported.schema",
                [
                    ("schema", &found.to_string()),
                    ("path", &path.display().to_string()),
                ],
            ),
            Self::Write(error) => catalog::strings(
                locale,
                "journal.window.mcp.owner.diagnostic.token.write",
                [("error", &atomic_message(error).render(locale))],
            ),
        }
    }
}

impl PermissionStoreError {
    pub fn message_for(&self, locale: &str) -> String {
        match self {
            Self::Directory(error) => catalog::strings(
                locale,
                "journal.window.mcp.owner.diagnostic.permission.directory",
                [("error", &path_detail(locale, error))],
            ),
            Self::Lock(error) => catalog::strings(
                locale,
                "journal.window.mcp.owner.diagnostic.permission.lock",
                [("error", &lock_detail(locale, error))],
            ),
            Self::Read { path, source } => catalog::strings(
                locale,
                "journal.window.mcp.owner.diagnostic.permission.read",
                [
                    ("path", &path.display().to_string()),
                    ("error", &source.to_string()),
                ],
            ),
            Self::Malformed { path, reason } => catalog::strings(
                locale,
                "journal.window.mcp.owner.diagnostic.permission.malformed",
                [("path", &path.display().to_string()), ("reason", reason)],
            ),
            Self::UnsupportedSchema { found, path } => catalog::strings(
                locale,
                "journal.window.mcp.owner.diagnostic.permission.unsupported.schema",
                [
                    ("schema", &found.to_string()),
                    ("path", &path.display().to_string()),
                ],
            ),
            Self::Write(error) => catalog::strings(
                locale,
                "journal.window.mcp.owner.diagnostic.permission.write",
                [("error", &atomic_message(error).render(locale))],
            ),
            Self::ConnectionNotFound { key } => catalog::strings(
                locale,
                "journal.window.mcp.owner.diagnostic.permission.connection.not.found",
                [("key", key)],
            ),
            Self::InvalidConnectionKey { key } => catalog::strings(
                locale,
                "journal.window.mcp.owner.diagnostic.permission.invalid.connection.key",
                [("key", key)],
            ),
            Self::AgentStateFault => catalog::text(
                locale,
                "journal.window.mcp.owner.diagnostic.permission.agent.state.fault",
            ),
        }
    }
}

impl ActivityReadError {
    pub fn message_for(&self, locale: &str) -> String {
        match self {
            Self::Path(error) => catalog::strings(
                locale,
                "journal.window.mcp.owner.diagnostic.activity.path",
                [("error", &path_detail(locale, error))],
            ),
        }
    }
}
