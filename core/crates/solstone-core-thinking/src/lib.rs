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

pub fn read_config(journal_root: &Path) -> Result<Map<String, Value>, ConfigLoadError> {
    Ok(read_journal_config(journal_root)?
        .config
        .unwrap_or_default())
}
