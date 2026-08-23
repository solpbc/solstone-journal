// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Strict, lock-guarded journal configuration writes.
//!
//! ```
//! use solstone_core_journal_config_write::{ConfigLoadError, ConfigMutationError};
//!
//! fn observe_load_error(error: ConfigMutationError) {
//!     match error {
//!         ConfigMutationError::Load(ConfigLoadError::Corrupt { path, .. }) => drop(path),
//!         _ => {}
//!     }
//! }
//! ```

mod commit;
mod config;
mod direct_port;
mod pairing_migration;
mod thinking_migration;

#[cfg(test)]
mod test_support;

pub use commit::{
    CommitConfigError, ConfigConflict, ConfigExpectation, ConfigFingerprint, commit_journal_config,
};
pub use config::{
    CasConfigMutationError, ConfigMutationError, JournalConfigMutation, JournalConfigTransaction,
    mutate_journal_config, mutate_journal_config_cas,
};
pub use direct_port::persist_direct_door_port;
pub use pairing_migration::{PairingAddressMigrationReport, migrate_pairing_home_address};
pub use solstone_core_journal_config::ConfigLoadError;
pub use solstone_core_journal_io::{
    AtomicWriteError, LockError, LockOptions, LockTimeout, hold_lock,
};
pub use thinking_migration::{
    LegacyProviderCleanup, cleanup_legacy_provider_install_config, pin_google_model_aliases,
    unify_provider_config,
};
