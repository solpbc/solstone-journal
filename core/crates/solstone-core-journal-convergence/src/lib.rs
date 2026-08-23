// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Journal- and lineage-bound claimed-dirty transaction.
//!
//! This crate deliberately owns no command-line surface, HTTP route, callosum
//! tract, generic filesystem traversal API, or output-path selection. It does
//! not acquire a journal root; callers hand it a
//! [`solstone_core_journal_io::JournalRoot`]. It owns no production completion
//! authority, no migration surface, no resume issuer, and no reciprocal
//! owner-operation file. It does not read, write, migrate, or unify with
//! `journal/health/catchup-state.json`. The resolver-authority lode replaces
//! [`OwnerBinding::issue_from_base`] and [`ClaimAdmission::issue_from_base`].

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod allocate;
mod claim;
mod digest;
mod error;
mod init;
mod intent;
mod layout;
mod lock;
mod owner;
mod preflight;
mod publish;
mod recover;
mod schema;
mod store;
#[cfg(test)]
mod test_support;
mod transaction;
mod walk;

pub use digest::RecordDigest;
pub use error::{ChangedWhat, ConvergenceError, DurableRole, Refusal};
pub use init::check_initialized;
pub use layout::DayKey;
pub use owner::{ClaimAdmission, OwnerBinding};
pub use preflight::{Admitted, CanonicalDaySet, Preflight, preflight};
pub use recover::{DayStoreRecovery, RecoveryReport, StoreVerdict};
pub use store::{ConvergenceStore, DaySnapshot};
pub use transaction::HeldDays;

#[cfg(test)]
// Tests plant and inspect journal files via std::fs; clippy.toml forbids those in production.
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod architecture {
    fn production_source(source: &str) -> String {
        let mut output = String::new();
        let mut skip_depth: i32 = 0;
        let mut skip_until_item = false;
        for line in source.lines() {
            let trimmed = line.trim_start();
            if skip_depth > 0 {
                skip_depth += line.chars().filter(|ch| *ch == '{').count() as i32;
                skip_depth -= line.chars().filter(|ch| *ch == '}').count() as i32;
                continue;
            }
            if skip_until_item {
                if trimmed.starts_with("#[") {
                    continue;
                }
                if line.contains('{') {
                    skip_until_item = false;
                    skip_depth = line.chars().filter(|ch| *ch == '{').count() as i32
                        - line.chars().filter(|ch| *ch == '}').count() as i32;
                    if skip_depth < 0 {
                        skip_depth = 0;
                    }
                    continue;
                }
                if trimmed.ends_with(';') {
                    skip_until_item = false;
                }
                continue;
            }
            if trimmed.starts_with("#[cfg(test)]") {
                skip_until_item = true;
                continue;
            }
            output.push_str(line);
            output.push('\n');
        }
        output
    }

    #[test]
    fn canonical_path_not_used_to_open() {
        for source in [
            include_str!("allocate.rs"),
            include_str!("claim.rs"),
            include_str!("digest.rs"),
            include_str!("error.rs"),
            include_str!("init.rs"),
            include_str!("intent.rs"),
            include_str!("layout.rs"),
            include_str!("lib.rs"),
            include_str!("lock.rs"),
            include_str!("owner.rs"),
            include_str!("preflight.rs"),
            include_str!("publish.rs"),
            include_str!("recover.rs"),
            include_str!("schema.rs"),
            include_str!("store.rs"),
            include_str!("transaction.rs"),
            include_str!("walk.rs"),
        ] {
            let production = production_source(source);
            assert!(!production.contains("canonical_path"));
            assert!(!production.contains("JournalRoot::open"));
        }
    }

    #[test]
    fn no_public_completion_or_migration_type() {
        // Recovery is a public read-only surface required by AC3/AC4/AC6
        // (`RecoveryReport`). This lode still bars a public *completion*
        // authority and any *migration* surface.
        let production = production_source(include_str!("lib.rs"));
        let public: String = production
            .lines()
            .filter(|line| {
                let trimmed = line.trim_start();
                trimmed.starts_with("pub use")
                    || trimmed.starts_with("pub struct")
                    || trimmed.starts_with("pub enum")
                    || trimmed.starts_with("pub fn")
            })
            .collect::<Vec<_>>()
            .join("\n")
            .to_ascii_lowercase();
        assert!(!public.contains("completion"));
        assert!(!public.contains("migration"));
        assert!(!public.contains("preparedcompletion"));
    }
}
