// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace};

fn path(journal: &Path) -> PathBuf {
    journal.join("health/cadence.json")
}

pub(crate) fn load(journal: &Path) -> BTreeMap<String, i64> {
    let path = path(journal);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return BTreeMap::new(),
        Err(error) => {
            log::warn!("Failed to load cadence state: {error}");
            return BTreeMap::new();
        }
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        log::warn!("Failed to load cadence state: invalid JSON");
        return BTreeMap::new();
    };
    let Some(object) = value.as_object() else {
        return BTreeMap::new();
    };
    object
        .iter()
        .filter_map(|(name, value)| value.as_i64().map(|stamp| (name.clone(), stamp)))
        .collect()
}

pub(crate) fn save(journal: &Path, state: &BTreeMap<String, i64>) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(state).map_err(|error| error.to_string())?;
    atomic_replace(path(journal), &bytes, AtomicWriteOptions::default())
        .map_err(|error| error.to_string())
}
