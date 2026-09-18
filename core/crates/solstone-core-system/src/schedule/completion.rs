// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;
use std::sync::Mutex;

use serde_json::{Map, Value, json};
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace};

use super::ScheduleError;

pub(crate) fn load_runtime_state(path: &Path) -> Result<Map<String, Value>, ScheduleError> {
    use solstone_core_journal_io::durability::{
        ArtifactId, DurableRead, read_json_durable_validated,
    };
    match read_json_durable_validated::<Value>(ArtifactId::SchedulerState, path, |val| {
        if val.is_object() {
            Ok(())
        } else {
            Err("scheduler state must be a JSON object".to_owned())
        }
    }) {
        Ok(DurableRead::Present(Value::Object(state))) => Ok(state),
        Ok(DurableRead::Present(_)) => unreachable!(),
        Ok(DurableRead::Absent | DurableRead::SetAside(_) | DurableRead::Unreadable { .. }) => {
            Ok(Map::new())
        }
        Err(error) => Err(ScheduleError::Io(error.to_string())),
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
    let mut state = load_runtime_state(path)?;
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
