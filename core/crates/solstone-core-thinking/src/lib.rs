// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only projections for the native Thinking surface.

pub mod brain;
pub mod generators;
pub mod local;
pub mod providers;

use std::path::Path;

use serde_json::{Map, Value};
use solstone_core_journal_config::{ConfigLoadError, read_journal_config};
use solstone_core_journal_config_write::ConfigMutationError;

/// Errors returned by Thinking's config-owning mutations.
#[derive(Debug)]
pub enum MutationError {
    /// The stable config sidecar lock could not be acquired.
    ConfigLock(String),
    /// The locked config could not be loaded.
    ConfigLoad(String),
    /// The changed config could not be atomically written.
    ConfigWrite(String),
    /// The pre-mutation snapshot used by a stale-write guard could not load.
    Read(ConfigLoadError),
    /// The configuration committed, but its audit record could not be appended.
    ActionLog(String),
}

impl MutationError {
    pub fn config(error: ConfigMutationError) -> Self {
        match error {
            ConfigMutationError::Lock(error) => Self::ConfigLock(error.to_string()),
            ConfigMutationError::Load(error) => Self::ConfigLoad(error.to_string()),
            ConfigMutationError::Write(error) => Self::ConfigWrite(error.to_string()),
        }
    }
}

pub fn read_config(journal_root: &Path) -> Result<Map<String, Value>, ConfigLoadError> {
    Ok(read_journal_config(journal_root)?
        .config
        .unwrap_or_default())
}
