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

mod bound_commit;
mod commit;
mod config;
#[cfg(any(unix, windows))]
mod credential_interlock;
mod direct_port;
mod pairing_migration;
mod thinking_migration;

#[cfg(test)]
mod test_support;

pub use bound_commit::conditional_update_journal_config_in_directory;
pub use bound_commit::{
    BoundConfigConfirmationRefusal, BoundConfigOutcome, BoundConfigPublication, BoundConfigRefusal,
    BoundConfigTransaction, DirectoryFlush, MAX_BOUND_CONFIG_BYTES,
    conditional_update_journal_config_at_root, conditional_update_journal_config_at_root_checked,
    conditional_update_journal_config_at_root_interlocked,
    conditional_update_journal_config_in_directory_checked, mutate_journal_config_at_root_checked,
    mutate_journal_config_at_root_interlocked,
};
pub use commit::{
    CommitConfigError, ConfigConflict, ConfigExpectation, ConfigFingerprint, commit_journal_config,
    commit_journal_config_checked,
};
pub use config::{
    CasConfigMutationError, ConfigMutationError, InsertDailyInputSinceError, JournalConfigMutation,
    JournalConfigTransaction, insert_daily_input_since, insert_daily_input_since_checked,
    mutate_journal_config, mutate_journal_config_cas, mutate_journal_config_checked,
};
pub use direct_port::{persist_direct_door_port, persist_direct_door_port_at_root_checked};
pub use pairing_migration::{PairingAddressMigrationReport, migrate_pairing_home_address};
pub use solstone_core_journal_config::ConfigLoadError;
pub use solstone_core_journal_io::{
    AtomicWriteError, LockError, LockOptions, LockTimeout, hold_lock,
};
pub use thinking_migration::{
    LegacyProviderCleanup, cleanup_legacy_provider_install_config, pin_google_model_aliases,
};
