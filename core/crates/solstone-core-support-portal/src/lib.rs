// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Local durable idempotency ledger for support portal operations.

mod canonical;
mod client;
mod diagnostics;
mod dpop;
mod errors;
mod jwk;
mod keypair;
mod ledger;
mod token;

#[cfg(test)]
mod client_tests;
#[cfg(test)]
mod fake_portal;

pub use canonical::{
    CANONICAL_NAMESPACE, canonical_fingerprint, canonicalize_operation, derive_child_action_id,
    operation_key, principal_tag,
};
pub use client::PortalClient;
pub use diagnostics::{
    PlatformInfo, bounded_redacted_text, collect_all, collect_brain_health, collect_config,
    collect_platform, collect_recent_errors, collect_revision, collect_services, is_secret_key,
    native_platform, strip_secrets,
};
pub use errors::{OperationError, PortalClientError};
pub use ledger::{Ledger, OperationRecord};
