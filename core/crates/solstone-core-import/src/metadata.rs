// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Open import metadata records.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace, path_lexists};

use crate::{ImportError, OrderedMetadata};

/// Ordered JSON object stored in `imports/<id>/import.json`.
pub type ImportMetadata = OrderedMetadata;

/// The state of an import attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptState {
    Running,
    Completed,
    Unconfirmed,
}

/// Authoritative attempt facts stored in `imports/<id>/import.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptFacts {
    pub attempt_id: String,
    pub generation: u64,
    pub state: AttemptState,
    pub started_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_description: Option<String>,
    /// Inputs of a multi-input import that could not be imported while the rest were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_failures: Option<u64>,
}

/// The read state of an import attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptRead {
    Present(AttemptFacts),
    Absent,
    Malformed,
}

/// Read attempt facts from an import metadata map, distinguishing Absent and Malformed.
#[must_use]
pub fn read_attempt_facts(metadata: &ImportMetadata) -> AttemptRead {
    match metadata.get("attempt") {
        None => AttemptRead::Absent,
        Some(val) => {
            if val.is_null() {
                AttemptRead::Malformed
            } else {
                match serde_json::from_value::<AttemptFacts>(val.clone()) {
                    Ok(facts) => AttemptRead::Present(facts),
                    Err(_) => AttemptRead::Malformed,
                }
            }
        }
    }
}

/// Extract attempt facts from an import metadata map if present.
#[must_use]
pub fn get_attempt_facts(metadata: &ImportMetadata) -> Option<AttemptFacts> {
    match read_attempt_facts(metadata) {
        AttemptRead::Present(facts) => Some(facts),
        AttemptRead::Absent | AttemptRead::Malformed => None,
    }
}

/// Hold the per-import directory advisory mutation lock.
pub fn hold_import_lock(
    journal_root: &Path,
    import_id: &str,
) -> Result<solstone_core_journal_io::FileLock, ImportError> {
    let import_dir = crate::staging::ensure_import_private_chain(journal_root, import_id)?;
    let lock_path = import_dir.join(".lock");
    solstone_core_journal_io::hold_lock(lock_path, solstone_core_journal_io::LockOptions::default())
        .map_err(|err| ImportError::LockFailed {
            path: import_dir,
            message: err.to_string(),
        })
}

/// Admit an in-flight attempt in import.json under the import lock before any source or chronicle mutation.
pub fn admit_running_attempt(
    journal_root: &Path,
    import_id: &str,
    started_at_ms: u64,
    source_hint: Option<&str>,
) -> Result<AttemptFacts, ImportError> {
    let _lock = hold_import_lock(journal_root, import_id)?;
    let mut metadata = match read_provenance(journal_root, import_id)? {
        Some(m) => m,
        None => ImportMetadata::new(),
    };
    let previous_gen = match read_attempt_facts(&metadata) {
        AttemptRead::Present(f) => f.generation,
        AttemptRead::Absent => 0,
        AttemptRead::Malformed => {
            return Err(ImportError::InvalidAttemptState {
                message: format!("attempt block in {import_id} is malformed"),
            });
        }
    };
    let generation = previous_gen.saturating_add(1);
    let facts = AttemptFacts {
        attempt_id: format!("{import_id}:{generation}"),
        generation,
        state: AttemptState::Running,
        started_at_ms,
        finished_at_ms: None,
        duration_ms: None,
        failure_reason: None,
        unavailable_description: None,
        input_failures: None,
    };
    let value = serde_json::to_value(&facts).map_err(|err| ImportError::MetadataWriteFailed {
        path: import_metadata_path(journal_root, import_id)
            .unwrap_or_else(|_| PathBuf::from(import_id)),
        message: err.to_string(),
    })?;
    metadata.insert("attempt".to_owned(), value);
    metadata.insert("task_id".to_owned(), serde_json::json!(import_id));
    if let Some(hint) = source_hint {
        metadata.insert("source_hint".to_owned(), serde_json::json!(hint));
    }
    write_import_metadata_unlocked(journal_root, import_id, &metadata)?;
    Ok(facts)
}

/// Record an in-flight attempt in import.json before any source or chronicle mutation.
pub fn record_running_attempt(
    journal_root: &Path,
    import_id: &str,
    started_at_ms: u64,
) -> Result<AttemptFacts, ImportError> {
    admit_running_attempt(journal_root, import_id, started_at_ms, None)
}

/// Commit completed attempt facts into import.json without taking the lock (caller holds lock).
pub fn record_completed_attempt_unlocked(
    journal_root: &Path,
    import_id: &str,
    expected_generation: u64,
    finished_at_ms: u64,
    duration_ms: Option<u64>,
    unavailable_description: Option<String>,
) -> Result<AttemptFacts, ImportError> {
    complete_attempt_unlocked(
        journal_root,
        import_id,
        expected_generation,
        finished_at_ms,
        duration_ms,
        unavailable_description,
        None,
    )
}

/// As [`record_completed_attempt_unlocked`], also recording how many inputs of a
/// multi-input import could not be imported (caller holds the lock).
pub fn record_completed_attempt_with_input_failures_unlocked(
    journal_root: &Path,
    import_id: &str,
    expected_generation: u64,
    finished_at_ms: u64,
    duration_ms: Option<u64>,
    input_failures: u64,
) -> Result<AttemptFacts, ImportError> {
    complete_attempt_unlocked(
        journal_root,
        import_id,
        expected_generation,
        finished_at_ms,
        duration_ms,
        None,
        Some(input_failures).filter(|count| *count > 0),
    )
}

fn complete_attempt_unlocked(
    journal_root: &Path,
    import_id: &str,
    expected_generation: u64,
    finished_at_ms: u64,
    duration_ms: Option<u64>,
    unavailable_description: Option<String>,
    input_failures: Option<u64>,
) -> Result<AttemptFacts, ImportError> {
    let mut metadata = read_import_metadata(journal_root, import_id)?;
    let mut facts =
        get_attempt_facts(&metadata).ok_or_else(|| ImportError::InvalidAttemptState {
            message: format!("no active attempt found for {import_id}"),
        })?;
    if facts.generation != expected_generation || facts.state != AttemptState::Running {
        return Err(ImportError::InvalidAttemptState {
            message: format!(
                "attempt generation mismatch for {import_id}: expected running gen {expected_generation}, found state {:?} gen {}",
                facts.state, facts.generation
            ),
        });
    }
    let duration = duration_ms.or_else(|| Some(finished_at_ms.saturating_sub(facts.started_at_ms)));
    facts.state = AttemptState::Completed;
    facts.finished_at_ms = Some(finished_at_ms);
    facts.duration_ms = duration;
    facts.unavailable_description = unavailable_description;
    facts.input_failures = input_failures;

    let value = serde_json::to_value(&facts).map_err(|err| ImportError::MetadataWriteFailed {
        path: import_metadata_path(journal_root, import_id)
            .unwrap_or_else(|_| PathBuf::from(import_id)),
        message: err.to_string(),
    })?;
    metadata.insert("attempt".to_owned(), value);
    write_import_metadata_unlocked(journal_root, import_id, &metadata)?;
    Ok(facts)
}

/// Commit completed attempt facts into import.json.
pub fn record_completed_attempt(
    journal_root: &Path,
    import_id: &str,
    expected_generation: u64,
    finished_at_ms: u64,
    duration_ms: Option<u64>,
    unavailable_description: Option<String>,
) -> Result<AttemptFacts, ImportError> {
    let _lock = hold_import_lock(journal_root, import_id)?;
    record_completed_attempt_unlocked(
        journal_root,
        import_id,
        expected_generation,
        finished_at_ms,
        duration_ms,
        unavailable_description,
    )
}

/// Mark attempt as unconfirmed or failed in import.json without taking the lock (caller holds lock).
pub fn record_unconfirmed_attempt_unlocked(
    journal_root: &Path,
    import_id: &str,
    expected_generation: u64,
    finished_at_ms: u64,
    failure_reason: Option<String>,
) -> Result<AttemptFacts, ImportError> {
    let mut metadata = read_import_metadata(journal_root, import_id)?;
    let mut facts =
        get_attempt_facts(&metadata).ok_or_else(|| ImportError::InvalidAttemptState {
            message: format!("no active attempt found for {import_id}"),
        })?;
    if facts.generation != expected_generation || facts.state != AttemptState::Running {
        return Err(ImportError::InvalidAttemptState {
            message: format!(
                "attempt generation mismatch for {import_id}: expected running gen {expected_generation}, found state {:?} gen {}",
                facts.state, facts.generation
            ),
        });
    }
    let duration = Some(finished_at_ms.saturating_sub(facts.started_at_ms));
    facts.state = AttemptState::Unconfirmed;
    facts.finished_at_ms = Some(finished_at_ms);
    facts.duration_ms = duration;
    facts.failure_reason = failure_reason;

    let value = serde_json::to_value(&facts).map_err(|err| ImportError::MetadataWriteFailed {
        path: import_metadata_path(journal_root, import_id)
            .unwrap_or_else(|_| PathBuf::from(import_id)),
        message: err.to_string(),
    })?;
    metadata.insert("attempt".to_owned(), value);
    write_import_metadata_unlocked(journal_root, import_id, &metadata)?;
    Ok(facts)
}

/// Mark attempt as unconfirmed or failed in import.json.
pub fn record_unconfirmed_attempt(
    journal_root: &Path,
    import_id: &str,
    expected_generation: u64,
    finished_at_ms: u64,
    failure_reason: Option<String>,
) -> Result<AttemptFacts, ImportError> {
    let _lock = hold_import_lock(journal_root, import_id)?;
    record_unconfirmed_attempt_unlocked(
        journal_root,
        import_id,
        expected_generation,
        finished_at_ms,
        failure_reason,
    )
}

/// Read a complete open import metadata record.
pub fn read_import_metadata(
    journal_root: &Path,
    import_id: &str,
) -> Result<ImportMetadata, ImportError> {
    let path = import_metadata_path(journal_root, import_id)?;
    let bytes = fs::read(&path).map_err(|error| ImportError::MetadataCorrupt {
        path: path.clone(),
        message: error.to_string(),
    })?;
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|error| ImportError::MetadataCorrupt {
            path: path.clone(),
            message: error.to_string(),
        })?;
    let Value::Object(metadata) = value else {
        return Err(ImportError::MetadataCorrupt {
            path,
            message: "import metadata must be a JSON object".to_owned(),
        });
    };
    Ok(metadata)
}

/// Read provenance when metadata exists, treating only absence as no provenance.
pub fn read_provenance(
    journal_root: &Path,
    import_id: &str,
) -> Result<Option<ImportMetadata>, ImportError> {
    let path = import_metadata_path(journal_root, import_id)?;
    if !path_lexists(&path).map_err(|error| ImportError::PathResolution {
        path: path.clone(),
        message: error.to_string(),
    })? {
        return Ok(None);
    }
    read_import_metadata(journal_root, import_id).map(Some)
}

/// Atomically write a complete ordered import metadata record.
pub fn write_import_metadata(
    journal_root: &Path,
    import_id: &str,
    metadata: &ImportMetadata,
) -> Result<PathBuf, ImportError> {
    let _lock = hold_import_lock(journal_root, import_id)?;
    let mut merged = metadata.clone();
    match read_provenance(journal_root, import_id) {
        Ok(Some(existing)) => {
            if let Some(att) = existing.get("attempt") {
                merged.insert("attempt".to_owned(), att.clone());
            } else {
                merged.remove("attempt");
            }
            // The durable task id wins when there is one; a start that has none yet
            // (the queued path records it here for the first time) keeps the caller's.
            if let Some(tid) = existing.get("task_id") {
                merged.insert("task_id".to_owned(), tid.clone());
            }
        }
        Ok(None) => {}
        Err(err) => return Err(err),
    }
    write_import_metadata_unlocked(journal_root, import_id, &merged)
}

pub(crate) fn write_import_metadata_unlocked(
    journal_root: &Path,
    import_id: &str,
    metadata: &ImportMetadata,
) -> Result<PathBuf, ImportError> {
    let import_dir = crate::staging::ensure_import_private_chain(journal_root, import_id)?;
    let path = import_dir.join("import.json");
    let bytes =
        serde_json::to_vec_pretty(metadata).map_err(|error| ImportError::MetadataWriteFailed {
            path: path.clone(),
            message: error.to_string(),
        })?;
    atomic_replace(&path, &bytes, AtomicWriteOptions { mode: Some(0o600) }).map_err(|error| {
        ImportError::MetadataWriteFailed {
            path: path.clone(),
            message: error.to_string(),
        }
    })?;
    Ok(path)
}

pub(crate) fn import_metadata_path(
    journal_root: &Path,
    import_id: &str,
) -> Result<PathBuf, ImportError> {
    Ok(crate::staging::import_directory(journal_root, import_id)?.join("import.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn record_running_attempt_creates_and_fails_closed_on_corrupt() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let id = "20260408_120000";

        // Initial running attempt on clean directory
        let facts = admit_running_attempt(root, id, 1000, None).unwrap();
        assert_eq!(facts.attempt_id, format!("{id}:1"));
        assert_eq!(facts.generation, 1);
        assert_eq!(facts.state, AttemptState::Running);
        assert_eq!(facts.started_at_ms, 1000);

        // Verify metadata on disk
        let meta = read_import_metadata(root, id).unwrap();
        let loaded = get_attempt_facts(&meta).unwrap();
        assert_eq!(loaded.state, AttemptState::Running);
        assert_eq!(loaded.generation, 1);

        // Corrupt the metadata file
        let path = import_metadata_path(root, id).unwrap();
        fs::write(&path, b"not json").unwrap();

        // Fail closed
        let err = admit_running_attempt(root, id, 2000, None).unwrap_err();
        assert!(matches!(err, ImportError::MetadataCorrupt { .. }));
    }

    #[test]
    fn record_completed_and_unconfirmed_attempts_validate_generation() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let id = "20260408_130000";

        let facts = admit_running_attempt(root, id, 1000, None).unwrap();
        assert_eq!(facts.generation, 1);

        // Mismatched generation is rejected
        let err = record_completed_attempt(
            root,
            id,
            2,
            1500,
            Some(500),
            Some("unavail desc".to_owned()),
        )
        .unwrap_err();
        assert!(matches!(err, ImportError::InvalidAttemptState { .. }));

        let completed = record_completed_attempt(
            root,
            id,
            1,
            1500,
            Some(500),
            Some("unavail desc".to_owned()),
        )
        .unwrap();

        assert_eq!(completed.state, AttemptState::Completed);
        assert_eq!(completed.generation, 1);
        assert_eq!(completed.duration_ms, Some(500));
        assert_eq!(
            completed.unavailable_description.as_deref(),
            Some("unavail desc")
        );

        // Subsequent attempt increments generation
        let facts2 = admit_running_attempt(root, id, 2000, None).unwrap();
        assert_eq!(facts2.generation, 2);
        assert_eq!(facts2.attempt_id, format!("{id}:2"));

        let unconfirmed =
            record_unconfirmed_attempt(root, id, 2, 2600, Some("disk error".to_owned())).unwrap();
        assert_eq!(unconfirmed.state, AttemptState::Unconfirmed);
        assert_eq!(unconfirmed.generation, 2);
        assert_eq!(unconfirmed.failure_reason.as_deref(), Some("disk error"));
    }

    #[test]
    fn admit_running_attempt_refuses_malformed_attempt_and_preserves_bytes() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let id = "20260408_140000";
        let import_dir = root.join("imports").join(id);
        fs::create_dir_all(&import_dir).unwrap();
        let path = import_dir.join("import.json");
        let initial_bytes = b"{\n  \"attempt\": null,\n  \"setting\": \"original\"\n}\n";
        fs::write(&path, initial_bytes).unwrap();

        let err = admit_running_attempt(root, id, 1000, None).unwrap_err();
        assert!(matches!(err, ImportError::InvalidAttemptState { .. }));

        let content = fs::read(&path).unwrap();
        assert_eq!(content, initial_bytes);
    }

    #[test]
    fn attempt_missing_generation_is_malformed() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let id = "20260408_143000";
        let import_dir = root.join("imports").join(id);
        fs::create_dir_all(&import_dir).unwrap();
        let path = import_dir.join("import.json");
        // missing generation field
        let initial_bytes = br#"{
  "attempt": {
    "attempt_id": "20260408_143000:1",
    "state": "running",
    "started_at_ms": 1000
  }
}"#;
        fs::write(&path, initial_bytes).unwrap();

        let err = admit_running_attempt(root, id, 2000, None).unwrap_err();
        assert!(matches!(err, ImportError::InvalidAttemptState { .. }));
        assert_eq!(fs::read(&path).unwrap(), initial_bytes);
    }

    #[test]
    fn write_import_metadata_records_a_first_task_id_and_never_replaces_a_durable_one() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let id = "20260408_150001";

        // The queued start path records its task id here for the first time: nothing durable
        // has one yet, so the caller's must survive (dropping it left the row pending forever).
        let mut staged = serde_json::Map::new();
        staged.insert("original_filename".to_owned(), serde_json::json!("a.txt"));
        write_import_metadata(root, id, &staged).unwrap(); // staging leaves import.json with no task id
        assert!(
            read_import_metadata(root, id)
                .unwrap()
                .get("task_id")
                .is_none()
        );
        let mut first = staged.clone();
        first.insert("task_id".to_owned(), serde_json::json!("task-1"));
        write_import_metadata(root, id, &first).unwrap();
        let read = |root: &Path| read_import_metadata(root, id).unwrap();
        assert_eq!(
            read(root).get("task_id"),
            Some(&serde_json::json!("task-1"))
        );

        // Once durable it is authoritative: a stale or different caller cannot change or drop it.
        let mut other = first.clone();
        other.insert("task_id".to_owned(), serde_json::json!("task-2"));
        write_import_metadata(root, id, &other).unwrap();
        assert_eq!(
            read(root).get("task_id"),
            Some(&serde_json::json!("task-1"))
        );
        let mut without = first.clone();
        without.remove("task_id");
        write_import_metadata(root, id, &without).unwrap();
        assert_eq!(
            read(root).get("task_id"),
            Some(&serde_json::json!("task-1"))
        );
    }

    #[test]
    fn write_import_metadata_always_takes_attempt_and_task_id_from_disk() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let id = "20260408_150000";

        // Step 1: gen 1 Completed
        admit_running_attempt(root, id, 1000, None).unwrap();
        record_completed_attempt(root, id, 1, 1500, Some(500), None).unwrap();

        // Caller reads map with Gen 1 Completed
        let mut stale_map = read_import_metadata(root, id).unwrap();

        // Concurrent admission makes gen 2 Running
        admit_running_attempt(root, id, 2000, None).unwrap();

        // Caller attempts to write stale_map with custom metadata field
        stale_map.insert("user_note".to_owned(), serde_json::json!("stale-write"));
        write_import_metadata(root, id, &stale_map).unwrap();

        // Durable attempt MUST remain gen 2 Running, not reverted to gen 1 Completed
        let current_meta = read_import_metadata(root, id).unwrap();
        let facts = get_attempt_facts(&current_meta).unwrap();
        assert_eq!(facts.generation, 2);
        assert_eq!(facts.state, AttemptState::Running);
        assert_eq!(
            current_meta.get("user_note"),
            Some(&serde_json::json!("stale-write"))
        );
    }
}
