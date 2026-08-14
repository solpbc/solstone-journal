// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;

use serde_json::{Map, Value};
use solstone_core_journal_config::ConfigLoadError;
use solstone_core_journal_config_write::ConfigMutationError;

pub mod cli;
mod defaults;
mod destination;
mod hosted;
mod keys;
mod merge;
mod state;

pub use defaults::backup_defaults;
pub use destination::assemble_backend_env;
pub use hosted::{delete_hosted_binding, load_hosted_binding, save_hosted_binding};
pub use keys::{
    CROCKFORD_ALPHABET, RECOVERY_KEY_LENGTH, confirm_recovery_key, format_recovery_key_display,
    generate_daily_key, generate_recovery_key, parse_recovery_key,
};
pub use merge::merge_backup_config;
pub use state::{
    clear_backup_config, generate_and_store_keys, get_backup_config, get_destination, get_keys,
    record_backup_result, record_offload_result, record_prune_result, record_restore_result,
    record_verification_result, set_destination, set_enabled, set_mode, set_offload,
    set_recovery_key, set_recovery_key_confirmed, set_retention, status_view,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupKeys {
    pub daily_key: String,
    pub recovery_key: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Destination {
    pub repository: String,
    pub backend: String,
    pub credentials: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostedBinding {
    pub broker_endpoint: String,
    pub account_id: String,
    pub instance_id: String,
    pub bucket: String,
    pub prefix: String,
    pub broker_token: String,
}

#[derive(Debug)]
pub enum BackupError {
    ConfigLoad(ConfigLoadError),
    ConfigMutation(ConfigMutationError),
    Entropy,
    HostedWrite,
    HostedDelete,
    InvalidMode,
    InvalidRetentionShape,
    InvalidRetentionValue,
    InvalidOffloadShape,
    InvalidOffloadEnabled,
    InvalidOffloadBytes,
    InvalidOffloadStatus,
    InvalidVerificationStatus,
    InvalidRestoreStatus,
    InvalidRestoreScope,
    InvalidRestoreCounters,
    StoredKeys,
    CanonicalRecoveryLength,
    CanonicalRecoveryCharacters,
    RecoveryParse,
    MissingCredential(&'static str),
    UnsupportedBackend(String),
}

impl fmt::Display for BackupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConfigLoad(error) => error.fmt(formatter),
            Self::ConfigMutation(error) => error.fmt(formatter),
            Self::Entropy => formatter.write_str("could not generate backup key."),
            Self::HostedWrite => formatter.write_str("could not save hosted binding."),
            Self::HostedDelete => formatter.write_str("could not delete hosted binding."),
            Self::InvalidMode => formatter.write_str("backup mode must be byo or operated"),
            Self::InvalidRetentionShape => {
                formatter.write_str("backup retention must include hourly, daily, weekly, monthly")
            }
            Self::InvalidRetentionValue => {
                formatter.write_str("backup retention values must be non-negative integers")
            }
            Self::InvalidOffloadShape => formatter
                .write_str("backup offload must include enabled, budget_bytes, floor_bytes"),
            Self::InvalidOffloadEnabled => {
                formatter.write_str("backup offload enabled must be a boolean")
            }
            Self::InvalidOffloadBytes => {
                formatter.write_str("backup offload byte values must be positive integers or null")
            }
            Self::InvalidOffloadStatus => {
                formatter.write_str("backup offload status must be ok, skipped, stalled, or error")
            }
            Self::InvalidVerificationStatus => {
                formatter.write_str("backup verification status must be ok, skipped, or error")
            }
            Self::InvalidRestoreStatus => formatter
                .write_str("backup restore status must be ok, no_op, refused, degraded, or error"),
            Self::InvalidRestoreScope => {
                formatter.write_str("backup restore scope must be day or all")
            }
            Self::InvalidRestoreCounters => {
                formatter.write_str("backup restore counters must be non-negative integers")
            }
            Self::StoredKeys => formatter.write_str("backup keys must be strings when present"),
            Self::CanonicalRecoveryLength => {
                formatter.write_str("canonical recovery key must be exactly 64 characters")
            }
            Self::CanonicalRecoveryCharacters => {
                formatter.write_str("canonical recovery key contains invalid Crockford characters")
            }
            Self::RecoveryParse => formatter.write_str(
                "recovery key must contain exactly 64 Crockford characters after cleanup",
            ),
            Self::MissingCredential(key) => write!(formatter, "missing backup credential: {key}"),
            Self::UnsupportedBackend(_) => formatter.write_str("unsupported backup backend"),
        }
    }
}

impl std::error::Error for BackupError {}
