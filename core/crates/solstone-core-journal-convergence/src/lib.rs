// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Journal- and lineage-bound record store for monotonic per-day convergence state.
//!
//! This crate deliberately owns no command-line surface, HTTP route, callosum
//! tract, generic filesystem traversal API, or output-path selection. It does
//! not acquire a journal root; callers hand it a
//! [`solstone_core_journal_io::JournalRoot`]. It owns no recovery/intent
//! surface and no production completion authority. It does not read, write,
//! migrate, or unify with `journal/health/catchup-state.json`.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod allocate;
mod digest;
mod error;
mod init;
mod layout;
mod lock;
mod publish;
mod schema;
mod store;
#[cfg(test)]
mod test_support;
mod walk;

pub use digest::RecordDigest;
pub use error::{ChangedWhat, ConvergenceError, DurableRole, Refusal};
pub use init::{check_initialized, initialize};
pub use layout::{DayKey, validate_day_set};
pub use lock::{AllocationProof, DayLockSet};
pub use publish::{OrdinaryAuthority, OrdinaryIntent, PublishOutcome, ValidatedProposal};
pub use store::{ConvergenceStore, DaySnapshot, LoadDay, PendingKind};

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod architecture {
    #[test]
    fn canonical_path_not_used_to_open() {
        for source in [
            include_str!("store.rs"),
            include_str!("init.rs"),
            include_str!("lock.rs"),
            include_str!("allocate.rs"),
            include_str!("publish.rs"),
            include_str!("walk.rs"),
        ] {
            let production = source.split("#[cfg(test)]").next().unwrap_or(source);
            assert!(!production.contains("canonical_path"));
            assert!(!production.contains("JournalRoot::open"));
        }
    }

    #[test]
    fn no_public_completion_or_recovery_type() {
        let production = include_str!("lib.rs").split("#[cfg(test)]").next().unwrap();
        assert!(!production.contains("CompletionAuthority"));
        assert!(!production.contains("MigrationAuthority"));
        assert!(!production.contains("PreparedCompletionAuthority"));
        assert!(!production.contains("pub use") || !production.contains("Recovery"));
    }
}
