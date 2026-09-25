// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Process-local coordinator for durable background speaker backfill operations.
//!
//! Provides non-blocking execution locking, owner token generation tracking,
//! and resumable lifecycle operations.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use chrono::Utc;
use solstone_core_journal_io::{
    JournalRoot, JournalRootError, LockError, LockOptions, ObjectIdentity, hold_lock,
};
use thiserror::Error;

use crate::backfill::{BackfillError, execute_backfill_member, plan_backfill_segments};
use crate::backfill_operations::{
    BACKFILL_OPERATION_SCHEMA_VERSION, BackfillCheckpointOutcome, BackfillCheckpointResult,
    BackfillFailureStage, BackfillOperationError, BackfillOperationEvent, BackfillOperationPayload,
    BackfillOperationState, BackfillOperationStatus, append_backfill_event,
    backfill_operation_status, backfill_operations_path, fold_backfill_operation,
    load_backfill_operations, truncate_torn_tail_if_present,
};
use crate::segment_catalog::resolve_exact;

static ACTIVE_TOKENS: LazyLock<Mutex<HashMap<(ObjectIdentity, String), u64>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

#[cfg(test)]
#[derive(Default)]
pub struct BackfillTestHooks {
    pub inject_scan_failure: Option<String>,
    pub inject_executor_failure: Option<String>,
    pub inject_panic: bool,
}

#[cfg(test)]
pub static TEST_HOOKS: LazyLock<Mutex<BackfillTestHooks>> =
    LazyLock::new(|| Mutex::new(BackfillTestHooks::default()));

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartBackfillRequest {
    pub operation_id: Option<String>,
    pub commit: bool,
    pub reattribute: bool,
    pub accumulation: bool,
    pub now_ms: i64,
}

#[derive(Debug, Error)]
pub enum BackfillCoordinatorError {
    #[error("journal root admission failed: {0}")]
    JournalRoot(#[from] JournalRootError),
    #[error("ledger operation failed: {0}")]
    Ledger(#[from] BackfillOperationError),
    #[error("invalid backfill flags: {0}")]
    InvalidFlags(String),
    #[error(
        "immutable backfill flags mismatch for operation {operation_id}: expected commit={expected_commit} reattribute={expected_reattribute} accumulation={expected_accumulation}, got commit={got_commit} reattribute={got_reattribute} accumulation={got_accumulation}"
    )]
    ImmutableFlagsMismatch {
        operation_id: String,
        expected_commit: bool,
        expected_reattribute: bool,
        expected_accumulation: bool,
        got_commit: bool,
        got_reattribute: bool,
        got_accumulation: bool,
    },
    #[error("backfill operation not found: {0}")]
    NotFound(String),
    #[error("backfill execution lock failed: {0}")]
    Lock(#[from] LockError),
    #[error("backfill scan failed: {0}")]
    Backfill(#[from] BackfillError),
}

/// Mint a new operation ID formatted as `bfop-` + 32 lowercase hex chars.
#[must_use]
pub fn mint_operation_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("getrandom must succeed");
    let mut hex = String::with_capacity(5 + 32);
    hex.push_str("bfop-");
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(hex, "{:02x}", b);
    }
    hex
}

/// Query status for an operation without modifying it.
pub fn backfill_status(
    journal_root: &Path,
    operation_id: &str,
) -> Result<Option<BackfillOperationStatus>, BackfillCoordinatorError> {
    let admitted = JournalRoot::open(journal_root)?;
    let ledger_path = backfill_operations_path(admitted.canonical_path());
    if !ledger_path.exists() {
        return Ok(None);
    }
    let rows = load_backfill_operations(&ledger_path)?;
    let active = {
        let registry = ACTIVE_TOKENS.lock().unwrap();
        registry.contains_key(&(admitted.identity(), operation_id.to_owned()))
    };
    Ok(backfill_operation_status(&rows, operation_id, active)?)
}

/// Start a new or existing backfill operation.
pub fn start_backfill(
    journal_root: &Path,
    request: &StartBackfillRequest,
) -> Result<BackfillOperationStatus, BackfillCoordinatorError> {
    if request.accumulation && !request.commit {
        return Err(BackfillCoordinatorError::InvalidFlags(
            "accumulation requires commit:true".to_owned(),
        ));
    }

    let admitted = JournalRoot::open(journal_root)?;
    let root_path = admitted.canonical_path().to_path_buf();
    let root_identity = admitted.identity();
    let ledger_path = backfill_operations_path(&root_path);
    if let Some(parent) = ledger_path.parent() {
        fs::create_dir_all(parent).map_err(|source| BackfillOperationError::CreateDirectory {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let operation_id = request
        .operation_id
        .clone()
        .unwrap_or_else(mint_operation_id);

    {
        let _ledger_lock = hold_lock(&ledger_path, LockOptions::default())?;
        truncate_torn_tail_if_present(&ledger_path)?;
        let rows = load_backfill_operations(&ledger_path)?;
        let existing_state = fold_backfill_operation(&rows, &operation_id)?;

        if let Some(state) = existing_state {
            if state.commit != request.commit
                || state.reattribute != request.reattribute
                || state.accumulation != request.accumulation
            {
                return Err(BackfillCoordinatorError::ImmutableFlagsMismatch {
                    operation_id: operation_id.clone(),
                    expected_commit: state.commit,
                    expected_reattribute: state.reattribute,
                    expected_accumulation: state.accumulation,
                    got_commit: request.commit,
                    got_reattribute: request.reattribute,
                    got_accumulation: request.accumulation,
                });
            }
            if state.completed
                && state.pending_segments.is_empty()
                && state.error_details.is_empty()
            {
                return Ok(backfill_operation_status(&rows, &operation_id, false)?
                    .expect("folded state exists"));
            }
        } else {
            let event = BackfillOperationEvent {
                schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
                event_id: format!("{operation_id}:accepted"),
                operation_id: operation_id.clone(),
                ts: Utc::now().to_rfc3339(),
                payload: BackfillOperationPayload::Accepted {
                    commit: request.commit,
                    reattribute: request.reattribute,
                    accumulation: request.accumulation,
                },
            };
            crate::backfill_operations::append_backfill_event_locked(&ledger_path, &event)?;
        }
    }

    // Register active token and spawn if not currently active
    let (token_id, needs_spawn) = {
        let mut registry = ACTIVE_TOKENS.lock().unwrap();
        let key = (root_identity, operation_id.clone());
        if let Some(token) = registry.get(&key) {
            (*token, false)
        } else {
            let token = NEXT_TOKEN.fetch_add(1, Ordering::SeqCst);
            registry.insert(key, token);
            (token, true)
        }
    };

    if needs_spawn {
        let op_id = operation_id.clone();
        let now_ms = request.now_ms;
        std::thread::spawn(move || {
            run_backfill_attempt(root_path, root_identity, op_id, token_id, now_ms);
        });
    }

    let rows_now = load_backfill_operations(&ledger_path)?;
    Ok(backfill_operation_status(&rows_now, &operation_id, true)?.expect("accepted state exists"))
}

/// Resume an interrupted or failed backfill operation.
pub fn resume_backfill(
    journal_root: &Path,
    operation_id: &str,
    now_ms: i64,
) -> Result<BackfillOperationStatus, BackfillCoordinatorError> {
    let admitted = JournalRoot::open(journal_root)?;
    let root_path = admitted.canonical_path().to_path_buf();
    let root_identity = admitted.identity();
    let ledger_path = backfill_operations_path(&root_path);

    if !ledger_path.exists() {
        return Err(BackfillCoordinatorError::NotFound(operation_id.to_owned()));
    }

    {
        let _ledger_lock = hold_lock(&ledger_path, LockOptions::default())?;
        truncate_torn_tail_if_present(&ledger_path)?;
        let rows = load_backfill_operations(&ledger_path)?;
        let existing_state = fold_backfill_operation(&rows, operation_id)?;

        let Some(state) = existing_state else {
            return Err(BackfillCoordinatorError::NotFound(operation_id.to_owned()));
        };

        if state.completed && state.pending_segments.is_empty() && state.error_details.is_empty() {
            return Ok(backfill_operation_status(&rows, operation_id, false)?
                .expect("folded state exists"));
        }
    }

    let (token_id, needs_spawn) = {
        let mut registry = ACTIVE_TOKENS.lock().unwrap();
        let key = (root_identity, operation_id.to_owned());
        if let Some(token) = registry.get(&key) {
            (*token, false)
        } else {
            let token = NEXT_TOKEN.fetch_add(1, Ordering::SeqCst);
            registry.insert(key, token);
            (token, true)
        }
    };

    if needs_spawn {
        let op_id = operation_id.to_owned();
        std::thread::spawn(move || {
            run_backfill_attempt(root_path, root_identity, op_id, token_id, now_ms);
        });
    }

    let rows_now = load_backfill_operations(&ledger_path)?;
    Ok(backfill_operation_status(&rows_now, operation_id, true)?.expect("resumed state exists"))
}

fn clear_active_token(identity: ObjectIdentity, operation_id: &str, token_id: u64) {
    let mut registry = ACTIVE_TOKENS.lock().unwrap();
    let key = (identity, operation_id.to_owned());
    if registry.get(&key) == Some(&token_id) {
        registry.remove(&key);
    }
}

fn run_backfill_attempt(
    journal_root: PathBuf,
    identity: ObjectIdentity,
    operation_id: String,
    token_id: u64,
    now_ms: i64,
) {
    let lock_target = journal_root.join("health/locks/speaker-backfill");
    let execution_lock = match hold_lock(
        &lock_target,
        LockOptions {
            timeout: Duration::ZERO,
            ..Default::default()
        },
    ) {
        Ok(lock) => lock,
        Err(_) => {
            // Lock contended: silently exit and clear ONLY this token
            clear_active_token(identity, &operation_id, token_id);
            return;
        }
    };

    let ledger_path = backfill_operations_path(&journal_root);

    // Initial load & fold
    let state = match load_and_fold(&ledger_path, &operation_id) {
        Ok(Some(state)) => state,
        _ => {
            clear_active_token(identity, &operation_id, token_id);
            return;
        }
    };

    if state.completed && state.pending_segments.is_empty() && state.error_details.is_empty() {
        clear_active_token(identity, &operation_id, token_id);
        return;
    }

    let generation = state.latest_generation.unwrap_or(0) + 1;

    // Record attempt started
    if let Err(err) = append_backfill_event(
        &ledger_path,
        &BackfillOperationEvent {
            schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
            event_id: format!("{operation_id}:attempt-{generation}:start"),
            operation_id: operation_id.clone(),
            ts: Utc::now().to_rfc3339(),
            payload: BackfillOperationPayload::AttemptStarted { generation },
        },
    ) {
        let _ = append_backfill_event(
            &ledger_path,
            &BackfillOperationEvent {
                schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
                event_id: format!("{operation_id}:fail-{generation}"),
                operation_id: operation_id.clone(),
                ts: Utc::now().to_rfc3339(),
                payload: BackfillOperationPayload::AttemptFailed {
                    generation,
                    stage: BackfillFailureStage::Executor,
                    detail: err.to_string(),
                },
            },
        );
        clear_active_token(identity, &operation_id, token_id);
        return;
    }

    let attempt_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        #[cfg(test)]
        let inject_panic = {
            let hooks = TEST_HOOKS.lock().unwrap_or_else(|e| e.into_inner());
            hooks.inject_panic
        };
        #[cfg(test)]
        if inject_panic {
            panic!("injected test panic");
        }

        #[cfg(test)]
        let inject_scan = {
            let hooks = TEST_HOOKS.lock().unwrap_or_else(|e| e.into_inner());
            hooks.inject_scan_failure.clone()
        };
        #[cfg(test)]
        if let Some(detail) = inject_scan {
            let _ = append_backfill_event(
                &ledger_path,
                &BackfillOperationEvent {
                    schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
                    event_id: format!("{operation_id}:fail-{generation}"),
                    operation_id: operation_id.clone(),
                    ts: Utc::now().to_rfc3339(),
                    payload: BackfillOperationPayload::AttemptFailed {
                        generation,
                        stage: BackfillFailureStage::Scan,
                        detail,
                    },
                },
            );
            return;
        }

        // Plan if prepared row is missing
        let (planned_segments, _total_scanned, _selected, _protected_skipped) =
            if state.planned_segments.is_empty() {
                match plan_backfill_segments(&journal_root, state.reattribute) {
                    Ok(plan) => {
                        let now = Utc::now().to_rfc3339();
                        let append_res = append_backfill_event(
                            &ledger_path,
                            &BackfillOperationEvent {
                                schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
                                event_id: format!("{operation_id}:attempt-{generation}:prep"),
                                operation_id: operation_id.clone(),
                                ts: now.clone(),
                                payload: BackfillOperationPayload::Prepared {
                                    started_at: now,
                                    reattribute: state.reattribute,
                                    total_count: plan.to_process.len(),
                                    segments: plan.to_process.clone(),
                                    total_scanned: plan.total_scanned,
                                    selected: plan.selected,
                                    protected_skipped: plan.protected_skipped,
                                },
                            },
                        );
                        if let Err(err) = append_res {
                            let _ = append_backfill_event(
                                &ledger_path,
                                &BackfillOperationEvent {
                                    schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
                                    event_id: format!("{operation_id}:fail-{generation}"),
                                    operation_id: operation_id.clone(),
                                    ts: Utc::now().to_rfc3339(),
                                    payload: BackfillOperationPayload::AttemptFailed {
                                        generation,
                                        stage: BackfillFailureStage::Scan,
                                        detail: err.to_string(),
                                    },
                                },
                            );
                            return;
                        }
                        (
                            plan.to_process,
                            plan.total_scanned,
                            plan.selected,
                            plan.protected_skipped,
                        )
                    }
                    Err(err) => {
                        let _ = append_backfill_event(
                            &ledger_path,
                            &BackfillOperationEvent {
                                schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
                                event_id: format!("{operation_id}:fail-{generation}"),
                                operation_id: operation_id.clone(),
                                ts: Utc::now().to_rfc3339(),
                                payload: BackfillOperationPayload::AttemptFailed {
                                    generation,
                                    stage: BackfillFailureStage::Scan,
                                    detail: err.to_string(),
                                },
                            },
                        );
                        return;
                    }
                }
            } else {
                (
                    state.planned_segments.clone(),
                    state.total_scanned,
                    state.selected_count,
                    state.protected_skipped,
                )
            };

        #[cfg(test)]
        let inject_executor = {
            let hooks = TEST_HOOKS.lock().unwrap_or_else(|e| e.into_inner());
            hooks.inject_executor_failure.clone()
        };
        #[cfg(test)]
        if let Some(detail) = inject_executor {
            let _ = append_backfill_event(
                &ledger_path,
                &BackfillOperationEvent {
                    schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
                    event_id: format!("{operation_id}:fail-{generation}"),
                    operation_id: operation_id.clone(),
                    ts: Utc::now().to_rfc3339(),
                    payload: BackfillOperationPayload::AttemptFailed {
                        generation,
                        stage: BackfillFailureStage::Executor,
                        detail,
                    },
                },
            );
            return;
        }

        let mut batch = Vec::new();
        let mut batch_index = 0;

        let uncheckpointed = planned_segments
            .into_iter()
            .filter(|seg| {
                !matches!(
                    state.checkpointed_segments.get(seg),
                    Some(BackfillCheckpointOutcome::Processed | BackfillCheckpointOutcome::Skipped)
                )
            })
            .collect::<Vec<_>>();

        for key in uncheckpointed {
            // Check if active token is still valid
            {
                let registry = ACTIVE_TOKENS.lock().unwrap();
                if registry.get(&(identity, operation_id.clone())) != Some(&token_id) {
                    return;
                }
            }

            let segment_path = match resolve_exact(
                &journal_root,
                &key.day,
                &key.stream,
                &key.segment_key,
                key.stream_layout,
            ) {
                Ok(Some(path)) => path,
                _ => {
                    batch.push(BackfillCheckpointResult {
                        segment: key.clone(),
                        outcome: BackfillCheckpointOutcome::Skipped,
                        error_detail: None,
                    });
                    if batch.len() >= 64 {
                        batch_index += 1;
                        let _ = append_batch(
                            &ledger_path,
                            &operation_id,
                            generation,
                            batch_index,
                            std::mem::take(&mut batch),
                        );
                    }
                    continue;
                }
            };

            let (outcome, error_detail) = execute_backfill_member(
                &journal_root,
                &key,
                &segment_path,
                state.commit,
                state.accumulation,
                now_ms,
            );

            batch.push(BackfillCheckpointResult {
                segment: key.clone(),
                outcome,
                error_detail,
            });

            if batch.len() >= 64 {
                batch_index += 1;
                let _ = append_batch(
                    &ledger_path,
                    &operation_id,
                    generation,
                    batch_index,
                    std::mem::take(&mut batch),
                );
            }
        }

        if !batch.is_empty() {
            batch_index += 1;
            let _ = append_batch(&ledger_path, &operation_id, generation, batch_index, batch);
        }

        // Re-fold and complete if 0 errors and 0 pending
        if let Ok(Some(final_state)) = load_and_fold(&ledger_path, &operation_id)
            && final_state.pending_segments.is_empty()
            && final_state.error_details.is_empty()
        {
            let _ = append_backfill_event(
                &ledger_path,
                &BackfillOperationEvent {
                    schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
                    event_id: format!("{operation_id}:completed"),
                    operation_id: operation_id.clone(),
                    ts: Utc::now().to_rfc3339(),
                    payload: BackfillOperationPayload::Completed {
                        completed_at: Utc::now().to_rfc3339(),
                    },
                },
            );
        }
    }));

    if attempt_res.is_err() {
        let _ = append_backfill_event(
            &ledger_path,
            &BackfillOperationEvent {
                schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
                event_id: format!("{operation_id}:fail-{generation}"),
                operation_id: operation_id.clone(),
                ts: Utc::now().to_rfc3339(),
                payload: BackfillOperationPayload::AttemptFailed {
                    generation,
                    stage: BackfillFailureStage::Panic,
                    detail: "attempt panicked during execution".to_owned(),
                },
            },
        );
    }

    drop(execution_lock);
    clear_active_token(identity, &operation_id, token_id);
}

fn append_batch(
    ledger_path: &Path,
    operation_id: &str,
    generation: u64,
    batch_index: usize,
    results: Vec<BackfillCheckpointResult>,
) -> Result<(), BackfillOperationError> {
    append_backfill_event(
        ledger_path,
        &BackfillOperationEvent {
            schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
            event_id: format!("{operation_id}:batch-{generation}:{batch_index}"),
            operation_id: operation_id.to_owned(),
            ts: Utc::now().to_rfc3339(),
            payload: BackfillOperationPayload::CheckpointBatch {
                generation,
                results,
            },
        },
    )
}

fn load_and_fold(
    ledger_path: &Path,
    operation_id: &str,
) -> Result<Option<BackfillOperationState>, BackfillOperationError> {
    let rows = load_backfill_operations(ledger_path)?;
    fold_backfill_operation(&rows, operation_id)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::backfill_operations::{
        BACKFILL_OPERATION_SCHEMA_VERSION, BackfillCheckpointOutcome, BackfillCheckpointResult,
        BackfillFailureStage, BackfillOperationEvent, BackfillOperationPayload, BackfillSegmentKey,
        BackfillStatusKind,
    };
    use solstone_core_journal_io::{LockOptions, SegmentLayout, hold_lock};

    static NEXT: AtomicUsize = AtomicUsize::new(0);
    static COORD_TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct Temp(PathBuf);

    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "solstone-coordinator-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn start_and_status_synthesizes_active_then_completed() {
        let _test_lock = COORD_TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let temp = Temp::new();
        let segment_dir = temp.path().join("chronicle/20260808/named/mic/120000_300");
        fs::create_dir_all(segment_dir.join("talents")).unwrap();
        fs::write(
            segment_dir.join("talents/speaker_labels.json"),
            r#"{"labels": [], "skipped": true}"#,
        )
        .unwrap();

        let req = StartBackfillRequest {
            operation_id: Some("bfop_coord_test".to_owned()),
            commit: true,
            reattribute: false,
            accumulation: false,
            now_ms: 1,
        };

        let status = start_backfill(temp.path(), &req).unwrap();
        assert_eq!(status.operation_id, "bfop_coord_test");
        assert!(matches!(
            status.status,
            BackfillStatusKind::ActivePreparing
                | BackfillStatusKind::ActiveRunning
                | BackfillStatusKind::Done
        ));

        // Poll until done
        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(20));
            if let Some(s) = backfill_status(temp.path(), "bfop_coord_test").unwrap()
                && s.status == BackfillStatusKind::Done
            {
                break;
            }
        }

        let final_status = backfill_status(temp.path(), "bfop_coord_test")
            .unwrap()
            .unwrap();
        assert_eq!(final_status.status, BackfillStatusKind::Done);
        assert!(final_status.done);
    }

    #[test]
    fn immutable_flags_mismatch_fails_loudly() {
        let _test_lock = COORD_TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let temp = Temp::new();
        let req1 = StartBackfillRequest {
            operation_id: Some("bfop_flags".to_owned()),
            commit: true,
            reattribute: false,
            accumulation: false,
            now_ms: 1,
        };
        start_backfill(temp.path(), &req1).unwrap();

        let req2 = StartBackfillRequest {
            operation_id: Some("bfop_flags".to_owned()),
            commit: false, // mismatch
            reattribute: false,
            accumulation: false,
            now_ms: 1,
        };
        assert!(matches!(
            start_backfill(temp.path(), &req2),
            Err(BackfillCoordinatorError::ImmutableFlagsMismatch { .. })
        ));
    }

    #[test]
    fn invalid_flags_accumulation_without_commit_rejected() {
        let _test_lock = COORD_TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let temp = Temp::new();
        let req = StartBackfillRequest {
            operation_id: Some("bfop_invalid_flags".to_owned()),
            commit: false,
            reattribute: false,
            accumulation: true,
            now_ms: 1,
        };
        assert!(matches!(
            start_backfill(temp.path(), &req),
            Err(BackfillCoordinatorError::InvalidFlags(_))
        ));
    }

    #[test]
    fn mint_operation_id_format() {
        let id = mint_operation_id();
        assert!(id.starts_with("bfop-"));
        assert_eq!(id.len(), 5 + 32);
        assert!(
            id[5..]
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn barrier_overlapping_same_id_same_flags_single_owner() {
        let _test_lock = COORD_TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        use std::sync::{Arc, Barrier};

        let temp = Arc::new(Temp::new());
        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();

        for _ in 0..2 {
            let t = Arc::clone(&temp);
            let b = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                b.wait();
                start_backfill(
                    t.path(),
                    &StartBackfillRequest {
                        operation_id: Some("bfop-barrier".to_owned()),
                        commit: true,
                        reattribute: false,
                        accumulation: false,
                        now_ms: 1,
                    },
                )
            }));
        }

        let mut results = Vec::new();
        for h in handles {
            results.push(h.join().unwrap().unwrap());
        }

        assert_eq!(results[0].operation_id, "bfop-barrier");
        assert_eq!(results[1].operation_id, "bfop-barrier");

        // Exactly 1 accepted event in ledger
        let ledger = backfill_operations_path(temp.path());
        let rows = load_backfill_operations(&ledger).unwrap();
        let accepted_count = rows
            .iter()
            .filter(|r| matches!(r.event.payload, BackfillOperationPayload::Accepted { .. }))
            .count();
        assert_eq!(accepted_count, 1);

        // Wait until done
        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(20));
            if let Some(s) = backfill_status(temp.path(), "bfop-barrier").unwrap()
                && s.status == BackfillStatusKind::Done
            {
                break;
            }
        }
        let final_status = backfill_status(temp.path(), "bfop-barrier")
            .unwrap()
            .unwrap();
        assert_eq!(final_status.status, BackfillStatusKind::Done);
    }

    #[test]
    fn immutable_flags_each_changed_individually_and_completed_id_reuse() {
        let _test_lock = COORD_TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let temp = Temp::new();
        let op_id = "bfop-immutable-test";
        let req = StartBackfillRequest {
            operation_id: Some(op_id.to_owned()),
            commit: true,
            reattribute: false,
            accumulation: false,
            now_ms: 1,
        };
        start_backfill(temp.path(), &req).unwrap();

        // Wait until done
        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(20));
            if let Some(s) = backfill_status(temp.path(), op_id).unwrap()
                && s.status == BackfillStatusKind::Done
            {
                break;
            }
        }

        let ledger = backfill_operations_path(temp.path());
        let bytes_before = fs::read(&ledger).unwrap();

        // 1. Commit changed
        let req_commit = StartBackfillRequest {
            operation_id: Some(op_id.to_owned()),
            commit: false,
            reattribute: false,
            accumulation: false,
            now_ms: 1,
        };
        assert!(matches!(
            start_backfill(temp.path(), &req_commit),
            Err(BackfillCoordinatorError::ImmutableFlagsMismatch { .. })
        ));
        assert_eq!(fs::read(&ledger).unwrap(), bytes_before);

        // 2. Reattribute changed
        let req_reattr = StartBackfillRequest {
            operation_id: Some(op_id.to_owned()),
            commit: true,
            reattribute: true,
            accumulation: false,
            now_ms: 1,
        };
        assert!(matches!(
            start_backfill(temp.path(), &req_reattr),
            Err(BackfillCoordinatorError::ImmutableFlagsMismatch { .. })
        ));
        assert_eq!(fs::read(&ledger).unwrap(), bytes_before);

        // 3. Accumulation changed
        let req_accum = StartBackfillRequest {
            operation_id: Some(op_id.to_owned()),
            commit: true,
            reattribute: false,
            accumulation: true,
            now_ms: 1,
        };
        assert!(matches!(
            start_backfill(temp.path(), &req_accum),
            Err(BackfillCoordinatorError::ImmutableFlagsMismatch { .. })
        ));
        assert_eq!(fs::read(&ledger).unwrap(), bytes_before);

        // Completed ID + same flags returns done without new rows or tokens
        let reuse_status = start_backfill(temp.path(), &req).unwrap();
        assert_eq!(reuse_status.status, BackfillStatusKind::Done);
        assert_eq!(fs::read(&ledger).unwrap(), bytes_before);
    }

    #[test]
    fn distinct_canonical_journal_roots_same_id_flags_independent() {
        let _test_lock = COORD_TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let temp1 = Temp::new();
        let temp2 = Temp::new();
        let op_id = "bfop-multi-root";

        let req = StartBackfillRequest {
            operation_id: Some(op_id.to_owned()),
            commit: true,
            reattribute: false,
            accumulation: false,
            now_ms: 1,
        };

        let s1 = start_backfill(temp1.path(), &req).unwrap();
        let s2 = start_backfill(temp2.path(), &req).unwrap();
        assert_eq!(s1.operation_id, op_id);
        assert_eq!(s2.operation_id, op_id);

        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(20));
            if let Some(s) = backfill_status(temp1.path(), op_id).unwrap()
                && s.status == BackfillStatusKind::Done
            {
                break;
            }
        }

        let f1 = backfill_status(temp1.path(), op_id).unwrap().unwrap();
        assert_eq!(f1.status, BackfillStatusKind::Done);

        // temp2 completes independently
        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(20));
            if let Some(s) = backfill_status(temp2.path(), op_id).unwrap()
                && s.status == BackfillStatusKind::Done
            {
                break;
            }
        }
        let f2 = backfill_status(temp2.path(), op_id).unwrap().unwrap();
        assert_eq!(f2.status, BackfillStatusKind::Done);
    }

    #[test]
    fn different_id_loses_execution_lock_inactive_accepted_and_resume() {
        let _test_lock = COORD_TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let temp = Temp::new();
        let op_id = "bfop-lock-loss";

        // Hold execution lock externally
        let lock_target = temp.path().join("health/locks/speaker-backfill");
        fs::create_dir_all(lock_target.parent().unwrap()).unwrap();
        let lock_guard = hold_lock(
            &lock_target,
            LockOptions {
                timeout: Duration::ZERO,
                ..Default::default()
            },
        )
        .unwrap();

        let req = StartBackfillRequest {
            operation_id: Some(op_id.to_owned()),
            commit: true,
            reattribute: false,
            accumulation: false,
            now_ms: 1,
        };
        let started = start_backfill(temp.path(), &req).unwrap();
        assert_eq!(started.operation_id, op_id);

        // Wait a little for attempt thread to fail to acquire execution lock and exit
        std::thread::sleep(Duration::from_millis(50));

        let s = backfill_status(temp.path(), op_id).unwrap().unwrap();
        assert_eq!(s.status, BackfillStatusKind::InactiveAccepted);
        assert_eq!(s.latest_generation, None);

        // Drop execution lock and resume
        drop(lock_guard);
        let resumed = resume_backfill(temp.path(), op_id, 2).unwrap();
        assert_eq!(resumed.operation_id, op_id);

        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(20));
            if let Some(s) = backfill_status(temp.path(), op_id).unwrap()
                && s.status == BackfillStatusKind::Done
            {
                break;
            }
        }
        let final_status = backfill_status(temp.path(), op_id).unwrap().unwrap();
        assert_eq!(final_status.status, BackfillStatusKind::Done);
        assert_eq!(final_status.latest_generation, Some(1));
    }

    #[test]
    fn process_death_after_accepted_before_generation_resumes_cleanly() {
        let _test_lock = COORD_TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let temp = Temp::new();
        let op_id = "bfop-death-sim";
        let admitted = JournalRoot::open(temp.path()).unwrap();
        let identity = admitted.identity();

        // A process that died right after accepting: the accepted event is in
        // the ledger, and no worker was ever spawned. Writing it directly makes
        // that state exact; calling start_backfill would race its own worker.
        let ledger_path = backfill_operations_path(admitted.canonical_path());
        fs::create_dir_all(ledger_path.parent().unwrap()).unwrap();
        {
            let _ledger_lock = hold_lock(&ledger_path, LockOptions::default()).unwrap();
            crate::backfill_operations::append_backfill_event_locked(
                &ledger_path,
                &BackfillOperationEvent {
                    schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
                    event_id: format!("{op_id}:accepted"),
                    operation_id: op_id.to_owned(),
                    ts: Utc::now().to_rfc3339(),
                    payload: BackfillOperationPayload::Accepted {
                        commit: true,
                        reattribute: false,
                        accumulation: false,
                    },
                },
            )
            .unwrap();
        }
        assert!(
            !ACTIVE_TOKENS
                .lock()
                .unwrap()
                .contains_key(&(identity, op_id.to_owned()))
        );

        let s = backfill_status(temp.path(), op_id).unwrap().unwrap();
        assert_eq!(s.status, BackfillStatusKind::InactiveAccepted);

        // Resume creates attempt and completes
        let _ = resume_backfill(temp.path(), op_id, 2);
        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(20));
            if let Some(s) = backfill_status(temp.path(), op_id).unwrap()
                && s.status == BackfillStatusKind::Done
            {
                break;
            }
        }
        let final_status = backfill_status(temp.path(), op_id).unwrap().unwrap();
        assert_eq!(final_status.status, BackfillStatusKind::Done);
    }

    #[test]
    fn attempt_failed_then_progress_then_token_drop_reports_inactive_interrupted() {
        let _test_lock = COORD_TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let temp = Temp::new();
        let op_id = "bfop-progress-test";
        let ledger = backfill_operations_path(temp.path());
        fs::create_dir_all(ledger.parent().unwrap()).unwrap();

        // Generation 1 fails
        append_backfill_event(
            &ledger,
            &BackfillOperationEvent {
                schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
                event_id: format!("{op_id}:accepted"),
                operation_id: op_id.to_owned(),
                ts: "2026-08-08T00:00:00Z".to_owned(),
                payload: BackfillOperationPayload::Accepted {
                    commit: true,
                    reattribute: false,
                    accumulation: false,
                },
            },
        )
        .unwrap();
        append_backfill_event(
            &ledger,
            &BackfillOperationEvent {
                schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
                event_id: format!("{op_id}:att-1"),
                operation_id: op_id.to_owned(),
                ts: "2026-08-08T00:00:01Z".to_owned(),
                payload: BackfillOperationPayload::AttemptStarted { generation: 1 },
            },
        )
        .unwrap();
        append_backfill_event(
            &ledger,
            &BackfillOperationEvent {
                schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
                event_id: format!("{op_id}:fail-1"),
                operation_id: op_id.to_owned(),
                ts: "2026-08-08T00:00:02Z".to_owned(),
                payload: BackfillOperationPayload::AttemptFailed {
                    generation: 1,
                    stage: BackfillFailureStage::Scan,
                    detail: "transient scan failure".to_owned(),
                },
            },
        )
        .unwrap();

        // Generation 2 starts and makes progress on 1 of 2 segments
        append_backfill_event(
            &ledger,
            &BackfillOperationEvent {
                schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
                event_id: format!("{op_id}:att-2"),
                operation_id: op_id.to_owned(),
                ts: "2026-08-08T00:00:03Z".to_owned(),
                payload: BackfillOperationPayload::AttemptStarted { generation: 2 },
            },
        )
        .unwrap();
        let seg1 = BackfillSegmentKey {
            day: "20260808".to_owned(),
            stream_layout: SegmentLayout::Named,
            stream: "mic".to_owned(),
            segment_key: "120000_300".to_owned(),
        };
        let seg2 = BackfillSegmentKey {
            day: "20260808".to_owned(),
            stream_layout: SegmentLayout::Named,
            stream: "mic".to_owned(),
            segment_key: "120500_300".to_owned(),
        };
        append_backfill_event(
            &ledger,
            &BackfillOperationEvent {
                schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
                event_id: format!("{op_id}:prep-2"),
                operation_id: op_id.to_owned(),
                ts: "2026-08-08T00:00:04Z".to_owned(),
                payload: BackfillOperationPayload::Prepared {
                    started_at: "2026-08-08T00:00:04Z".to_owned(),
                    reattribute: false,
                    total_count: 2,
                    segments: vec![seg1.clone(), seg2.clone()],
                    total_scanned: 2,
                    selected: 2,
                    protected_skipped: 0,
                },
            },
        )
        .unwrap();
        append_backfill_event(
            &ledger,
            &BackfillOperationEvent {
                schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
                event_id: format!("{op_id}:batch-2:1"),
                operation_id: op_id.to_owned(),
                ts: "2026-08-08T00:00:05Z".to_owned(),
                payload: BackfillOperationPayload::CheckpointBatch {
                    generation: 2,
                    results: vec![BackfillCheckpointResult {
                        segment: seg1,
                        outcome: BackfillCheckpointOutcome::Processed,
                        error_detail: None,
                    }],
                },
            },
        )
        .unwrap();

        // Check status without active token
        let s = backfill_status(temp.path(), op_id).unwrap().unwrap();
        assert_eq!(s.status, BackfillStatusKind::InactiveInterrupted);
        assert_eq!(s.latest_generation, Some(2));
        assert_eq!(s.completed_count, 1);
        assert_eq!(s.pending_count, 1);
        assert_eq!(s.failure_stage, Some(BackfillFailureStage::Scan));
        assert!(s.failure_detail.is_some());
    }

    #[test]
    fn injected_failures_scan_executor_panic_produce_distinct_details_and_stages() {
        let _test_lock = COORD_TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let temp = Temp::new();

        // 1. Scan failure
        {
            let mut hooks = TEST_HOOKS.lock().unwrap_or_else(|e| e.into_inner());
            hooks.inject_scan_failure = Some("injected_scan_err_xyz".to_owned());
        }
        let s_scan = start_backfill(
            temp.path(),
            &StartBackfillRequest {
                operation_id: Some("bfop-inj-scan".to_owned()),
                commit: true,
                reattribute: false,
                accumulation: false,
                now_ms: 1,
            },
        )
        .unwrap();
        assert_eq!(s_scan.operation_id, "bfop-inj-scan");

        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(20));
            if let Some(s) = backfill_status(temp.path(), "bfop-inj-scan").unwrap()
                && s.status == BackfillStatusKind::AttemptFailed
            {
                break;
            }
        }
        let status_scan = backfill_status(temp.path(), "bfop-inj-scan")
            .unwrap()
            .unwrap();
        assert_eq!(status_scan.status, BackfillStatusKind::AttemptFailed);
        assert_eq!(status_scan.failure_stage, Some(BackfillFailureStage::Scan));
        let detail_scan = status_scan.failure_detail.clone().unwrap_or_default();
        assert!(!detail_scan.is_empty());

        // Reset hooks
        {
            let mut hooks = TEST_HOOKS.lock().unwrap_or_else(|e| e.into_inner());
            hooks.inject_scan_failure = None;
        }

        // 2. Executor failure
        {
            let mut hooks = TEST_HOOKS.lock().unwrap_or_else(|e| e.into_inner());
            hooks.inject_executor_failure = Some("injected_executor_err_abc".to_owned());
        }
        let _ = start_backfill(
            temp.path(),
            &StartBackfillRequest {
                operation_id: Some("bfop-inj-exec".to_owned()),
                commit: true,
                reattribute: false,
                accumulation: false,
                now_ms: 1,
            },
        )
        .unwrap();

        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(20));
            if let Some(s) = backfill_status(temp.path(), "bfop-inj-exec").unwrap()
                && s.status == BackfillStatusKind::AttemptFailed
            {
                break;
            }
        }
        let status_exec = backfill_status(temp.path(), "bfop-inj-exec")
            .unwrap()
            .unwrap();
        assert_eq!(status_exec.status, BackfillStatusKind::AttemptFailed);
        assert_eq!(
            status_exec.failure_stage,
            Some(BackfillFailureStage::Executor)
        );
        let detail_exec = status_exec.failure_detail.clone().unwrap_or_default();
        assert!(!detail_exec.is_empty());

        // Reset hooks
        {
            let mut hooks = TEST_HOOKS.lock().unwrap_or_else(|e| e.into_inner());
            hooks.inject_executor_failure = None;
        }

        // 3. Panic
        {
            let mut hooks = TEST_HOOKS.lock().unwrap_or_else(|e| e.into_inner());
            hooks.inject_panic = true;
        }
        let _ = start_backfill(
            temp.path(),
            &StartBackfillRequest {
                operation_id: Some("bfop-inj-panic".to_owned()),
                commit: true,
                reattribute: false,
                accumulation: false,
                now_ms: 1,
            },
        )
        .unwrap();

        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(20));
            if let Some(s) = backfill_status(temp.path(), "bfop-inj-panic").unwrap()
                && s.status == BackfillStatusKind::AttemptFailed
            {
                break;
            }
        }
        let status_panic = backfill_status(temp.path(), "bfop-inj-panic")
            .unwrap()
            .unwrap();
        assert_eq!(status_panic.status, BackfillStatusKind::AttemptFailed);
        assert_eq!(
            status_panic.failure_stage,
            Some(BackfillFailureStage::Panic)
        );
        let detail_panic = status_panic.failure_detail.clone().unwrap_or_default();
        assert!(!detail_panic.is_empty());

        // Reset hooks
        {
            let mut hooks = TEST_HOOKS.lock().unwrap_or_else(|e| e.into_inner());
            hooks.inject_panic = false;
        }

        // Assert all 3 details are distinct
        assert_ne!(detail_scan, detail_exec);
        assert_ne!(detail_exec, detail_panic);
        assert_ne!(detail_scan, detail_panic);
    }
}
