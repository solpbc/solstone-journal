// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owner-visible local operation failures.

use thiserror::Error;

/// Typed failure returned by the local support-operation ledger.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OperationError {
    /// The caller supplied canonical operation input the reference rejects with `ValueError`.
    #[error("{message}")]
    OperationInputInvalid {
        /// The canonicalizer's reference-compatible validation message.
        message: String,
    },
    /// A current generation owns an unexpired lease.
    #[error("That operation is already in progress.")]
    OperationInProgress,
    /// The same action ID was used with different content.
    #[error("That operation conflicts with an earlier attempt.")]
    IdempotencyConflict,
    /// The requested state transition is not legal.
    #[error("That operation isn't available in the current state.")]
    OperationInvalidState,
    /// The portal changed terms after the operation was started.
    #[error("Support terms changed and require re-consent.")]
    OperationTosChanged,
    /// A compacted terminal operation cannot be resumed.
    #[error("That operation is no longer available.")]
    OperationRetired,
    /// The portal tombstoned an operation.
    #[error("That operation was erased.")]
    OperationErased,
    /// A later generation replaced the caller's lease.
    #[error("That operation isn't available in the current state.")]
    OperationSuperseded,
    /// The filesystem lock could not be acquired for a reason other than contention.
    #[error("{message}")]
    OperationLockUnavailable {
        /// The underlying lock failure, preserved rather than misreported as contention.
        message: String,
    },
    /// The local state cannot be safely read or written.
    #[error("{message}")]
    OperationStateUnavailable {
        /// A fixed, reference-compatible diagnostic literal.
        message: &'static str,
    },
}

impl OperationError {
    /// Return the owner-visible reason code when routes map this error directly.
    pub const fn reason_code(&self) -> Option<&'static str> {
        match self {
            Self::OperationInputInvalid { .. }
            | Self::OperationLockUnavailable { .. }
            | Self::OperationStateUnavailable { .. } => Some("support_portal_failed"),
            Self::OperationInProgress => Some("operation_in_progress"),
            Self::IdempotencyConflict => Some("idempotency_conflict"),
            Self::OperationInvalidState => Some("invalid_state"),
            Self::OperationTosChanged => Some("tos_changed"),
            Self::OperationRetired => Some("operation_retired"),
            Self::OperationErased => Some("operation_erased"),
            // Measured at solstone/apps/support/routes.py:_operation_error_response:
            // Python has no OperationSupersededError branch, so it falls through
            // to the generic support-portal failure response instead.
            Self::OperationSuperseded => Some("support_portal_failed"),
        }
    }

    /// Return the matching HTTP status when the reference route maps this error.
    pub const fn http_status(&self) -> Option<u16> {
        match self {
            Self::OperationInputInvalid { .. }
            | Self::OperationLockUnavailable { .. }
            | Self::OperationStateUnavailable { .. }
            | Self::OperationSuperseded => Some(500),
            Self::OperationInProgress | Self::IdempotencyConflict | Self::OperationInvalidState => {
                Some(409)
            }
            Self::OperationTosChanged => Some(401),
            Self::OperationRetired | Self::OperationErased => Some(410),
        }
    }

    /// Return the owner-visible message associated with a mapped reason.
    pub const fn owner_message(&self) -> Option<&'static str> {
        match self {
            Self::OperationInputInvalid { .. }
            | Self::OperationLockUnavailable { .. }
            | Self::OperationStateUnavailable { .. }
            | Self::OperationSuperseded => Some("I couldn't reach support right now."),
            Self::OperationInProgress => Some("That operation is already in progress."),
            Self::IdempotencyConflict => Some("That operation conflicts with an earlier attempt."),
            Self::OperationInvalidState => {
                Some("That operation isn't available in the current state.")
            }
            Self::OperationTosChanged => Some("Support terms changed and require re-consent."),
            Self::OperationRetired => Some("That operation is no longer available."),
            Self::OperationErased => Some("That operation was erased."),
        }
    }
}

pub(crate) const LEDGER_BUSY: &str = "operation ledger is busy";
pub(crate) const KEY_BUSY: &str = "operation fingerprint key is busy";
pub(crate) const KEY_UNSAFE: &str = "operation fingerprint key permissions are unsafe";
pub(crate) const KEY_UNREADABLE: &str = "operation fingerprint key is unreadable";
pub(crate) const KEY_INVALID: &str = "operation fingerprint key is invalid";
pub(crate) const KEY_UNAVAILABLE: &str = "operation fingerprint key is unavailable";
pub(crate) const RECORD_UNREADABLE: &str = "operation ledger record is unreadable";
pub(crate) const RECORD_INVALID: &str = "operation ledger record is invalid";
pub(crate) const TIMESTAMP_INVALID: &str = "operation timestamp is invalid";
pub(crate) const ACTION_ID_INVALID: &str = "invalid operation action id";
