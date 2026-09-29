// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Open import metadata records.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use solstone_core_journal_io::{
    AtomicWriteOptions, FileLock, LockOptions, atomic_replace, path_lexists,
};

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

/// The only failure text an attempt records for an import that definitively failed. The
/// cause stays with the operator (the producer's returned error, the CLI, the log): an
/// attempt's reason reaches the owner, so it is never a raw diagnostic.
pub const IMPORT_FAILED_REASON: &str = "import failed";

/// The only reason an attempt records when its outcome could not be established.
pub const IMPORT_UNCONFIRMED_REASON: &str = "this import couldn't be confirmed as finished.";

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

/// One entity set aside during a journal-archive merge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagedEntityRecord {
    pub source_id: String,
    pub source_name: String,
    pub staging_path: String,
}

/// The set of entities set aside during a specific attempt of a journal-archive merge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagedEntityList {
    pub attempt_id: String,
    pub entities: Vec<StagedEntityRecord>,
}

/// Build the metadata map recording a journal archive merge outcome.
#[allow(clippy::too_many_arguments)]
pub fn journal_archive_result_metadata(
    entries_written: usize,
    entities_seeded: usize,
    merge_summary: Value,
    principal_collision: Option<Value>,
    merge_log_path: String,
    merge_staging_path: String,
    errors: &[String],
    staged_entities: Option<StagedEntityList>,
) -> serde_json::Map<String, Value> {
    serde_json::Map::from_iter([
        (
            "source_type".to_owned(),
            serde_json::json!("journal_archive"),
        ),
        (
            "entries_written".to_owned(),
            serde_json::json!(entries_written),
        ),
        (
            "entities_seeded".to_owned(),
            serde_json::json!(entities_seeded),
        ),
        ("merge_summary".to_owned(), merge_summary),
        (
            "principal_collision".to_owned(),
            principal_collision.unwrap_or(Value::Null),
        ),
        (
            "merge_log_path".to_owned(),
            serde_json::json!(merge_log_path),
        ),
        (
            "merge_staging_path".to_owned(),
            serde_json::json!(merge_staging_path),
        ),
        (
            "summary_errors".to_owned(),
            if errors.is_empty() {
                Value::Null
            } else {
                serde_json::json!(errors)
            },
        ),
        (
            "staged_entities".to_owned(),
            staged_entities
                .filter(|list| !list.entities.is_empty())
                .and_then(|list| serde_json::to_value(list).ok())
                .unwrap_or(Value::Null),
        ),
    ])
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

/// The wall-clock bound after which a `Running` attempt is no longer treated as live, when
/// nothing says whether its producer is: a record written before attempts were held, or a
/// queued start whose importer has not admitted its attempt yet. Shared by the live-attempt
/// refusal and the projection so the two can never disagree about whether an attempt is alive.
pub const RUNNING_ATTEMPT_BOUND_MS: u64 = 3_600_000;

/// Whether the process that admitted a `Running` attempt still runs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptHolder {
    /// A process holds the attempt: it is running, however long it takes.
    Held,
    /// The attempt was held and nothing holds it now: its producer is gone.
    Released,
    /// The record predates held attempts, or the lock could not be read.
    Unknown,
}

/// Attempt locks this process holds, keyed by lock path. A producer admits an attempt
/// holding its lock and keeps it until it records the attempt's end; the kernel releases it
/// if the process dies first, which is how a reader tells an orphaned attempt from a slow one.
static HELD_ATTEMPTS: LazyLock<Mutex<HashMap<PathBuf, FileLock>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn attempt_lock_path(journal_root: &Path, import_id: &str) -> Result<PathBuf, ImportError> {
    Ok(crate::staging::import_directory(journal_root, import_id)?.join(".attempt"))
}

fn hold_attempt(journal_root: &Path, import_id: &str) -> Result<(), ImportError> {
    let path = attempt_lock_path(journal_root, import_id)?;
    let mut held = HELD_ATTEMPTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if held.contains_key(&path) {
        return Ok(());
    }
    let options = LockOptions {
        timeout: Duration::from_secs(2),
        mode: Some(0o600),
        ..LockOptions::default()
    };
    let lock = solstone_core_journal_io::hold_lock(&path, options).map_err(|err| {
        ImportError::LockFailed {
            path: path.clone(),
            message: err.to_string(),
        }
    })?;
    held.insert(path, lock);
    Ok(())
}

/// Let go of an attempt this process holds. A no-op for one it does not hold.
pub fn release_attempt(journal_root: &Path, import_id: &str) {
    if let Ok(path) = attempt_lock_path(journal_root, import_id) {
        HELD_ATTEMPTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&path);
    }
}

/// Read whether a `Running` attempt's producer still holds it. Read-only: a record with no
/// attempt lock beside it is `Unknown`, never created.
#[must_use]
pub fn attempt_holder(journal_root: &Path, import_id: &str) -> AttemptHolder {
    let Ok(path) = attempt_lock_path(journal_root, import_id) else {
        return AttemptHolder::Unknown;
    };
    let mut sidecar = path.clone().into_os_string();
    sidecar.push(".lock");
    if !path_lexists(Path::new(&sidecar)).unwrap_or(false) {
        return AttemptHolder::Unknown;
    }
    match solstone_core_journal_io::lock_is_held(&path) {
        Ok(true) => AttemptHolder::Held,
        Ok(false) => AttemptHolder::Released,
        Err(_) => AttemptHolder::Unknown,
    }
}

fn running_attempt_is_live(journal_root: &Path, import_id: &str, started_at_ms: u64) -> bool {
    match attempt_holder(journal_root, import_id) {
        AttemptHolder::Held => true,
        AttemptHolder::Released => false,
        AttemptHolder::Unknown => {
            now_ms().saturating_sub(started_at_ms) <= RUNNING_ATTEMPT_BOUND_MS
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Refuse a second producer while a live `Running` attempt holds this import.
///
/// Returns the refusal sentence when one should be shown, or `None` to proceed. It takes the
/// source's display name rather than a `RegistrySource`, because the generic audio and text
/// producers have no registry variant to pass; each caller wraps the sentence in its own
/// failure shape.
///
/// This is advisory and deliberately lock-free: it reads provenance outside the import lock,
/// so two children can both pass it. The generation guard in the terminal write is the real
/// serialization; this only turns the common double-start into a clear message.
#[must_use]
pub fn refuse_if_live_running(
    journal_root: &Path,
    import_id: &str,
    source_name: &str,
) -> Option<String> {
    let Ok(Some(metadata)) = read_provenance(journal_root, import_id) else {
        return None;
    };
    let facts = get_attempt_facts(&metadata)?;
    if facts.state != AttemptState::Running
        || !running_attempt_is_live(journal_root, import_id, facts.started_at_ms)
    {
        return None;
    }
    Some(format!(
        "{source_name} import failed: another import of this file is already running"
    ))
}

/// Admit an in-flight attempt in import.json under the import lock before any source or chronicle mutation.
///
/// This process holds the attempt from here until it records the attempt's end; see
/// [`attempt_holder`]. A producer that ends without recording one calls [`release_attempt`].
pub fn admit_running_attempt(
    journal_root: &Path,
    import_id: &str,
    started_at_ms: u64,
    source_hint: Option<&str>,
) -> Result<AttemptFacts, ImportError> {
    hold_attempt(journal_root, import_id)?;
    let admitted = admit_held_attempt(journal_root, import_id, started_at_ms, source_hint);
    if admitted.is_err() {
        release_attempt(journal_root, import_id);
    }
    admitted
}

/// Take hold of a `Running` attempt another invocation admitted, to carry on with it.
pub fn resume_running_attempt(journal_root: &Path, import_id: &str) -> Result<(), ImportError> {
    hold_attempt(journal_root, import_id)
}

fn admit_held_attempt(
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
    release_attempt(journal_root, import_id);
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
    release_attempt(journal_root, import_id);
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

/// Settle an import whose importer process has exited while its record still reads running.
///
/// Nothing else will ever finish such a record: the process that would have written its
/// outcome is gone, and without this the owner sees "running" until the wall-clock bound. A
/// non-zero exit settles it as failed; a clean exit that left no outcome settles it as
/// unconfirmed. A record that reads anything other than running is left alone, so an importer
/// that recorded its own outcome is never second-guessed. Returns whether the record changed.
pub fn settle_exited_import(
    journal_root: &Path,
    import_id: &str,
    exit_code: i32,
    finished_at_ms: u64,
) -> Result<bool, ImportError> {
    let _lock = hold_import_lock(journal_root, import_id)?;
    // The record as written, not whether its producer still holds it: the importer has just
    // exited, so it never does, and its exit code is the better account of how it ended.
    if crate::projection::project_recorded_import(journal_root, import_id).status
        != crate::projection::ProjectionStatus::Running
    {
        return Ok(false);
    }
    let failure_reason = (exit_code != 0).then(|| IMPORT_FAILED_REASON.to_owned());
    let mut metadata = read_import_metadata(journal_root, import_id)?;
    let previous_generation = match read_attempt_facts(&metadata) {
        AttemptRead::Present(facts) if facts.state == AttemptState::Running => {
            record_unconfirmed_attempt_unlocked(
                journal_root,
                import_id,
                facts.generation,
                finished_at_ms,
                failure_reason,
            )?;
            return Ok(true);
        }
        // A retry whose importer exited before it admitted its own attempt: the record
        // reads running because the queued start is newer than the attempt it holds.
        AttemptRead::Present(facts) => facts.generation,
        AttemptRead::Absent => 0,
        AttemptRead::Malformed => return Ok(false),
    };
    // The importer exited before it admitted an attempt (a refused argument, a failed spawn),
    // so the queued start's task id is the only clock the record has.
    let started_at_ms = queued_task_ms(&metadata).unwrap_or(finished_at_ms);
    let generation = previous_generation.saturating_add(1);
    let facts = AttemptFacts {
        attempt_id: format!("{import_id}:{generation}"),
        generation,
        state: AttemptState::Unconfirmed,
        started_at_ms,
        finished_at_ms: Some(finished_at_ms),
        duration_ms: Some(finished_at_ms.saturating_sub(started_at_ms)),
        failure_reason,
        unavailable_description: None,
        input_failures: None,
    };
    let value = serde_json::to_value(&facts).map_err(|err| ImportError::MetadataWriteFailed {
        path: import_metadata_path(journal_root, import_id)
            .unwrap_or_else(|_| PathBuf::from(import_id)),
        message: err.to_string(),
    })?;
    metadata.insert("attempt".to_owned(), value);
    write_import_metadata_unlocked(journal_root, import_id, &metadata)?;
    Ok(true)
}

/// The queue time of a start sent to the supervisor, which is its task id. An attempt's
/// admission replaces the task id with the import id, which is not a number.
#[must_use]
pub fn queued_task_ms(metadata: &ImportMetadata) -> Option<u64> {
    metadata
        .get("task_id")
        .and_then(Value::as_str)
        .and_then(|task_id| task_id.parse().ok())
}

/// Record the task id a queued start was sent under.
///
/// Unlike a whole-record write, which keeps the durable task id, this replaces the id an
/// earlier attempt left: a retry of a settled import is queued work its attempt block does not
/// describe yet, and the record reads running from here rather than keeping the old outcome
/// until the importer admits its own attempt.
pub fn record_queued_task(
    journal_root: &Path,
    import_id: &str,
    task_id: &str,
) -> Result<PathBuf, ImportError> {
    let _lock = hold_import_lock(journal_root, import_id)?;
    let mut metadata = read_import_metadata(journal_root, import_id)?;
    metadata.insert("task_id".to_owned(), serde_json::json!(task_id));
    write_import_metadata_unlocked(journal_root, import_id, &metadata)
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

/// Record what an import produced (counts, summaries, warnings) into import.json,
/// replacing earlier values of the same keys; `Null` removes a key (caller holds the lock).
pub fn record_import_results_unlocked(
    journal_root: &Path,
    import_id: &str,
    results: serde_json::Map<String, Value>,
) -> Result<(), ImportError> {
    let mut metadata = read_import_metadata(journal_root, import_id)?;
    for (key, value) in results {
        if value.is_null() {
            metadata.remove(&key);
        } else {
            metadata.insert(key, value);
        }
    }
    write_import_metadata_unlocked(journal_root, import_id, &metadata).map(|_| ())
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
            if let Some(tid) = existing.get("task_id").filter(|tid| !tid.is_null()) {
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

    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    fn queued_start(root: &Path, id: &str) -> u64 {
        let started = now_ms();
        let metadata = ImportMetadata::from_iter([
            ("upload_timestamp".to_owned(), serde_json::json!(started)),
            ("task_id".to_owned(), serde_json::json!(started.to_string())),
        ]);
        write_import_metadata(root, id, &metadata).unwrap();
        started
    }

    fn status(root: &Path, id: &str) -> crate::ProjectionStatus {
        crate::project_import_result(root, id).status
    }

    #[test]
    fn an_importer_that_exits_before_admitting_reads_failed_at_once() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let id = "20260927_090000";
        let started = queued_start(root, id);
        assert_eq!(status(root, id), crate::ProjectionStatus::Running);

        assert!(settle_exited_import(root, id, 1, started + 40).unwrap());

        assert_eq!(status(root, id), crate::ProjectionStatus::Failed);
        let facts = get_attempt_facts(&read_import_metadata(root, id).unwrap()).unwrap();
        assert_eq!(facts.generation, 1);
        assert_eq!(facts.started_at_ms, started);
        assert_eq!(facts.duration_ms, Some(40));
        // A retry admits the next generation rather than colliding with the settled one.
        assert_eq!(
            admit_running_attempt(root, id, now_ms(), None)
                .unwrap()
                .generation,
            2
        );
    }

    #[test]
    fn an_importer_that_dies_mid_attempt_reads_failed_or_unconfirmed_by_exit() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        for (id, exit_code, expected) in [
            ("20260927_091000", 101, crate::ProjectionStatus::Failed),
            ("20260927_092000", 0, crate::ProjectionStatus::Unconfirmed),
        ] {
            queued_start(root, id);
            admit_running_attempt(root, id, now_ms(), None).unwrap();
            assert_eq!(status(root, id), crate::ProjectionStatus::Running);

            assert!(settle_exited_import(root, id, exit_code, now_ms()).unwrap());
            assert_eq!(status(root, id), expected, "exit {exit_code}");
        }
    }

    #[test]
    fn an_importer_that_recorded_its_own_outcome_is_left_alone() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let id = "20260927_093000";
        queued_start(root, id);
        let facts = admit_running_attempt(root, id, now_ms(), None).unwrap();
        record_unconfirmed_attempt(
            root,
            id,
            facts.generation,
            now_ms(),
            Some(IMPORT_FAILED_REASON.to_owned()),
        )
        .unwrap();
        let before = fs::read(import_metadata_path(root, id).unwrap()).unwrap();

        assert!(!settle_exited_import(root, id, 0, now_ms()).unwrap());
        assert_eq!(
            fs::read(import_metadata_path(root, id).unwrap()).unwrap(),
            before
        );
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
