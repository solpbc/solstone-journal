// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(any(test, feature = "test-hooks"))]
use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;
#[cfg(any(test, feature = "test-hooks"))]
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{
    AtomicWriteOptions, DetailedAtomicOutcome, LockOptions, PathError, ReadError,
    atomic_replace_detailed, ensure_directory, hold_lock, list_dir_entries, path_lexists,
    read_bytes, read_text, remove_file, resolve_journal_path, write_bytes_exclusive,
};

const GENERATION_PATH: &str = "health/entity-edge-repair/generation";
const JOBS_DIR: &str = "health/entity-edge-repair/jobs";
const COMPLETIONS_DIR: &str = "health/entity-edge-repair/completions";
const PROGRESS_DIR: &str = "health/entity-edge-repair/progress";
const FAILURES_DIR: &str = "health/entity-edge-repair/failures";
const PUBLISH_LOCK: &str = "health/entity-edge-repair/publish.lock";

#[derive(Debug)]
pub enum EntityEdgeRepairError {
    Io(std::io::Error),
    Path(PathError),
    Read(ReadError),
    Store(solstone_core_indexer_store::StoreError),
    Message(String),
}

impl fmt::Display for EntityEdgeRepairError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::Path(error) => error.fmt(formatter),
            Self::Read(error) => error.fmt(formatter),
            Self::Store(error) => error.fmt(formatter),
            Self::Message(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for EntityEdgeRepairError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Path(error) => Some(error),
            Self::Read(error) => Some(error),
            Self::Store(error) => Some(error),
            Self::Message(_) => None,
        }
    }
}

impl From<std::io::Error> for EntityEdgeRepairError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<PathError> for EntityEdgeRepairError {
    fn from(error: PathError) -> Self {
        Self::Path(error)
    }
}

impl From<ReadError> for EntityEdgeRepairError {
    fn from(error: ReadError) -> Self {
        Self::Read(error)
    }
}

impl From<solstone_core_indexer_store::StoreError> for EntityEdgeRepairError {
    fn from(error: solstone_core_indexer_store::StoreError) -> Self {
        Self::Store(error)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityEdgeRepairJob {
    pub operation: String,
    pub merge_id: String,
    pub generation: u64,
    pub enqueued_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EntityEdgeRepairCompletion {
    pub operation: String,
    pub merge_id: String,
    pub generation: u64,
    pub published: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub affected_rows: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rebuilt: Option<bool>,
    pub completed_at: u64,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

pub(crate) fn read_generation(journal: &Path) -> Result<u64, EntityEdgeRepairError> {
    let path = resolve_journal_path(journal, GENERATION_PATH)?;
    let content = read_text(&path, String::new())?;
    if content.trim().is_empty() {
        return Ok(0);
    }
    content.trim().parse::<u64>().map_err(|e| {
        EntityEdgeRepairError::Message(format!("invalid generation value in {path:?}: {e}"))
    })
}

pub(crate) fn bump_generation(journal: &Path) -> Result<u64, EntityEdgeRepairError> {
    let lock_path = resolve_journal_path(journal, PUBLISH_LOCK)?;
    if let Some(parent) = lock_path.parent() {
        ensure_directory(parent).map_err(|e| EntityEdgeRepairError::Message(e.to_string()))?;
    }
    let _lock = hold_lock(&lock_path, LockOptions::default())
        .map_err(|e| EntityEdgeRepairError::Message(e.to_string()))?;

    let current = read_generation(journal)?;
    let next = current + 1;
    let path = resolve_journal_path(journal, GENERATION_PATH)?;
    if let Some(parent) = path.parent() {
        ensure_directory(parent).map_err(|e| EntityEdgeRepairError::Message(e.to_string()))?;
    }
    let content = format!("{next}\n");
    match atomic_replace_detailed(&path, content.as_bytes(), 0o600)
        .map_err(|e| EntityEdgeRepairError::Message(e.to_string()))?
    {
        DetailedAtomicOutcome::Published => Ok(next),
        outcome => Err(EntityEdgeRepairError::Message(format!(
            "generation publication uncertain: {outcome:?}"
        ))),
    }
}

pub(crate) fn job_exists(journal: &Path, operation: &str, merge_id: &str) -> bool {
    let filename = format!("{operation}-{merge_id}.json");
    let relative = format!("{JOBS_DIR}/{filename}");
    if let Ok(path) = resolve_journal_path(journal, &relative) {
        path_lexists(&path).unwrap_or(false)
    } else {
        false
    }
}

pub(crate) fn enqueue_edge_repair_job(
    journal: &Path,
    operation: &str,
    merge_id: &str,
    generation: u64,
) -> Result<(), EntityEdgeRepairError> {
    let path = resolve_journal_path(journal, &format!("{JOBS_DIR}/{operation}-{merge_id}.json"))?;
    if let Some(parent) = path.parent() {
        ensure_directory(parent).map_err(|e| EntityEdgeRepairError::Message(e.to_string()))?;
    }
    let job = EntityEdgeRepairJob {
        operation: operation.to_owned(),
        merge_id: merge_id.to_owned(),
        generation,
        enqueued_at: now_ms(),
    };
    let bytes =
        serde_json::to_vec(&job).map_err(|e| EntityEdgeRepairError::Message(e.to_string()))?;
    match atomic_replace_detailed(&path, &bytes, 0o600)
        .map_err(|e| EntityEdgeRepairError::Message(e.to_string()))?
    {
        DetailedAtomicOutcome::Published => Ok(()),
        outcome => Err(EntityEdgeRepairError::Message(format!(
            "job publication uncertain: {outcome:?}"
        ))),
    }
}

pub fn read_entity_edge_repair_completion(
    journal: &Path,
    operation: &str,
    merge_id: &str,
) -> Result<Option<EntityEdgeRepairCompletion>, EntityEdgeRepairError> {
    let filename = format!("{operation}-{merge_id}.json");
    let relative = format!("{COMPLETIONS_DIR}/{filename}");
    let path = resolve_journal_path(journal, &relative)?;
    if !path_lexists(&path).unwrap_or(false) {
        return Ok(None);
    }
    let bytes = read_bytes(&path, Vec::new())?;
    if bytes.is_empty() {
        return Ok(None);
    }
    let completion = serde_json::from_slice(&bytes)
        .map_err(|e| EntityEdgeRepairError::Message(format!("invalid completion record: {e}")))?;
    Ok(Some(completion))
}

fn write_completion_exclusive(
    journal: &Path,
    completion: &EntityEdgeRepairCompletion,
) -> Result<(), EntityEdgeRepairError> {
    let path = resolve_journal_path(
        journal,
        &format!(
            "{COMPLETIONS_DIR}/{}-{}.json",
            completion.operation, completion.merge_id
        ),
    )?;
    if let Some(parent) = path.parent() {
        ensure_directory(parent).map_err(|e| EntityEdgeRepairError::Message(e.to_string()))?;
    }
    let bytes = serde_json::to_vec(completion)
        .map_err(|e| EntityEdgeRepairError::Message(e.to_string()))?;
    write_bytes_exclusive(&path, &bytes, AtomicWriteOptions { mode: Some(0o600) })
        .map_err(|e| EntityEdgeRepairError::Message(e.to_string()))?;
    Ok(())
}

pub fn attach_edge_repair_completion(journal: &Path, event: &mut serde_json::Value) {
    let Some(object) = event.as_object_mut() else {
        return;
    };
    let kind = object
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let (op, merge_id) = if kind == "merge" {
        let mid = object
            .get("operation")
            .and_then(serde_json::Value::as_object)
            .and_then(|op| op.get("merge_id"))
            .and_then(serde_json::Value::as_str)
            .or_else(|| object.get("merge_id").and_then(serde_json::Value::as_str));
        ("merge", mid)
    } else if kind == "merge_undo" {
        let mid = object
            .get("operation")
            .and_then(serde_json::Value::as_object)
            .and_then(|op| op.get("undo_of"))
            .and_then(serde_json::Value::as_str);
        ("undo", mid)
    } else {
        ("", None)
    };

    let Some(mid) = merge_id else {
        return;
    };

    if let Ok(Some(completion)) = read_entity_edge_repair_completion(journal, op, mid) {
        if let Some(rebuilt) = completion.rebuilt {
            object.insert("rebuilt".to_owned(), serde_json::json!(rebuilt));
        }
        if let Some(rows) = completion.affected_rows {
            object.insert("affected_rows".to_owned(), serde_json::json!(rows));
        }
        if completion.published {
            object.remove("edge_repair_state");
        } else {
            object.insert(
                "edge_repair_state".to_owned(),
                serde_json::json!("superseded"),
            );
        }
        return;
    }

    let filename = format!("{op}-{mid}.json");
    let job_path = resolve_journal_path(journal, &format!("{JOBS_DIR}/{filename}")).ok();
    if job_path.is_some_and(|p| path_lexists(&p).unwrap_or(false)) {
        let fail_path = resolve_journal_path(journal, &format!("{FAILURES_DIR}/{filename}")).ok();
        let prog_path = resolve_journal_path(journal, &format!("{PROGRESS_DIR}/{filename}")).ok();
        let state = if fail_path.is_some_and(|p| path_lexists(&p).unwrap_or(false)) {
            "failed"
        } else if prog_path.is_some_and(|p| path_lexists(&p).unwrap_or(false)) {
            "interrupted"
        } else {
            "pending"
        };
        object.insert("edge_repair_state".to_owned(), serde_json::json!(state));
    }
}

#[cfg(any(test, feature = "test-hooks"))]
static PAUSED_JOBS: Mutex<Option<BTreeSet<String>>> = Mutex::new(None);
#[cfg(any(test, feature = "test-hooks"))]
static EVIDENCE_CUT_ARMED: Mutex<BTreeSet<std::path::PathBuf>> = Mutex::new(BTreeSet::new());
#[cfg(any(test, feature = "test-hooks"))]
static BETWEEN_PUBLISH_CUT_ARMED: Mutex<Option<(std::path::PathBuf, usize)>> = Mutex::new(None);
#[cfg(any(test, feature = "test-hooks"))]
static INJECT_SPAWN_FAILURE: AtomicBool = AtomicBool::new(false);

#[cfg(any(test, feature = "test-hooks"))]
pub fn pause_entity_edge_repair(operation: &str, merge_id: &str) {
    let key = format!("{operation}-{merge_id}");
    let mut guard = PAUSED_JOBS.lock().unwrap();
    if guard.is_none() {
        *guard = Some(BTreeSet::new());
    }
    if let Some(ref mut set) = *guard {
        set.insert(key);
    }
}

#[cfg(any(test, feature = "test-hooks"))]
pub fn release_entity_edge_repair(operation: &str, merge_id: &str) {
    let key = format!("{operation}-{merge_id}");
    let mut guard = PAUSED_JOBS.lock().unwrap();
    if let Some(ref mut set) = *guard {
        set.remove(&key);
    }
}

#[cfg(any(test, feature = "test-hooks"))]
pub fn arm_entity_edge_repair_evidence_cut(journal: &Path) {
    let key = journal
        .canonicalize()
        .unwrap_or_else(|_| journal.to_path_buf());
    EVIDENCE_CUT_ARMED.lock().unwrap().insert(key);
}

#[cfg(any(test, feature = "test-hooks"))]
pub fn disarm_entity_edge_repair_evidence_cut(journal: &Path) {
    let key = journal
        .canonicalize()
        .unwrap_or_else(|_| journal.to_path_buf());
    EVIDENCE_CUT_ARMED.lock().unwrap().remove(&key);
}

#[cfg(any(test, feature = "test-hooks"))]
pub fn arm_entity_edge_repair_between_publish_cut(journal: &Path, after_candidate_idx: usize) {
    let key = journal
        .canonicalize()
        .unwrap_or_else(|_| journal.to_path_buf());
    *BETWEEN_PUBLISH_CUT_ARMED.lock().unwrap() = Some((key, after_candidate_idx));
}

#[cfg(any(test, feature = "test-hooks"))]
pub fn disarm_entity_edge_repair_between_publish_cut() {
    *BETWEEN_PUBLISH_CUT_ARMED.lock().unwrap() = None;
}

#[cfg(any(test, feature = "test-hooks"))]
static INJECT_APPLY_FAILURE: AtomicBool = AtomicBool::new(false);

#[cfg(any(test, feature = "test-hooks"))]
pub fn inject_spawn_failure_once() {
    INJECT_SPAWN_FAILURE.store(true, Ordering::SeqCst);
}

#[cfg(any(test, feature = "test-hooks"))]
pub fn inject_apply_failure_once() {
    INJECT_APPLY_FAILURE.store(true, Ordering::SeqCst);
}

#[cfg(any(test, feature = "test-hooks"))]
pub fn is_edge_repair_driver_active() -> bool {
    #[cfg(test)]
    {
        false
    }
    #[cfg(not(test))]
    {
        DRIVER_ACTIVE.load(Ordering::SeqCst)
    }
}

#[cfg(any(test, feature = "test-hooks"))]
fn is_job_paused(operation: &str, merge_id: &str) -> bool {
    let key = format!("{operation}-{merge_id}");
    let guard = PAUSED_JOBS.lock().unwrap();
    guard.as_ref().is_some_and(|s| s.contains(&key))
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn is_job_paused(_operation: &str, _merge_id: &str) -> bool {
    false
}

#[cfg(any(test, feature = "test-hooks"))]
fn check_evidence_cut(journal: &Path) -> bool {
    let key = journal
        .canonicalize()
        .unwrap_or_else(|_| journal.to_path_buf());
    EVIDENCE_CUT_ARMED.lock().unwrap().remove(&key)
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn check_evidence_cut(_journal: &Path) -> bool {
    false
}

#[cfg(any(test, feature = "test-hooks"))]
fn check_between_publish_cut(journal: &Path, candidate_idx: usize) -> bool {
    let key = journal
        .canonicalize()
        .unwrap_or_else(|_| journal.to_path_buf());
    let mut guard = BETWEEN_PUBLISH_CUT_ARMED.lock().unwrap();
    if let Some((ref armed_journal, target_idx)) = *guard
        && armed_journal == &key
        && candidate_idx == target_idx
    {
        *guard = None;
        return true;
    }
    false
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn check_between_publish_cut(_journal: &Path, _candidate_idx: usize) -> bool {
    false
}

/// Drive entity edge repair jobs.
///
/// Extract and diff run outside the index write lock; each publish is one immediate
/// transaction for one changed path; generation is re-checked under publish.lock;
/// unaffected paths are not rewritten.
pub fn drive_entity_edge_repair(journal: &Path) -> Result<usize, EntityEdgeRepairError> {
    let jobs_dir = resolve_journal_path(journal, JOBS_DIR)?;
    if !path_lexists(&jobs_dir).unwrap_or(false) {
        return Ok(0);
    }
    let entries = list_dir_entries(&jobs_dir)?;
    let mut jobs = Vec::new();
    for entry in entries {
        let name = entry.name.to_string_lossy().to_string();
        if name.ends_with(".json") {
            let bytes = match read_bytes(&entry.path, Vec::new()) {
                Ok(bytes) if !bytes.is_empty() => bytes,
                _ => continue,
            };
            match serde_json::from_slice::<EntityEdgeRepairJob>(&bytes) {
                Ok(job) => jobs.push((name, entry.path, job)),
                Err(_) => {
                    let _ = remove_file(journal, &format!("{JOBS_DIR}/{name}"));
                }
            }
        }
    }
    jobs.sort_by_key(|(_, _, job)| job.generation);

    let mut completed_count = 0;
    for (name, _path, job) in jobs {
        let existing_completion =
            read_entity_edge_repair_completion(journal, &job.operation, &job.merge_id)?;
        if existing_completion.is_some() {
            let _ = remove_file(journal, &format!("{JOBS_DIR}/{name}"));
            let _ = remove_file(journal, &format!("{PROGRESS_DIR}/{name}"));
            let _ = remove_file(journal, &format!("{FAILURES_DIR}/{name}"));
            continue;
        }

        if is_job_paused(&job.operation, &job.merge_id) {
            let prog_path = resolve_journal_path(journal, &format!("{PROGRESS_DIR}/{name}"))?;
            if let Some(parent) = prog_path.parent() {
                let _ = ensure_directory(parent);
            }
            let _ = atomic_replace_detailed(&prog_path, b"{}", 0o600);
            let _ = solstone_core_indexer_store::plan_edge_repair(journal)
                .map_err(EntityEdgeRepairError::Store)?;
            continue;
        }

        let current_gen = read_generation(journal)?;
        if job.generation < current_gen {
            let completion = EntityEdgeRepairCompletion {
                operation: job.operation.clone(),
                merge_id: job.merge_id.clone(),
                generation: job.generation,
                published: false,
                affected_rows: None,
                rebuilt: None,
                completed_at: now_ms(),
            };
            write_completion_exclusive(journal, &completion)?;
            let _ = remove_file(journal, &format!("{JOBS_DIR}/{name}"));
            let _ = remove_file(journal, &format!("{PROGRESS_DIR}/{name}"));
            let _ = remove_file(journal, &format!("{FAILURES_DIR}/{name}"));
            completed_count += 1;
            continue;
        }

        // Write progress marker before plan_edge_repair
        let prog_path = resolve_journal_path(journal, &format!("{PROGRESS_DIR}/{name}"))?;
        if let Some(parent) = prog_path.parent() {
            let _ = ensure_directory(parent);
        }
        let _ = atomic_replace_detailed(&prog_path, b"{}", 0o600);

        let plan = match solstone_core_indexer_store::plan_edge_repair(journal) {
            Ok(plan) => plan,
            Err(err) => {
                let fail_path = resolve_journal_path(journal, &format!("{FAILURES_DIR}/{name}"))?;
                if let Some(parent) = fail_path.parent() {
                    let _ = ensure_directory(parent);
                }
                let _ = atomic_replace_detailed(&fail_path, err.to_string().as_bytes(), 0o600);
                return Err(EntityEdgeRepairError::Store(err));
            }
        };

        if is_job_paused(&job.operation, &job.merge_id) {
            continue;
        }

        let current_gen = read_generation(journal)?;
        if job.generation < current_gen {
            let completion = EntityEdgeRepairCompletion {
                operation: job.operation.clone(),
                merge_id: job.merge_id.clone(),
                generation: job.generation,
                published: false,
                affected_rows: None,
                rebuilt: None,
                completed_at: now_ms(),
            };
            write_completion_exclusive(journal, &completion)?;
            let _ = remove_file(journal, &format!("{JOBS_DIR}/{name}"));
            let _ = remove_file(journal, &format!("{PROGRESS_DIR}/{name}"));
            let _ = remove_file(journal, &format!("{FAILURES_DIR}/{name}"));
            completed_count += 1;
            continue;
        }

        let lock_path = resolve_journal_path(journal, PUBLISH_LOCK)?;
        if let Some(parent) = lock_path.parent() {
            ensure_directory(parent).map_err(|e| EntityEdgeRepairError::Message(e.to_string()))?;
        }

        let mut published_any = false;
        let mut total_affected = 0;
        let mut superseded = false;
        let target_generation = job.generation;

        for (idx, candidate) in plan.iter().enumerate() {
            let lock_guard = hold_lock(&lock_path, LockOptions::default())
                .map_err(|e| EntityEdgeRepairError::Message(e.to_string()))?;

            #[cfg(any(test, feature = "test-hooks"))]
            let injected_err = INJECT_APPLY_FAILURE.swap(false, Ordering::SeqCst);
            #[cfg(not(any(test, feature = "test-hooks")))]
            let injected_err = false;

            let outcome = if injected_err {
                Err(solstone_core_indexer_store::StoreError::Io(
                    std::io::Error::other("injected store apply failure"),
                ))
            } else {
                solstone_core_indexer_store::apply_edge_repair_candidate(journal, candidate, || {
                    let gen_now = read_generation(journal).map_err(|e| {
                        solstone_core_indexer_store::StoreError::Io(std::io::Error::other(
                            e.to_string(),
                        ))
                    })?;
                    Ok(gen_now == target_generation)
                })
            };

            drop(lock_guard);

            match outcome {
                Ok(solstone_core_indexer_store::CandidatePublishOutcome::Republished {
                    inserted,
                    deleted,
                }) => {
                    published_any = true;
                    total_affected += inserted + deleted;
                    if check_between_publish_cut(journal, idx) {
                        return Err(EntityEdgeRepairError::Message(
                            "between-publish cut triggered".to_string(),
                        ));
                    }
                }
                Ok(solstone_core_indexer_store::CandidatePublishOutcome::Deleted { deleted }) => {
                    published_any = true;
                    total_affected += deleted;
                    if check_between_publish_cut(journal, idx) {
                        return Err(EntityEdgeRepairError::Message(
                            "between-publish cut triggered".to_string(),
                        ));
                    }
                }
                Ok(solstone_core_indexer_store::CandidatePublishOutcome::Unchanged) => {}
                Ok(solstone_core_indexer_store::CandidatePublishOutcome::Superseded) => {
                    superseded = true;
                    break;
                }
                Err(err) => {
                    let fail_path =
                        resolve_journal_path(journal, &format!("{FAILURES_DIR}/{name}"))?;
                    if let Some(parent) = fail_path.parent() {
                        let _ = ensure_directory(parent);
                    }
                    let err_str = err.to_string();
                    let _ = atomic_replace_detailed(&fail_path, err_str.as_bytes(), 0o600);
                    return Err(EntityEdgeRepairError::Store(err));
                }
            }
        }

        if superseded {
            if published_any {
                let completion = EntityEdgeRepairCompletion {
                    operation: job.operation.clone(),
                    merge_id: job.merge_id.clone(),
                    generation: job.generation,
                    published: false,
                    affected_rows: None,
                    rebuilt: Some(false),
                    completed_at: now_ms(),
                };
                write_completion_exclusive(journal, &completion)?;
            } else {
                let completion = EntityEdgeRepairCompletion {
                    operation: job.operation.clone(),
                    merge_id: job.merge_id.clone(),
                    generation: job.generation,
                    published: false,
                    affected_rows: None,
                    rebuilt: None,
                    completed_at: now_ms(),
                };
                write_completion_exclusive(journal, &completion)?;
            }
            let _ = remove_file(journal, &format!("{JOBS_DIR}/{name}"));
            let _ = remove_file(journal, &format!("{PROGRESS_DIR}/{name}"));
            let _ = remove_file(journal, &format!("{FAILURES_DIR}/{name}"));
            completed_count += 1;
        } else {
            let lock_guard = hold_lock(&lock_path, LockOptions::default())
                .map_err(|e| EntityEdgeRepairError::Message(e.to_string()))?;
            let gen_now = read_generation(journal)?;
            if gen_now != target_generation {
                drop(lock_guard);
                if published_any {
                    let completion = EntityEdgeRepairCompletion {
                        operation: job.operation.clone(),
                        merge_id: job.merge_id.clone(),
                        generation: job.generation,
                        published: false,
                        affected_rows: None,
                        rebuilt: Some(false),
                        completed_at: now_ms(),
                    };
                    write_completion_exclusive(journal, &completion)?;
                } else {
                    let completion = EntityEdgeRepairCompletion {
                        operation: job.operation.clone(),
                        merge_id: job.merge_id.clone(),
                        generation: job.generation,
                        published: false,
                        affected_rows: None,
                        rebuilt: None,
                        completed_at: now_ms(),
                    };
                    write_completion_exclusive(journal, &completion)?;
                }
                let _ = remove_file(journal, &format!("{JOBS_DIR}/{name}"));
                let _ = remove_file(journal, &format!("{PROGRESS_DIR}/{name}"));
                let _ = remove_file(journal, &format!("{FAILURES_DIR}/{name}"));
                completed_count += 1;
            } else {
                let _ = remove_file(journal, "awareness/discovery_clusters.json");
                drop(lock_guard);

                if check_evidence_cut(journal) {
                    return Err(EntityEdgeRepairError::Message(
                        "evidence cut triggered".to_string(),
                    ));
                }

                let completion = EntityEdgeRepairCompletion {
                    operation: job.operation.clone(),
                    merge_id: job.merge_id.clone(),
                    generation: job.generation,
                    published: true,
                    affected_rows: Some(total_affected),
                    rebuilt: Some(false),
                    completed_at: now_ms(),
                };
                write_completion_exclusive(journal, &completion)?;
                let _ = remove_file(journal, &format!("{JOBS_DIR}/{name}"));
                let _ = remove_file(journal, &format!("{PROGRESS_DIR}/{name}"));
                let _ = remove_file(journal, &format!("{FAILURES_DIR}/{name}"));
                completed_count += 1;
            }
        }
    }

    Ok(completed_count)
}

#[cfg(not(test))]
static DRIVER_ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn spawn_entity_edge_repair(journal: &Path) {
    #[cfg(test)]
    {
        let _ = journal;
    }
    #[cfg(not(test))]
    {
        #[cfg(feature = "test-hooks")]
        if INJECT_SPAWN_FAILURE.swap(false, Ordering::SeqCst) {
            return;
        }

        if DRIVER_ACTIVE
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        let journal_path = journal.to_path_buf();
        let spawn_res = std::thread::Builder::new()
            .name("entity-edge-repair".to_string())
            .spawn(move || {
                struct DriverGuard;
                impl Drop for DriverGuard {
                    fn drop(&mut self) {
                        DRIVER_ACTIVE.store(false, Ordering::SeqCst);
                    }
                }
                let _guard = DriverGuard;
                loop {
                    match drive_entity_edge_repair(&journal_path) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                }
            });
        if spawn_res.is_err() {
            DRIVER_ACTIVE.store(false, Ordering::SeqCst);
        }
    }
}
