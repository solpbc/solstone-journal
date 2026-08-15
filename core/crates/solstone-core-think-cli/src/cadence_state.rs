// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace};

#[derive(Clone, Debug, Default)]
pub(crate) struct CadenceState {
    values: Map<String, Value>,
}

impl CadenceState {
    pub(crate) fn load(journal: &Path) -> Self {
        let path = path(journal);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(error) => {
                log::warn!("Failed to load cadence state: {error}");
                return Self::default();
            }
        };
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
            log::warn!("Failed to load cadence state: invalid JSON");
            return Self::default();
        };
        let Some(values) = value.as_object().cloned() else {
            return Self::default();
        };
        Self { values }
    }

    pub(crate) fn timestamp(&self, name: &str) -> Option<i64> {
        self.values.get(name).and_then(Value::as_i64)
    }

    pub(crate) fn set_timestamp(&mut self, name: &str, timestamp: i64) {
        self.values.insert(name.to_owned(), Value::from(timestamp));
    }

    pub(crate) fn save(&self, journal: &Path) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(&self.values).map_err(|error| error.to_string())?;
        atomic_replace(path(journal), &bytes, AtomicWriteOptions::default())
            .map_err(|error| error.to_string())
    }
}

fn path(journal: &Path) -> PathBuf {
    journal.join("health/cadence.json")
}
