// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;
use std::sync::Mutex;

use serde_json::{Map, Value, json};
use solstone_core_journal_io::{
    AtomicWriteOptions, MalformedPolicy, ReadError, atomic_replace, read_json,
};

use super::ScheduleError;

pub(crate) fn load_runtime_state(path: &Path) -> Result<Map<String, Value>, ScheduleError> {
    match read_json::<Value>(path, Value::Object(Map::new()), MalformedPolicy::Raise) {
        Ok(Value::Object(state)) => Ok(state),
        Ok(_) => Err(ScheduleError::StateShape {
            path: path.to_path_buf(),
        }),
        Err(ReadError::Malformed(_)) => Ok(Map::new()),
        Err(error) => Err(io_error(error)),
    }
}

pub(crate) fn record_completion(
    lock: &Mutex<()>,
    path: &Path,
    name: &str,
    ended_at: f64,
    exit_status: &str,
    reference: &str,
) -> Result<(), ScheduleError> {
    let _guard = lock.lock().expect("schedule completion lock poisoned");
    let mut state =
        match read_json::<Value>(path, Value::Object(Map::new()), MalformedPolicy::Raise) {
            Ok(Value::Object(state)) => state,
            // Python's `.get()` raises for a valid non-object; the task runner's
            // broad caller catches it. Surface the corresponding failure here.
            Ok(_) => {
                return Err(ScheduleError::StateShape {
                    path: path.to_path_buf(),
                });
            }
            Err(ReadError::Malformed(_)) => Map::new(),
            Err(error) => return Err(io_error(error)),
        };
    let current = state
        .get(name)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut current = current;
    current.insert("last_run".to_owned(), json!(ended_at));
    current.insert("last_status".to_owned(), json!(exit_status));
    current.insert("last_ref".to_owned(), json!(reference));
    state.insert(name.to_owned(), Value::Object(current));
    let bytes = serde_json::to_vec_pretty(&Value::Object(state))
        .map_err(|error| ScheduleError::Io(error.to_string()))?;
    atomic_replace(path, &bytes, AtomicWriteOptions::default()).map_err(io_error)
}

fn io_error(error: impl std::fmt::Display) -> ScheduleError {
    ScheduleError::Io(error.to_string())
}
