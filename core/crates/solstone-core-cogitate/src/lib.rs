// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Decision-layer contract for bounded cogitate talents.

pub mod access_tiers;
pub mod failure_codes;
pub mod finalization;
pub mod policy;
pub mod preambles;
pub mod prompt;
pub mod read_scope;

pub use access_tiers::{
    AccessCapabilities, AccessTier, AccessTierError, COGITATE_ACCESS_TIERS, FUTURE_ACCESS_TIERS,
    TALENT_ACCESS_TIERS, capabilities_for_access_tier,
};
pub use failure_codes::{
    DETERMINISTIC_FAILURE_CAPS, DETERMINISTIC_FAILURE_REASON_CODES, failure_capped,
};
pub use finalization::{FinalizationConfig, FinalizationValue, expects_emit_final};
pub use policy::{CommandDecision, classify_command};
pub use preambles::{
    COGITATE_DIAGNOSTIC_PREAMBLE, COGITATE_JOURNAL_COMMANDS, COGITATE_RUNTIME_PREAMBLE,
    TALENT_FINALIZATION_MODES,
};
pub use prompt::{cogitate_sol_tool_hint, compose_system_instruction};
pub use read_scope::{ReadScopeConfig, ReadScopeError, resolve_read_scope};

#[cfg(test)]
mod divergence;
#[cfg(test)]
mod oracle;
